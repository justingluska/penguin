//! The key schedule and the encrypted channel both sides share.
//!
//! ```text
//! transcript = SHA-256("penguin-pair/3" ‖ mac_pub ‖ phone_pub)
//! prk        = HKDF-Extract(salt = transcript, ikm = X25519(…) ‖ qr_secret)
//! k_phone    = HKDF-Expand(prk, "penguin-pair/3 phone->mac", 32)
//! k_mac      = HKDF-Expand(prk, "penguin-pair/3 mac->phone", 32)
//! code       = HKDF-Expand(prk, "penguin-pair/3 code", 4) → 6 digits
//! ```
//!
//! The labels carry [`PROTOCOL_VERSION`], so two versions never derive the
//! same keys.
//!
//! Each direction is ChaCha20-Poly1305 under its own key, nonce = 4 zero
//! bytes ‖ the message counter (u64, big-endian, from 0), associated data =
//! the transcript. The counter isn't sent: a receiver opens only the next
//! message it expects, so a replayed, dropped or reordered frame fails.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use x25519_dalek::SharedSecret;
use zeroize::Zeroizing;

use crate::{PairError, Result, PROTOCOL_VERSION};

/// "penguin-pair/3".
fn label() -> String {
    format!("penguin-pair/{PROTOCOL_VERSION}")
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Mac,
    Phone,
}

struct Direction {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Direction {
    fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(key.into()),
            counter: 0,
        }
    }

    fn nonce(&self) -> Nonce {
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&self.counter.to_be_bytes());
        nonce.into()
    }
}

/// An established, encrypted channel (one per pairing).
pub(crate) struct Channel {
    send: Direction,
    recv: Direction,
    transcript: [u8; 32],
    code: String,
}

impl Channel {
    pub fn derive(
        role: Role,
        dh: &SharedSecret,
        qr_secret: &[u8; 32],
        mac_pub: &[u8; 32],
        phone_pub: &[u8; 32],
    ) -> Result<Self> {
        // A low-order public key forces a known DH output; the QR secret
        // still keeps the keys unguessable, but refuse it outright.
        if !dh.was_contributory() {
            return Err(PairError::BadKey);
        }
        let mut hash = Sha256::new();
        let label = label();
        hash.update(label.as_bytes());
        hash.update(mac_pub);
        hash.update(phone_pub);
        let transcript: [u8; 32] = hash.finalize().into();

        let mut ikm = Zeroizing::new([0u8; 64]);
        ikm[..32].copy_from_slice(dh.as_bytes());
        ikm[32..].copy_from_slice(qr_secret);
        let hk = Hkdf::<Sha256>::new(Some(&transcript), ikm.as_slice());

        let expand = |info: &str| -> Result<Zeroizing<[u8; 32]>> {
            let mut out = Zeroizing::new([0u8; 32]);
            hk.expand(format!("{label} {info}").as_bytes(), out.as_mut_slice())
                .map_err(|_| PairError::Other("key derivation failed".into()))?;
            Ok(out)
        };
        let to_mac = expand("phone->mac")?;
        let to_phone = expand("mac->phone")?;
        let code_bytes = expand("code")?;
        let n = u32::from_be_bytes([code_bytes[0], code_bytes[1], code_bytes[2], code_bytes[3]])
            % 1_000_000;
        let code = format!("{:03} {:03}", n / 1000, n % 1000);

        let (send, recv) = match role {
            Role::Phone => (Direction::new(&to_mac), Direction::new(&to_phone)),
            Role::Mac => (Direction::new(&to_phone), Direction::new(&to_mac)),
        };
        Ok(Self {
            send,
            recv,
            transcript,
            code,
        })
    }

    /// The six-digit code both screens show ("482 193").
    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = self.send.nonce();
        let sealed = self
            .send
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: &self.transcript,
                },
            )
            .map_err(|_| PairError::Other("encryption failed".into()))?;
        self.send.counter = self
            .send
            .counter
            .checked_add(1)
            .ok_or_else(|| PairError::Other("message counter exhausted".into()))?;
        Ok(sealed)
    }

    /// Open the next expected message. A failure leaves the counter alone,
    /// but callers end the pairing on any failure.
    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let nonce = self.recv.nonce();
        let plain = self
            .recv
            .cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad: &self.transcript,
                },
            )
            .map_err(|_| PairError::Tampered)?;
        self.recv.counter += 1;
        Ok(Zeroizing::new(plain))
    }
}
