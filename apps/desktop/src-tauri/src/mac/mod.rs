//! AppKit and WebKit calls behind the file clipboard (`pasteboard`) and
//! spell checking (`spelling`). Only objc2 types here, no Tauri ones, so this
//! directory type-checks on its own for a Mac target (e.g. from a Linux box:
//! a scratch crate that `#[path]`-includes it, then `cargo check --target
//! aarch64-apple-darwin`). The Tauri glue is in file_export.rs and
//! spelling.rs.

pub mod pasteboard;
pub mod spelling;
