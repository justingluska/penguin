//! Parcel tracking numbers, recognized by shape and checked with each
//! carrier's check digit, so order numbers, phone numbers and account
//! numbers don't pass as tracking numbers.
//!
//! - UPS: "1Z" + 15 characters + a mod-10 check digit (letters map to
//!   digits as (A=2 … Z=7), odd positions weight 1, even positions weight 2).
//! - USPS Intelligent Mail package barcode: 20 or 22 digits starting 91–95,
//!   mod-10 check digit with weights 3 and 1 from the right (USPS Pub. 199).
//! - UPU S10 (international mail, "RR123456785US"): two letters, eight
//!   digits, a mod-11 check digit with weights 8 6 4 2 3 5 9 7, and a
//!   two-letter country code.
//! - FedEx Express 12 digits: weights 1 3 7 from the right, sum mod 11 mod 10.
//! - DHL Express 10 digits: the first nine as a number, mod 7.
//! - Amazon Logistics "TBA" + 12 digits (no check digit).
//!
//! FedEx and DHL numbers are plain digit runs, so they also need the
//! carrier named nearby. A number printed right after "Tracking number" is
//! accepted for any carrier, unverified.

use super::Shipment;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tracking {
    pub carrier: &'static str,
    pub number: String,
    pub verified: bool,
}

/// A carrier's public tracking page for `number`.
pub(crate) fn tracking_url(carrier: &str, number: &str) -> Option<String> {
    let base = match carrier {
        "UPS" => "https://www.ups.com/track?tracknum=",
        "USPS" => "https://tools.usps.com/go/TrackConfirmAction?tLabels=",
        "FedEx" => "https://www.fedex.com/fedextrack/?trknbr=",
        "DHL" => "https://www.dhl.com/global-en/home/tracking.html?tracking-id=",
        _ => return None,
    };
    Some(format!("{base}{number}"))
}

fn ups_value(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'A'..=b'Z' => Some(((c - b'A') as u32 + 2) % 10),
        _ => None,
    }
}

pub(crate) fn ups_valid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 18 || &b[..2] != b"1Z" {
        return false;
    }
    let mut sum = 0;
    for (i, &c) in b[2..17].iter().enumerate() {
        let Some(v) = ups_value(c) else { return false };
        sum += if i % 2 == 1 { v * 2 } else { v };
    }
    let Some(check) = ups_value(b[17]) else {
        return false;
    };
    b[17].is_ascii_digit() && (10 - sum % 10) % 10 == check
}

/// Mod 10, weights 3 and 1 starting with 3 at the digit left of the check.
pub(crate) fn mod10_31_valid(digits: &str) -> bool {
    let b = digits.as_bytes();
    if b.len() < 2 || !b.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let (data, check) = b.split_at(b.len() - 1);
    let sum: u32 = data
        .iter()
        .rev()
        .enumerate()
        .map(|(i, c)| (c - b'0') as u32 * if i % 2 == 0 { 3 } else { 1 })
        .sum();
    (10 - sum % 10) % 10 == (check[0] - b'0') as u32
}

pub(crate) fn usps_valid(s: &str) -> bool {
    (s.len() == 20 || s.len() == 22)
        && s.starts_with('9')
        && matches!(s.as_bytes()[1], b'1'..=b'5')
        && mod10_31_valid(s)
}

pub(crate) fn s10_valid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 13
        || !b[..2].iter().all(u8::is_ascii_uppercase)
        || !b[11..].iter().all(u8::is_ascii_uppercase)
        || !b[2..11].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    const W: [u32; 8] = [8, 6, 4, 2, 3, 5, 9, 7];
    let sum: u32 = b[2..10]
        .iter()
        .zip(W)
        .map(|(c, w)| (c - b'0') as u32 * w)
        .sum();
    let check = match 11 - sum % 11 {
        10 => 0,
        11 => 5,
        n => n,
    };
    (b[10] - b'0') as u32 == check
}

pub(crate) fn fedex12_valid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 12 || !b.iter().all(u8::is_ascii_digit) {
        return false;
    }
    const W: [u32; 3] = [1, 3, 7];
    let sum: u32 = b[..11]
        .iter()
        .rev()
        .enumerate()
        .map(|(i, c)| (c - b'0') as u32 * W[i % 3])
        .sum();
    sum % 11 % 10 == (b[11] - b'0') as u32
}

pub(crate) fn dhl10_valid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || !b.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let n: u64 = s[..9].parse().unwrap_or(0);
    (n % 7) as u8 == b[9] - b'0'
}

/// Candidate tokens: runs of letters and digits, with single spaces
/// inside digit groups joined ("9400 1000 0000 ..." is one number).
fn candidates(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_alphanumeric() || (i > 0 && b[i - 1].is_ascii_alphanumeric()) {
            i += 1;
            continue;
        }
        let start = i;
        let mut tok = String::new();
        let mut j = i;
        while j < b.len() {
            if b[j].is_ascii_alphanumeric() {
                tok.push(b[j].to_ascii_uppercase() as char);
                j += 1;
            } else if b[j] == b' '
                && j + 1 < b.len()
                && b[j + 1].is_ascii_digit()
                && tok.bytes().last().is_some_and(|c| c.is_ascii_digit())
                && tok.len() >= 4
            {
                // "9400 1000 ..." groups of 4: join only digit groups.
                j += 1;
            } else {
                break;
            }
        }
        if tok.len() >= 10 {
            out.push((start, tok));
        }
        i = j.max(i + 1);
    }
    out
}

/// Every tracking number in `text`. `lower` is `text` lowercased (for
/// carrier cues).
#[cfg(test)]
pub(crate) fn find(text: &str, lower: &str) -> Vec<Tracking> {
    find_with_cues(text, lower, lower)
}

