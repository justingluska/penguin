//! Read receipts (RFC 8098 Message Disposition Notifications).
//!
//! Penguin can ask for a read receipt on mail you send (Settings → Privacy
//! "Ask for read receipts", off by default): the message carries a
//! `Disposition-Notification-To` header, and the recipient's mail app may
//! ask them whether to send one back. A receipt that comes back is an email
//! with a `message/disposition-notification` part; this module reads that
//! part, and `store_receipts.rs` keeps what it says so the sent message can
//! show "Read by …". Nothing here touches the network, and nothing ever
//! tracks anyone without their app's (and usually their) consent: see
//! docs/PRIVACY.md for why Penguin does receipts and not tracking pixels.

use serde::{Deserialize, Serialize};

/// The machine-readable part of a receipt, the fields Penguin uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mdn {
    /// Message-ID of the message the receipt is about, without `<>`.
    pub original_message_id: String,
    /// Who it's from: `Final-Recipient`, else `Original-Recipient`
    /// (the address, without the `rfc822;` type). None when neither parses.
    pub recipient: Option<String>,
    /// The disposition type, lowercased: `displayed` (read), `deleted`
    /// (deleted without being read), `dispatched`, `processed`, …
    pub disposition: String,
    /// `manual-action` or `automatic-action` (how the receipt was sent), when given.
    pub action_mode: Option<String>,
}

impl Mdn {
    /// The receipt says the message was displayed (read).
    pub fn is_read(&self) -> bool {
        self.disposition == "displayed"
    }
}

/// Largest part read (a real one is a few hundred bytes).
pub const MAX_MDN_BYTES: usize = 16 * 1024;

/// Parse a `message/disposition-notification` body. None unless it names
/// the original message and a disposition.
pub fn parse_mdn(text: &str) -> Option<Mdn> {
    let text = &text[..floor_char_boundary(text, MAX_MDN_BYTES)];
    let mut original = None;
    let mut final_rcpt = None;
    let mut original_rcpt = None;
    let mut disposition = None;
    for (name, value) in fields(text) {
        match name.to_ascii_lowercase().as_str() {
            "original-message-id" => original = bare_id(&value),
            "final-recipient" => final_rcpt = address(&value),
            "original-recipient" => original_rcpt = address(&value),
            "disposition" => disposition = Some(value),
            _ => {}
        }
    }
    let disposition = disposition?;
    // "manual-action/MDN-sent-manually; displayed/…modifiers"
    let (mode, kind) = match disposition.split_once(';') {
        Some((mode, kind)) => (Some(mode), kind),
        None => (None, disposition.as_str()),
    };
    let kind = kind.split('/').next().unwrap_or_default().trim().to_ascii_lowercase();
    if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
        return None;
    }
    let action_mode = mode
        .and_then(|m| m.split('/').next())
        .map(|m| m.trim().to_ascii_lowercase())
        .filter(|m| !m.is_empty());
    Some(Mdn {
        original_message_id: original?,
        recipient: final_rcpt.or(original_rcpt),
        disposition: kind,
        action_mode,
    })
}

/// Header-style fields with continuation lines unfolded. Stops at nothing:
/// an MDN body is all fields (RFC 8098 §3.1), possibly in groups.
fn fields(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, v)) = out.last_mut() {
                v.push(' ');
                v.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if !name.is_empty() && !name.contains(' ') {
                out.push((name.to_string(), value.trim().to_string()));
            }
        }
    }
    out
}

/// `<abc@host>` (maybe with comments or spaces) → `abc@host`.
fn bare_id(v: &str) -> Option<String> {
    let v = v.trim();
    let inner = match (v.find('<'), v.rfind('>')) {
        (Some(a), Some(b)) if b > a => &v[a + 1..b],
        _ => v,
    };
    let inner = inner.trim();
    (!inner.is_empty() && !inner.contains(char::is_whitespace)).then(|| inner.to_string())
}

/// `rfc822; Alex@Mail.example` → `alex@mail.example`.
fn address(v: &str) -> Option<String> {
    let addr = match v.split_once(';') {
        Some((kind, a)) if kind.trim().eq_ignore_ascii_case("rfc822") => a,
        Some(_) => return None,
        None => v,
    };
    let addr = addr.trim().trim_start_matches('<').trim_end_matches('>').trim();
    (addr.contains('@') && !addr.contains(char::is_whitespace)).then(|| addr.to_ascii_lowercase())
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    const GMAIL: &str = "Reporting-UA: mail.example; Penguin test\r\n\
Original-Recipient: rfc822;Alex@Harbor.example\r\n\
Final-Recipient: rfc822;alex@harbor.example\r\n\
Original-Message-ID: <CAF+abc.123@mail.example>\r\n\
Disposition: manual-action/MDN-sent-manually; displayed\r\n";

    #[test]
    fn reads_a_typical_receipt() {
        let m = parse_mdn(GMAIL).unwrap();
        assert_eq!(m.original_message_id, "CAF+abc.123@mail.example");
        assert_eq!(m.recipient.as_deref(), Some("alex@harbor.example"));
        assert_eq!(m.disposition, "displayed");
        assert_eq!(m.action_mode.as_deref(), Some("manual-action"));
        assert!(m.is_read());
    }

    #[test]
    fn folded_fields_modifiers_and_fallback_recipient() {
        let text = "Original-Recipient: rfc822; sam@mail.example\n\
Original-Message-ID:\n <x1@mail.example>\n\
Disposition: automatic-action/MDN-sent-automatically;\n\tdeleted/expired\n";
        let m = parse_mdn(text).unwrap();
        assert_eq!(m.original_message_id, "x1@mail.example");
        assert_eq!(m.recipient.as_deref(), Some("sam@mail.example"));
        assert_eq!(m.disposition, "deleted");
        assert_eq!(m.action_mode.as_deref(), Some("automatic-action"));
        assert!(!m.is_read());
    }

    #[test]
    fn needs_the_original_and_a_disposition() {
        assert_eq!(parse_mdn(""), None);
        assert_eq!(parse_mdn("Disposition: manual-action/MDN-sent-manually; displayed"), None);
        assert_eq!(parse_mdn("Original-Message-ID: <a@b.example>"), None);
        assert_eq!(parse_mdn("Original-Message-ID: <a@b.example>\nDisposition: ; <script>"), None);
        // An unknown recipient type isn't guessed at.
        let m = parse_mdn("Final-Recipient: x400; foo\nOriginal-Message-ID: a@b.example\nDisposition: displayed").unwrap();
        assert_eq!(m.recipient, None);
        assert_eq!(m.action_mode, None);
        assert_eq!(m.original_message_id, "a@b.example");
    }

    #[test]
    fn oversized_input_is_cut_on_a_char_boundary() {
        let big = format!("{GMAIL}X-Pad: {}", "é".repeat(MAX_MDN_BYTES));
        assert!(parse_mdn(&big).is_some());
    }
}
