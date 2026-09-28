//! penguin-imap: IMAP + SMTP accounts (Yahoo, AOL, iCloud, Fastmail, Proton
//! Mail Bridge, any server, and Gmail with an app password) behind the
//! provider seam (docs/PROVIDERS-IMPL.md).
//!
//! - [`ImapBackend`] (the kind's `Backend`): clients, Keychain, sync tasks.
//! - [`ImapProvider`] (one account's `MailProvider`).
//! - [`connect`]: checking a new account's servers and password.
//!
//! Layers, bottom up: `proto` (wire format), `net` (TCP/TLS), `session`
//! (IMAP commands), `smtp`, then `folders`/`mime`/`threading`/`db` (mapping
//! server state onto Penguin's model), `account` (shared per-account
//! context), and `provider`/`sync`/`backend` on top.
//!
//! Privacy: never logs bodies, subjects, addresses or passwords (ids,
//! counts and codes only); server texts in errors go through [`redact`].

pub mod account;
pub mod backend;
pub mod compress;
pub mod connect;
pub mod cursor;
pub mod db;
pub mod folders;
pub mod mime;
pub mod net;
pub mod proto;
pub mod provider;
pub mod search;
pub mod session;
pub mod smtp;
pub mod sync;
pub mod threading;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod testserver;

pub use backend::ImapBackend;
pub use provider::ImapProvider;

/// The provider seam's error: this crate returns it everywhere.
pub use penguin_provider::Error;
pub type Result<T> = std::result::Result<T, Error>;

/// Replace anything that looks like an email address in server text, so
/// error messages (which get logged) never carry one.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let core = word.trim_matches(|c: char| "<>()[],;:\"'".contains(c));
        if core.contains('@') && core.len() > 2 && !core.starts_with('@') && !core.ends_with('@') {
            out.push_str(&word.replace(core, "<address>"));
        } else {
            out.push_str(word);
        }
    }
    out
}

#[cfg(test)]
mod lib_tests {
    #[test]
    fn redacts_addresses() {
        assert_eq!(
            super::redact("550 5.1.1 <sam@mail.example>: user unknown"),
            "550 5.1.1 <<address>>: user unknown"
        );
        assert_eq!(super::redact("no @ here"), "no @ here");
    }
}
