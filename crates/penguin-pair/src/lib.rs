//! Pair iPhone: the Mac hands its accounts' credentials (Google and
//! Microsoft refresh tokens, IMAP app passwords) to the phone over the
//! local network, with no server in between. The design and
//! threat model are in docs/PAIRING.md; in short:
//!
//! 1. The Mac makes an [`Offer`]: a fresh X25519 key and a 32-byte one-time
//!    secret. Both go into the QR code ([`Invitation`]) with the addresses
//!    the Mac listens on. The offer expires after two minutes and serves one
//!    phone.
//! 2. The phone scans it, makes its own fresh X25519 key and sends a hello:
//!    its public key plus an encrypted [`PhoneHello`] (who it is, its Google
//!    client, the accounts it already has).
//! 3. Both sides derive the session keys with HKDF-SHA256 from the X25519
//!    result AND the QR secret, salted with a hash of both public keys. A
//!    device without the QR can't produce a hello the Mac accepts; a device
//!    without the Mac's private key (anyone on the network, even one who saw
//!    the QR) can't read the hello or answer it as the Mac.
//! 4. The Mac shows the phone and a six-digit code (derived from the same
//!    keys, also shown on the phone) and sends nothing until the user
//!    clicks Send. Then the [`Transfer`]: v3 Keychain vault entries for
//!    Gmail, the Mac's "Desktop app" client when any of them are browser
//!    sign-ins (a refresh token only refreshes with the client that minted
//!    it), the IMAP and Microsoft vault entries, and account names, colors,
//!    order and (IMAP, Microsoft) server settings. The phone answers
//!    [`PhoneDone`].
//!
//! Every message after the hello is ChaCha20-Poly1305 with a per-direction
//! key and an implicit counter nonce, so replayed, reordered or altered
//! frames fail to open. Secrets are zeroized when dropped.
//!
//! The protocol is sans-IO ([`Offer`]/[`MacSession`] on the Mac,
//! [`PhoneSession`] on the phone) over length-prefixed frames ([`frame`]);
//! the `tokio` feature adds the Mac's listener ([`server`]).

mod crypto;
pub mod frame;
mod invitation;
mod mac;
mod messages;
mod phone;
#[cfg(feature = "tokio")]
pub mod server;
#[cfg(test)]
mod tests;

pub use invitation::Invitation;
pub use mac::{MacSession, Offer, OFFER_TTL};
pub use messages::{
    AccountKind, AccountMeta, AccountStatus, DesktopClient, FailedAccount, MacMessage, PhoneDone,
    PhoneHello, PhoneMessage, SecretString, SignInReason, Transfer, MAX_PROVIDER_CONFIG,
};
pub use phone::PhoneSession;

/// The protocol version: first byte of the hello, part of the QR text and
/// of the key-derivation labels. Both sides refuse any other version, with
/// an error that says which side to update.
///
/// - 1: tokens from the Mac's sign-in-sheet (iOS) client only.
/// - 2: the transfer may also carry the Mac's "Desktop app" client
///   ([`Transfer::desktop_client`]), so browser sign-ins move too.
/// - 3: IMAP and Microsoft accounts move too ([`Transfer::imap_vault`],
///   [`Transfer::microsoft_vault`], [`AccountMeta::provider`] and
///   [`AccountMeta::provider_config`]).
pub const PROTOCOL_VERSION: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairError {
    #[error("This isn't a Penguin pairing code. On your Mac open Settings → Accounts → Pair iPhone and scan the code it shows.")]
    NotACode,
    #[error(
        "This pairing code is from a newer version of Penguin. Update Penguin on both devices."
    )]
    UnsupportedVersion,
    /// A code or hello from an earlier protocol version (an older Penguin).
    #[error("The other device has an older version of Penguin. Update Penguin on both your Mac and your iPhone, then try again.")]
    OlderVersion,
    #[error(
        "The pairing code expired. Start again on your Mac (Settings → Accounts → Pair iPhone)."
    )]
    Expired,
    #[error("This pairing code was already used. Start again on your Mac (Settings → Accounts → Pair iPhone).")]
    AlreadyUsed,
    /// The other side doesn't hold the keys this pairing code stands for:
    /// a wrong or stale code, or someone else on the network.
    #[error("The other device didn't prove it has this pairing code.")]
    BadKey,
    #[error("Too many devices tried a wrong pairing code. Start again on your Mac.")]
    TooManyAttempts,
    /// A frame failed to authenticate: altered, replayed or out of order.
    #[error("A pairing message was damaged or replayed; nothing was saved. Try again.")]
    Tampered,
    #[error("A pairing message was too large.")]
    TooLarge,
    #[error("The other device sent something unexpected ({0}).")]
    Protocol(String),
    #[error("The other device disconnected before pairing finished.")]
    Disconnected,
    #[error("Pairing timed out.")]
    TimedOut,
    #[error("Network error: {0}")]
    Io(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = PairError> = std::result::Result<T, E>;

/// 32 bytes from the system's CSPRNG.
pub(crate) fn random32() -> Result<[u8; 32]> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| PairError::Other(format!("system RNG unavailable: {e}")))?;
    Ok(bytes)
}
