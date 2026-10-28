//! Sender avatars on the Mac: contact photos and brand logos in place of
//! monograms. The resolver itself (sources, cache, the hardened fetcher,
//! authentication gating) is the shared `penguin-avatars` crate, also used
//! by the iPhone app; this module is the Mac's glue:
//!
//! - commands.rs: the Tauri commands, the resolve worker (which emits
//!   `penguin://avatars-changed`) and the daily contacts sync;
//! - protocol.rs: the `avatar:` URI scheme serving cached PNGs to the
//!   webview;
//! - account.rs: each signed-in account's own Google profile photo.
//!
//! The crate's modules are re-exported here so the rest of the app keeps
//! naming them `crate::avatars::net`, `crate::avatars::dns`, ….

pub mod account;
pub mod commands;
pub mod protocol;

pub use penguin_avatars::{cache, dns, image, net, people, sources};
pub use penguin_avatars::{
    image_url, now_secs, AvatarInfo, AvatarKind, AvatarRequest, Avatars, ContactsState, Job, Prefs,
    Resolver,
};

pub const EVENT_AVATARS_CHANGED: &str = "penguin://avatars-changed";

#[cfg(test)]
mod tests;
