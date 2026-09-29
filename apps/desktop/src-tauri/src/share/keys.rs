//! Object keys and the headers an upload carries.
//!
//! A key is `penguin/<token>/<name>`: `token` is 128 random bits from the OS
//! (26 lowercase base32 characters), so a key can't be guessed or listed from
//! another one, and `name` is the original file name made URL- and
//! shell-friendly (an agent pastes the link into `curl -O`). The original
//! name, Unicode and all, travels in `Content-Disposition` instead.

/// Every key Penguin creates starts with this (a lifecycle rule can match it).
pub const PREFIX: &str = "penguin/";
/// Test uploads (Settings → Share links → Test) go here.
pub const TEST_PREFIX: &str = "penguin/test/";
/// The longest `name` part of a key, in bytes (ASCII only, so also chars).
const MAX_NAME: usize = 100;
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
/// 128 bits in base32 (5 bits per character).
pub const TOKEN_LEN: usize = 26;

/// 16 random bytes from the OS as lowercase RFC 4648 base32 without
/// padding. None when the OS generator fails: then no share is made (a key
/// from a weaker source could be guessed).
pub fn token() -> Option<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    Some(base32(&bytes))
}

pub(crate) fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((acc >> bits) & 31) as usize] as char);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(BASE32[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// A new, unguessable key for a file called `filename` (see [`token`]).
pub fn object_key(filename: &str) -> Option<String> {
    Some(format!("{PREFIX}{}/{}", token()?, key_name(filename)))
}

/// A key Penguin made for a share: `penguin/<26 base32>/<safe name>`. The
/// only keys `share_delete` and the cleanup ever touch.
pub fn is_share_key(key: &str) -> bool {
    let Some(rest) = key.strip_prefix(PREFIX) else {
        return false;
    };
    let Some((token, name)) = rest.split_once('/') else {
        return false;
    };
    token.len() == TOKEN_LEN
        && token.bytes().all(|b| BASE32.contains(&b))
        && !name.is_empty()
        && name.len() <= MAX_NAME
        && name.bytes().all(is_name_byte)
        && !name.starts_with('.')
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')
}

/// The file name as a key segment: ASCII letters, digits, `.`, `-` and `_`;
/// whitespace becomes `-`, anything else `_` (runs collapse to one), no
/// leading dots or dashes, at most 100 bytes with the extension kept, and
/// `file` when nothing is left.
pub fn key_name(filename: &str) -> String {
    // Only the last path segment, whatever the separator.
    let base = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename)
        .trim();
    let mut out = String::with_capacity(base.len());
    for c in base.chars() {
        let mapped = if c.is_ascii() && is_name_byte(c as u8) {
            c
        } else if c.is_whitespace() {
            '-'
        } else {
            '_'
        };
        let last = out.chars().last();
        if matches!(mapped, '-' | '_') && last == Some(mapped) {
            continue;
        }
        out.push(mapped);
    }
    let trimmed = out.trim_start_matches(['.', '-', '_']).to_string();
    let trimmed = trimmed.trim_end_matches(['-', '_', '.']).to_string();
    if trimmed.is_empty() || trimmed.bytes().all(|b| !b.is_ascii_alphanumeric()) {
        return "file".into();
    }
    if trimmed.len() <= MAX_NAME {
        return trimmed;
    }
    // Keep a short extension (".pdf", ".jpeg") and cut the stem.
    match trimmed.rfind('.') {
        Some(i) if i > 0 && trimmed.len() - i <= 11 => {
            let ext = &trimmed[i..];
            let stem = trimmed[..MAX_NAME - ext.len()].trim_end_matches(['-', '_', '.']);
            format!("{stem}{ext}")
        }
        _ => trimmed[..MAX_NAME].to_string(),
    }
}

/// `Content-Type` for the upload: the attachment's own type when it is a
/// well-formed `type/subtype`, else `application/octet-stream`.
pub fn content_type(mime: &str) -> String {
    let mime = mime.trim().to_ascii_lowercase();
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    match mime.split_once('/') {
        Some((t, sub)) if token(t) && token(sub) => mime,
        _ => "application/octet-stream".into(),
    }
}

/// `Content-Disposition` for the upload, so a download keeps the original
/// name: an ASCII `filename` fallback plus the exact name as RFC 5987
/// `filename*`. Pictures open in the browser (`inline`); anything else
/// downloads (`attachment`), so a shared HTML file never renders on the
/// storage's domain.
pub fn content_disposition(filename: &str, content_type: &str) -> String {
    let name = crate::ops::sanitize_filename(filename);
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::with_capacity(name.len() * 3);
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    let kind = if matches!(
        content_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) {
        "inline"
    } else {
        "attachment"
    };
    format!("{kind}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}
