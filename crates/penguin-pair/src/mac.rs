//! The Mac's side: an [`Offer`] (what the QR code stands for) accepts one
//! phone's hello and becomes a [`MacSession`].

use std::time::{Duration, Instant};

use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::crypto::{Channel, Role};
use crate::messages::{from_json, to_json, MacMessage, PhoneHello, PhoneMessage};
use crate::{random32, Invitation, PairError, Result, PROTOCOL_VERSION};

/// How long a QR code is good for.
pub const OFFER_TTL: Duration = Duration::from_secs(120);
/// Wrong hellos tolerated before the offer is withdrawn. Guessing the
/// 256-bit secret is hopeless; this only stops a noisy device from keeping
/// an offer busy.
pub(crate) const MAX_FAILURES: u32 = 5;
/// version ‖ phone public key ‖ at least an AEAD tag.
const MIN_HELLO: usize = 1 + 32 + 16;

enum State {
    Open {
        failures: u32,
    },
    /// A phone proved the secret; the offer is spent.
    Used,
    TooManyAttempts,
}

/// One pairing offer: a fresh key pair and a one-time secret.
pub struct Offer {
    /// Taken (and wiped) once a phone pairs.
    key: Option<StaticSecret>,
    public: [u8; 32],
    secret: Zeroizing<[u8; 32]>,
    created: Instant,
    ttl: Duration,
    state: State,
}

impl Offer {
    pub fn new() -> Result<Self> {
        Self::with_ttl(OFFER_TTL, Instant::now())
    }

    pub fn with_ttl(ttl: Duration, now: Instant) -> Result<Self> {
        let key = StaticSecret::from(random32()?);
        let public = PublicKey::from(&key).to_bytes();
        Ok(Self {
            key: Some(key),
            public,
            secret: Zeroizing::new(random32()?),
            created: now,
            ttl,
            state: State::Open { failures: 0 },
        })
    }

    /// An offer with a chosen secret: a stand-in for an attacker who saw
    /// the QR code but doesn't have the Mac's key.
    #[cfg(test)]
    pub(crate) fn with_secret(secret: [u8; 32]) -> Self {
        let mut offer = Self::new().unwrap();
        offer.secret = Zeroizing::new(secret);
        offer
    }

    /// What goes into the QR code, for a listener on `port` at `hosts`.
    pub fn invitation(&self, port: u16, hosts: Vec<String>) -> Invitation {
        Invitation {
            mac_public: self.public,
            secret: self.secret.clone(),
            port,
            hosts,
        }
    }

    pub fn expires_at(&self) -> Instant {
        self.created + self.ttl
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at()
    }

    /// Check a phone's hello (a frame's body). On success the offer is
    /// spent: every later hello, even a replay of this one, is refused.
    pub fn accept(&mut self, hello: &[u8], now: Instant) -> Result<MacSession> {
        let failures = match self.state {
            State::Used => return Err(PairError::AlreadyUsed),
            State::TooManyAttempts => return Err(PairError::TooManyAttempts),
            State::Open { failures } => failures,
        };
        if self.is_expired(now) {
            return Err(PairError::Expired);
        }
        if hello.first().is_some_and(|v| *v > PROTOCOL_VERSION) {
            return Err(PairError::UnsupportedVersion);
        }
        if hello
            .first()
            .is_some_and(|v| (1..PROTOCOL_VERSION).contains(v))
        {
            // An older phone. It can't have scanned this offer's code (it
            // refuses the version), so count it like any wrong hello.
            return Err(self.failed(failures, PairError::OlderVersion));
        }
        if hello.len() < MIN_HELLO || hello[0] != PROTOCOL_VERSION {
            return Err(self.failed(failures, PairError::Protocol("not a hello".into())));
        }
        let phone_public: [u8; 32] = hello[1..33].try_into().expect("length checked");
        let Some(key) = &self.key else {
            return Err(PairError::AlreadyUsed);
        };
        let dh = key.diffie_hellman(&PublicKey::from(phone_public));
        let mut channel =
            match Channel::derive(Role::Mac, &dh, &self.secret, &self.public, &phone_public) {
                Ok(c) => c,
                Err(_) => return Err(self.failed(failures, PairError::BadKey)),
            };
        let plain = match channel.open(&hello[33..]) {
            Ok(p) => p,
            Err(_) => return Err(self.failed(failures, PairError::BadKey)),
        };
        // The phone proved it holds the QR secret: spend the offer now,
        // whatever the hello says, and wipe the key.
        self.state = State::Used;
        self.key = None;
        let phone = from_json::<PhoneHello>(&plain)?.sanitized()?;
        Ok(MacSession { channel, phone })
    }

    fn failed(&mut self, failures: u32, error: PairError) -> PairError {
        let failures = failures + 1;
        if failures >= MAX_FAILURES {
            self.state = State::TooManyAttempts;
            self.key = None;
            return PairError::TooManyAttempts;
        }
        self.state = State::Open { failures };
        error
    }
}

/// The Mac's end of an established pairing.
pub struct MacSession {
    channel: Channel,
    /// Who connected (cleaned up for display).
    pub phone: PhoneHello,
}

impl MacSession {
    /// The six digits the phone shows too.
    pub fn code(&self) -> &str {
        self.channel.code()
    }

    /// A frame body for the phone.
    pub fn seal(&mut self, message: &MacMessage) -> Result<Vec<u8>> {
        let json = to_json(message)?;
        self.channel.seal(&json)
    }

    /// The phone's next frame body.
    pub fn open(&mut self, body: &[u8]) -> Result<PhoneMessage> {
        let plain = self.channel.open(body)?;
        from_json(&plain)
    }
}
