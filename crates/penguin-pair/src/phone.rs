//! The phone's side: from a scanned [`Invitation`] to the hello and the
//! encrypted channel.

use x25519_dalek::{PublicKey, StaticSecret};

use crate::crypto::{Channel, Role};
use crate::messages::{from_json, to_json, MacMessage, PhoneHello, PhoneMessage};
use crate::{random32, Invitation, PairError, Result, PROTOCOL_VERSION};

/// The phone's end of a pairing. Made by [`PhoneSession::start`], which also
/// returns the hello to send first.
pub struct PhoneSession {
    channel: Channel,
    /// A frame from the Mac has opened: the other end holds the Mac's key.
    verified: bool,
}

impl PhoneSession {
    /// A fresh key for this pairing, the keys derived with the QR's Mac key
    /// and secret, and the hello frame body (version ‖ public key ‖ the
    /// sealed `hello`).
    pub fn start(invitation: &Invitation, hello: &PhoneHello) -> Result<(Self, Vec<u8>)> {
        // Used once, then wiped when dropped at the end of this function.
        let key = StaticSecret::from(random32()?);
        let public = PublicKey::from(&key).to_bytes();
        let dh = key.diffie_hellman(&PublicKey::from(invitation.mac_public));
        let mut channel = Channel::derive(
            Role::Phone,
            &dh,
            &invitation.secret,
            &invitation.mac_public,
            &public,
        )
        // Only a forged QR code carries a key that can't agree.
        .map_err(|_| PairError::NotACode)?;
        let sealed = channel.seal(&to_json(hello)?)?;
        let mut body = Vec::with_capacity(1 + 32 + sealed.len());
        body.push(PROTOCOL_VERSION);
        body.extend_from_slice(&public);
        body.extend_from_slice(&sealed);
        Ok((
            Self {
                channel,
                verified: false,
            },
            body,
        ))
    }

    /// The six digits the Mac shows too.
    pub fn code(&self) -> &str {
        self.channel.code()
    }

    /// The Mac's next frame body. A frame that doesn't open came from
    /// something without the Mac's key (or was altered): [`PairError::BadKey`]
    /// for the first one, [`PairError::Tampered`] after.
    pub fn open(&mut self, body: &[u8]) -> Result<MacMessage> {
        let plain = match self.channel.open(body) {
            Ok(plain) => plain,
            Err(_) if !self.verified => return Err(PairError::BadKey),
            Err(e) => return Err(e),
        };
        self.verified = true;
        from_json(&plain)
    }

    /// Whether the Mac has proved it holds the key in the QR code.
    pub fn verified(&self) -> bool {
        self.verified
    }

    /// A frame body for the Mac.
    pub fn seal(&mut self, message: &PhoneMessage) -> Result<Vec<u8>> {
        let json = to_json(message)?;
        self.channel.seal(&json)
    }
}