/// [`find`] with carrier names looked for in `cues` (sender, subject and
/// text, lowercase).
pub(crate) fn find_with_cues(text: &str, lower: &str, cues: &str) -> Vec<Tracking> {
    let fedex = cues.contains("fedex");
    let dhl = cues.contains("dhl");
    let amazon = cues.contains("amazon");
    let ups = contains_word(cues, "ups");
    let usps = cues.contains("usps") || cues.contains("postal service");
    let mut out: Vec<Tracking> = Vec::new();
    let mut push = |t: Tracking| {
        if !out.iter().any(|o| o.number == t.number) {
            out.push(t);
        }
    };
    for (_, tok) in candidates(text) {
        let t = if ups_valid(&tok) {
            Some(("UPS", true))
        } else if usps_valid(&tok) {
            Some(("USPS", true))
        } else if s10_valid(&tok) {
            Some((
                if tok.ends_with("US") {
                    "USPS"
                } else {
                    "Postal service"
                },
                true,
            ))
        } else if fedex && fedex12_valid(&tok) {
            Some(("FedEx", true))
        } else if dhl && dhl10_valid(&tok) {
            Some(("DHL", true))
        } else if amazon
            && tok.len() == 15
            && tok.starts_with("TBA")
            && tok[3..].bytes().all(|c| c.is_ascii_digit())
        {
            Some(("Amazon", false))
        } else {
            None
        };
        if let Some((carrier, verified)) = t {
            push(Tracking {
                carrier,
                number: tok,
                verified,
            });
        }
    }
    // "Tracking number: X" for carriers without a check we know.
    for label in [
        "tracking number",
        "tracking #",
        "tracking no",
        "tracking id",
        "número de seguimiento",
        "numero de seguimiento",
        "número de rastreo",
        "seguimiento",
        "guía",
    ] {
        let mut from = 0;
        while let Some(p) = lower[from..].find(label) {
            let s = from + p + label.len();
            from = s;
            let rest = &text[s..];
            let rest = rest.trim_start_matches([':', '#', '.', ' ', '\t', '\n', '\r']);
            let tok: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            let tok = tok.trim_matches('-').to_uppercase();
            if (8..=34).contains(&tok.len()) && tok.bytes().any(|c| c.is_ascii_digit()) {
                let carrier = if fedex {
                    "FedEx"
                } else if dhl {
                    "DHL"
                } else if ups {
                    "UPS"
                } else if usps {
                    "USPS"
                } else if amazon {
                    "Amazon"
                } else {
                    "Carrier"
                };
                push(Tracking {
                    carrier,
                    number: tok,
                    verified: false,
                });
            }
        }
    }
    out
}

fn contains_word(hay: &str, w: &str) -> bool {
    hay.match_indices(w).any(|(i, _)| {
        let before = hay[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after = hay[i + w.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

impl Shipment {
    pub(crate) fn from_tracking(t: &Tracking) -> Shipment {
        Shipment {
            carrier: Some(t.carrier.to_string()),
            tracking_number: Some(t.number.clone()),
            verified: t.verified,
            tracking_url: tracking_url(t.carrier, &t.number),
            ..Shipment::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_digits() {
        // UPS: worked example (15 data characters, R = 9).
        assert!(ups_valid("1Z5R89390357567127"));
        assert!(!ups_valid("1Z5R89390357567128"));
        // UPU S10 reference example.
        assert!(s10_valid("RR123456785US"));
        assert!(!s10_valid("RR123456784US"));
        // FedEx Express 12-digit.
        assert!(fedex12_valid("986578788855"));
        assert!(!fedex12_valid("986578788856"));
        // DHL Express 10-digit.
        assert!(dhl10_valid("3318810025"));
        assert!(!dhl10_valid("3318810024"));
        // USPS IMpb: build a valid one from the rule, then break it.
        let data = "940011189922319742849";
        let sum: u32 = data
            .bytes()
            .rev()
            .enumerate()
            .map(|(i, c)| (c - b'0') as u32 * if i % 2 == 0 { 3 } else { 1 })
            .sum();
        let n = format!("{data}{}", (10 - sum % 10) % 10);
        assert!(usps_valid(&n));
        let bad = format!("{data}{}", (10 - sum % 10 + 1) % 10);
        assert!(!usps_valid(&bad));
    }

    #[test]
    fn finds_numbers_in_text() {
        let t = "Your package is on its way!\nUPS tracking: 1Z5R89390357567127\nOrder #112-4455";
        let f = find(t, &t.to_lowercase());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].carrier, "UPS");
        assert!(f[0].verified);

        // Grouped USPS digits; a phone and an order number don't count.
        let data = "9400111899223197428490";
        let spaced = format!(
            "{} {} {} {} {} {}",
            &data[..4],
            &data[4..8],
            &data[8..12],
            &data[12..16],
            &data[16..20],
            &data[20..]
        );
        let t = format!("USPS tracking {spaced}\nCall 415-555-0199. Order 1234567890123");
        let f = find(&t, &t.to_lowercase());
        assert!(f.iter().all(|x| x.carrier == "USPS"), "{f:?}");

        // FedEx digits need FedEx named.
        let t = "Shipped with FedEx: 986578788855";
        assert_eq!(find(t, &t.to_lowercase())[0].carrier, "FedEx");
        let t = "Reference 986578788855";
        assert!(find(t, &t.to_lowercase()).is_empty());

        // Labeled numbers for other carriers, unverified.
        let t = "Tracking number: LX9981234AB";
        let f = find(t, &t.to_lowercase());
        assert_eq!(f[0].number, "LX9981234AB");
        assert!(!f[0].verified);
    }
}
