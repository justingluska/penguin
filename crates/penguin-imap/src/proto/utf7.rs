//! Modified UTF-7 mailbox names (RFC 3501 §5.1.3): `Entw&APw-rfe` ⇄
//! `Entwürfe`. Gmail's X-GM-LABELS values use the same encoding.

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

fn b64_value(c: u8) -> Option<u32> {
    B64.iter().position(|&x| x == c).map(|i| i as u32)
}

/// Encode a mailbox name for the wire.
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
        out.push('&');
        let mut bits: u32 = 0;
        let mut nbits = 0;
        for b in bytes {
            bits = (bits << 8) | b as u32;
            nbits += 8;
            while nbits >= 6 {
                nbits -= 6;
                out.push(B64[((bits >> nbits) & 0x3f) as usize] as char);
            }
        }
        if nbits > 0 {
            out.push(B64[((bits << (6 - nbits)) & 0x3f) as usize] as char);
        }
        out.push('-');
        pending.clear();
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

/// Decode a mailbox name from the wire. Malformed sequences are kept as
/// they are (a name is never lost, just shown raw).
pub fn decode(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    // Servers with UTF8=ACCEPT send UTF-8, which has no '&' sequences to undo.
    let mut out = String::with_capacity(s.len());
    let mut rest: &str = &s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let chunk = &after[..end];
        if chunk.is_empty() {
            out.push('&');
        } else {
            match decode_chunk(chunk) {
                Some(text) => out.push_str(&text),
                None => {
                    out.push('&');
                    out.push_str(chunk);
                    out.push('-');
                }
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_chunk(chunk: &str) -> Option<String> {
    let mut bits: u32 = 0;
    let mut nbits = 0;
    let mut bytes = Vec::new();
    for c in chunk.bytes() {
        bits = (bits << 6) | b64_value(c)?;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            bytes.push(((bits >> nbits) & 0xff) as u8);
        }
    }
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|p| u16::from_be_bytes([p[0], p[1]]))
        .collect();
    String::from_utf16(&units).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names() {
        for (plain, wire) in [
            ("INBOX", "INBOX"),
            ("Entwürfe", "Entw&APw-rfe"),
            ("Tom & Jerry", "Tom &- Jerry"),
            ("日本語", "&ZeVnLIqe-"),
            ("[Gmail]/Gesendet", "[Gmail]/Gesendet"),
            ("~peter/mail/台北/日本語", "~peter/mail/&U,BTFw-/&ZeVnLIqe-"),
            ("🐧", "&2D3cJw-"),
        ] {
            assert_eq!(encode(plain), wire, "{plain}");
            assert_eq!(decode(wire.as_bytes()), plain, "{wire}");
        }
    }

    #[test]
    fn broken_input_is_kept() {
        assert_eq!(decode(b"a&b"), "a&b");
        assert_eq!(decode(b"a&!!-b"), "a&!!-b");
    }
}
