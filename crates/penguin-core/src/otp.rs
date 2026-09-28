//! Verification-code detection ("0357 is your Uber code").
//!
//! Pure and fast: runs at ingest on the subject plus the start of the
//! authored body (quoted history is never looked at), and the result is
//! stored with the message (`messages.otp`). The rules were built from the
//! formats real services send, described generically:
//! - the code alone as the subject, with "a one-time … code" in the body;
//! - "<code> is your <service> (verification|login|sign-in) code" (subject or body);
//! - "Your (verification|security|one-time|login) code is: <code>",
//!   "Code: <code>", "One-Time Password (OTP): <code>", "(code: <code>)",
//!   "Use code <code> to …", "验证码：<code>", "Su código es: <code>";
//! - "use the code below … <code>" with the code on its own line;
//! - 4–8 digits (leading zeros kept), "NNN NNN" split in two, a letter
//!   prefix ("G-NNNNNN"), Slack-style "ABC-D3F", and 6–8 character letter +
//!   digit codes (upper case, or lower-case hex) when explicitly labeled;
//! - magic links: "Sign in to <service>" / "Your login request" with a
//!   single button and an expiry or "didn't request" line.
//!
//! What it must NOT pick up: account/member/order/invoice/tracking numbers,
//! "Account ending: NNNNN", prices, years, dates, times, phone numbers, zip
//! codes, promo/presale/coupon codes, anti-phishing "communication codes",
//! sign-in alerts without a code, replies and forwards (a code quoted back
//! by a person), and anything you sent yourself.
//!
//! Trust: `verified` is `sender_authenticated` (Gmail's DMARC / aligned DKIM
//! pass). A From-address allowlist is deliberately NOT used as trust: From
//! is exactly what a phishing mail fakes. Unverified codes are still shown
//! (muted, copy on explicit click only), never auto-copied.

use crate::text;
use crate::types::{Message, Otp, OtpKind};

/// Bytes of normalized body text scanned. Codes sit near the top: the latest
/// one observed was ~250 characters in, behind a greeting and a
/// "we received a request…" sentence.
const BODY_WINDOW: usize = 600;
const SUBJECT_WINDOW: usize = 200;

/// Detect a code or sign-in link in a stored message. Sent mail and drafts
/// never carry one (it's your own text).
pub fn detect_message(m: &Message) -> Option<Otp> {
    let (authored, _) = text::split_quoted(&m.body_text);
    detect_authored(m, &authored)
}

/// `detect_message` with the authored text already split off (ingest has it).
pub fn detect_authored(m: &Message, authored: &str) -> Option<Otp> {
    if m.label_ids.iter().any(|l| l == "SENT" || l == "DRAFT") {
        return None;
    }
    let body = if authored.trim().is_empty() {
        m.snippet.as_str()
    } else {
        authored
    };
    detect(&m.subject, body, m.sender_authenticated, m.date)
}

/// Detect from already-extracted parts. `body` should be the authored text
/// (or the snippet for headers-only mail).
pub fn detect(subject: &str, body: &str, sender_authenticated: bool, date: i64) -> Option<Otp> {
    let subj_lc = subject.trim_start().to_ascii_lowercase();
    // A person replying to or forwarding a code isn't the service sending it.
    if ["re:", "re :", "fwd:", "fw:", "aw:", "tr:", "rv:"]
        .iter()
        .any(|p| subj_lc.starts_with(p))
    {
        return None;
    }
    let subject = normalize(subject, SUBJECT_WINDOW);
    let body = normalize(body, BODY_WINDOW);
    let s_lc = subject.to_ascii_lowercase();
    let b_lc = body.to_ascii_lowercase();
    let all_lc = format!("{s_lc}\n{b_lc}");

    if !has_any(&all_lc, TRIGGERS) {
        return None;
    }
    if has_any(&all_lc, PROMO) && !has_any(&all_lc, SECURITY) {
        return None;
    }
    if has_any(&all_lc, MEETING) {
        return None;
    }
    let ctx = Ctx {
        all_lc: &all_lc,
        subject_lc: &s_lc,
    };

    let mut best: Option<(i32, String)> = None;
    for (seg, lc, in_subject) in [(&subject, &s_lc, true), (&body, &b_lc, false)] {
        for c in candidates(seg) {
            let Some(score) = score(&ctx, seg, lc, &c, in_subject, &subject) else {
                continue;
            };
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, c.code));
            }
        }
    }
    let verified = sender_authenticated;
    if let Some((_, code)) = best {
        return Some(Otp {
            kind: OtpKind::Code,
            code: Some(code),
            verified,
            date,
        });
    }
    if is_magic_link(&s_lc, &b_lc) {
        return Some(Otp {
            kind: OtpKind::Link,
            code: None,
            verified,
            date,
        });
    }
    None
}

// ---------------------------------------------------------------------------
// vocabulary (lower case)

/// At least one of these must appear somewhere for any detection to run.
const TRIGGERS: &[&str] = &[
    "code",
    "passcode",
    "verif",
    "one-time",
    "one time",
    "single-use",
    "otp",
    "2fa",
    "two-factor",
    "2-step",
    "pin",
    "sign in",
    "sign-in",
    "log in",
    "log-in",
    "login",
    "authenticat",
    "验证码",
    "código",
    "codigo",
];

/// Marketing words: with these present and no security wording, a "code" is
/// a coupon.
const PROMO: &[&str] = &[
    "% off",
    "promo",
    "coupon",
    "discount",
    "presale",
    "pre-sale",
    "voucher",
    "gift card",
    "free shipping",
    "shop now",
    "referral",
    "sale ends",
    "checkout",
    "special pricing",
];

/// Meeting invites: a "Passcode: 123456" there joins a call, it isn't a
/// sign-in code, and the row is better off showing the invite.
const MEETING: &[&str] = &[
    "meeting id",
    "join zoom",
    "zoom meeting",
    "join meeting",
    "webinar id",
    "join the meeting",
    "google meet",
    "meet.google.com",
    "teams meeting",
];

const SECURITY: &[&str] = &[
    "verification",
    "verify",
    "one-time",
    "one time",
    "single-use",
    "sign in",
    "sign-in",
    "log in",
    "login",
    "passcode",
    "security code",
    "2fa",
    "two-factor",
    "otp",
    "authenticat",
    "验证码",
];

/// Words naming the code right before it ("… code: 1234").
const LABELS: &[&str] = &[
    "code",
    "codes",
    "passcode",
    "pass code",
    "pin",
    "otp",
    "(otp)",
    "one-time password",
    "one time password",
    "single-use code",
    "验证码",
    "código",
    "codigo",
    "kode",
    "codice",
];

/// Qualifiers that make "<x> code" something other than a sign-in code.
const NEG_QUALIFIERS: &[&str] = &[
    "promo",
    "promotional",
    "coupon",
    "discount",
    "gift",
    "referral",
    "presale",
    "pre-sale",
    "invite",
    "invitation",
    "voucher",
    "zip",
    "postal",
    "post",
    "area",
    "country",
    "source",
    "qr",
    "bar",
    "dress",
    "color",
    "colour",
    "tracking",
    "product",
    "style",
    "sort",
    "swift",
    "bank",
    "communication",
    "reference",
    "booking",
    "discount",
    "offer",
    "redemption",
    "pickup",
    "access-list",
    "error",
    "status",
    "response",
    "tax",
    "class",
    "course",
    "airport",
    "reservation",
    "sale",
    "unlock",
    "ticket",
    "settings",
    "setting",
    "source-code",
    "hour",
    "sample",
    "example",
];

/// Words right before a bare number that make it an identifier or quantity.
const NEG_BEFORE: &[&str] = &[
    "order",
    "invoice",
    "inv",
    "tracking",
    "account",
    "acct",
    "ending",
    "member",
    "number",
    "no",
    "nr",
    "ref",
    "reference",
    "booking",
    "reservation",
    "client",
    "customer",
    "id",
    "zip",
    "postal",
    "suite",
    "ste",
    "apt",
    "unit",
    "ticket",
    "case",
    "phone",
    "tel",
    "fax",
    "call",
    "text",
    "card",
    "routing",
    "receipt",
    "transaction",
    "flight",
    "gate",
    "seat",
    "room",
    "ext",
    "extension",
    "box",
    "po",
    "item",
    "sku",
    "model",
    "version",
    "build",
    "issue",
    "loan",
    "policy",
    "claim",
    "patient",
    "employee",
    "student",
    "license",
    "serial",
    "year",
    "since",
    "by",
    "on",
    "at",
    "until",
    "till",
    "in",
    "of",
    "from",
    "before",
    "after",
    "total",
    "amount",
    "balance",
    "paid",
    "price",
    "usd",
    "eur",
    "street",
    "st",
    "ave",
    "avenue",
    "road",
    "rd",
    "floor",
    "pr",
    "pull",
    "commit",
    "confirmation",
];

/// Words right after a number that make it a quantity.
const UNITS: &[&str] = &[
    "minutes", "minute", "mins", "min", "hours", "hour", "hrs", "hr", "seconds", "secs", "days",
    "day", "weeks", "months", "years", "am", "pm", "usd", "eur", "gbp", "dollars", "gb", "mb",
    "kb", "px", "ft", "lbs", "kg", "miles", "km", "points", "pts", "people", "users", "members",
    "views", "messages", "emails", "items", "orders", "results", "st", "nd", "rd", "th", "street",
    "ave", "avenue",
];

/// Travel/shopping context: a "confirmation code" here is a booking or
/// order reference.
const BOOKING: &[&str] = &[
    "flight",
    "reservation",
    "booking",
    "itinerary",
    "trip",
    "hotel",
    "check-in",
    "boarding",
    "your order",
    "order #",
    "order confirmation",
    "shopping",
    "order number",
    "purchase",
    "shipped",
    "shipping",
    "receipt",
];

fn has_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

// ---------------------------------------------------------------------------
// normalization + candidates

/// Drop the invisible preheader padding (U+034F, zero-width spaces, soft
/// hyphens) that verification mails are full of, and collapse whitespace.
fn normalize(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max + 8));
    let mut space = true;
    for c in s.chars() {
        if out.len() >= max {
            break;
        }
        let c = match c {
            '\u{034F}'
            | '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2060}'..='\u{2064}'
            | '\u{FEFF}' => continue,
            c if c.is_whitespace() => ' ',
            c => c,
        };
        if c == ' ' {
            if space {
                continue;
            }
            space = true;
        } else {
            space = false;
        }
        out.push(c);
    }
    let end = out.trim_end().len();
    out.truncate(end);
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    /// 4–8 digits, or "123-456".
    Digits,
    /// "G-123456", "ABC-D3F", "abcd-123456".
    Hyphen,
    /// 6–8 letters and digits mixed ("06NF5T", "1ff10e63").
    Mixed,
    /// Only with an explicit "code is X" / "code: X": all-caps letters
    /// ("QWEX"), mixed-case ("DtUy7k"), word codes ("amble-tovi-ranok"),
    /// 9–10 digit recovery codes.
    Loose,
}

#[derive(Debug)]
struct Cand {
    start: usize,
    end: usize,
    shape: Shape,
    /// What gets copied (the "NNN NNN" split form is joined).
    code: String,
}

fn is_run_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

fn candidates(s: &str) -> Vec<Cand> {
    let bytes = s.as_bytes();
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !is_run_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && is_run_byte(bytes[j]) {
            j += 1;
        }
        let (mut a, mut b) = (i, j);
        while a < b && bytes[a] == b'-' {
            a += 1;
        }
        while b > a && bytes[b - 1] == b'-' {
            b -= 1;
        }
        if a < b {
            runs.push((a, b));
        }
        i = j;
    }
    let mut out = Vec::new();
    let mut k = 0;
    while k < runs.len() {
        let (a, b) = runs[k];
        let run = &s[a..b];
        // "183 449": two 3-digit halves split by one space.
        if b - a == 3 && run.bytes().all(|c| c.is_ascii_digit()) {
            if let Some(&(c, d)) = runs.get(k + 1) {
                if c == b + 1
                    && bytes[b] == b' '
                    && d - c == 3
                    && s[c..d].bytes().all(|x| x.is_ascii_digit())
                    && !digit_near(s, a, d)
                {
                    out.push(Cand {
                        start: a,
                        end: d,
                        shape: Shape::Digits,
                        code: format!("{run}{}", &s[c..d]),
                    });
                    k += 2;
                    continue;
                }
            }
        }
        if let Some(shape) = shape_of(run) {
            out.push(Cand {
                start: a,
                end: b,
                shape,
                code: run.to_string(),
            });
        }
        k += 1;
    }
    out
}

/// A digit right outside [a, b) ignoring one space: the split pair is part
/// of a longer number (phone, card).
fn digit_near(s: &str, a: usize, b: usize) -> bool {
    let before = s[..a].trim_end_matches(' ');
    let after = s[b..].trim_start_matches(' ');
    before.ends_with(|c: char| c.is_ascii_digit())
        || after.starts_with(|c: char| c.is_ascii_digit())
}

fn shape_of(run: &str) -> Option<Shape> {
    let n = run.len();
    let digits = run.bytes().filter(u8::is_ascii_digit).count();
    let letters = run.bytes().filter(u8::is_ascii_alphabetic).count();
    let upper = run
        .bytes()
        .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase());
    let lower = run
        .bytes()
        .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase());
    if !run.contains('-') {
        if digits == n {
            return match n {
                4..=8 => Some(Shape::Digits),
                9..=10 => Some(Shape::Loose),
                _ => None,
            };
        }
        if (6..=8).contains(&n) && digits > 0 && letters > 0 {
            // Mixed case with digits ("iPhone15") only when labeled.
            return Some(if upper || lower {
                Shape::Mixed
            } else {
                Shape::Loose
            });
        }
        if digits == 0 && upper && (4..=8).contains(&n) && !CAPS_WORDS.contains(&run) {
            return Some(Shape::Loose);
        }
        return None;
    }
    let parts: Vec<&str> = run.split('-').collect();
    let all_digits = |x: &str| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit());
    let all_lower = |x: &str| x.bytes().all(|c| c.is_ascii_lowercase());
    // "amble-tovi-ranok-sute"
    if parts.len() >= 3
        && parts
            .iter()
            .all(|p| (3..=6).contains(&p.len()) && all_lower(p))
    {
        return Some(Shape::Loose);
    }
    if parts.len() != 2 {
        return None;
    }
    let (p, q) = (parts[0], parts[1]);
    let upper_alnum = |x: &str| {
        x.bytes()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    };
    // "G-123456", "abcd-123456"
    if (1..=4).contains(&p.len())
        && p.bytes().all(|c| c.is_ascii_alphabetic())
        && all_digits(q)
        && (4..=8).contains(&q.len())
    {
        return Some(Shape::Hyphen);
    }
    // "123-456" (not a phone "555-0123")
    if all_digits(p) && all_digits(q) {
        return (p.len() == 3 && q.len() == 3).then_some(Shape::Digits);
    }
    // "ABC-D3F", "DBA-744"
    if (3..=4).contains(&p.len()) && (3..=4).contains(&q.len()) && upper_alnum(p) && upper_alnum(q)
    {
        return Some(Shape::Hyphen);
    }
    None
}

/// All-caps words that can follow "CODE IS" in shouted mail.
const CAPS_WORDS: &[&str] = &[
    "BELOW", "ABOVE", "HERE", "VALID", "INVALID", "EXPIRED", "READY", "SENT", "CORRECT", "WRONG",
    "REQUIRED", "NEEDED", "ACTIVE", "USED", "ONLY", "YOUR", "THIS", "CODE", "THE", "FREE", "NONE",
    "NULL", "PENDING",
];

// ---------------------------------------------------------------------------
// scoring

struct Ctx<'a> {
    all_lc: &'a str,
    subject_lc: &'a str,
}

/// Subject words that say "this mail is a code".
const SUBJECT_LABELS: &[&str] = &["code", "pin", "passcode", "otp", "验证码", "código"];

fn score(ctx: &Ctx, seg: &str, lc: &str, c: &Cand, in_subject: bool, subject: &str) -> Option<i32> {
    if !clean_boundary(seg, c.start, c.end) {
        return None;
    }
    let before = &lc[..c.start];
    let after = &lc[c.end..];
    let bonus = if in_subject { 5 } else { 0 };
    let security = has_any(ctx.all_lc, SECURITY);
    let colon = before.trim_end().ends_with([':', '：']);
    // Distance from the nearest code phrase ("use the code below … 123456").
    let near = NEAR
        .iter()
        .filter_map(|t| rfind_word(before, t).map(|p| c.start - (p + t.len())))
        .min();
    // "…your Serverly account: 9ac03f71", "…to activate your account: 123456"
    // (a bare "Account: 12345678" stays an account number).
    // "Account ending: 02008" stays out.
    let colon_ok = colon
        && (c.shape == Shape::Mixed
            || near.is_some_and(|n| n <= 100)
                && !matches!(
                    last_word(before),
                    "ending"
                        | "number"
                        | "no"
                        | "nr"
                        | "id"
                        | "ref"
                        | "reference"
                        | "phone"
                        | "tel"
                        | "fax"
                        | "zip"
                        | "order"
                        | "invoice"
                        | "tracking"
                ));
    // A label alone ("… code 3f2a9c1" in a commit list, "PIN: 1234" in a
    // hotel booking) needs sign-in wording in the mail or a code subject.
    let label_ok = (security || has_any(ctx.subject_lc, SUBJECT_LABELS))
        && (in_subject
            || !has_any(ctx.all_lc, BOOKING)
            || has_any(
                ctx.all_lc,
                &["verif", "one-time", "sign in", "log in", "login"],
            ));
    let label = match labeled_before(ctx, before) {
        Some(l) => Some(l),
        // "Account ending: 02008 · Your card PIN has been set"
        None if neg_before(before) && !colon_ok => return None,
        None => labeled_after(ctx, after).map(|ok| Label {
            ok,
            explicit: false,
        }),
    };
    match label {
        // Loose shapes and year-like numbers ("Hour of Code 2025") need
        // "code is X" / "code: X".
        Some(Label { ok: true, explicit })
            if label_ok && (explicit || c.shape != Shape::Loose && !is_year(&c.code)) =>
        {
            return Some(100 + bonus)
        }
        // "zip code 10001", "error code 5003", "promo code X".
        Some(_) => return None,
        None => {}
    }
    if c.shape == Shape::Loose
        || is_year(&c.code)
        || unit_after(after)
        || (neg_before(before) && !colon_ok)
    {
        return None;
    }
    // The subject is the code, or carries it next to login wording
    // ("0647", "Game ID Login [183 449]").
    if in_subject {
        // Digits only: "[FOR-123]" ticket keys sit in subjects too.
        if c.shape != Shape::Digits || !security {
            return None;
        }
        let bare = subject
            .trim()
            .trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
        if bare == &seg[c.start..c.end] {
            return Some(70);
        }
        return has_any(
            lc,
            &[
                "code", "login", "log in", "sign in", "sign-in", "passcode", "otp",
            ],
        )
        .then_some(50);
    }
    if !security {
        return None;
    }
    let near = near?;
    match c.shape {
        Shape::Digits if near <= 200 => Some(40 - (near as i32) / 20),
        // Letter codes need a tight "…: CODE" after the phrase.
        Shape::Mixed | Shape::Hyphen if near <= 80 && colon => Some(35),
        _ => None,
    }
}

struct Label {
    /// A sign-in code label, not "zip code".
    ok: bool,
    /// "code is X" / "code: X", not just "code X".
    explicit: bool,
}

/// Phrases a bare body code follows ("use the code below … 123456").
const NEAR: &[&str] = &[
    "verification code",
    "security code",
    "login code",
    "log in code",
    "sign-in code",
    "sign in code",
    "access code",
    "authentication code",
    "confirmation code",
    "one-time code",
    "one time code",
    "single-use code",
    "passcode",
    "otp",
    "one-time password",
    "code below",
    "following code",
    "this code",
    "the code",
    "your code",
    "code shown",
    "code above",
    "验证码",
    "código",
];

/// Last occurrence of `needle` as whole words (not "shipping" for "pin").
fn rfind_word(hay: &str, needle: &str) -> Option<usize> {
    let mut end = hay.len();
    while let Some(p) = hay[..end].rfind(needle) {
        let before_ok = !hay[..p].ends_with(|c: char| c.is_alphanumeric());
        let after_ok = !hay[p + needle.len()..].starts_with(|c: char| c.is_alphanumeric());
        if before_ok && after_ok || !needle.is_ascii() {
            return Some(p);
        }
        end = p;
    }
    None
}

/// Reject numbers glued to currency, dates, times, decimals, URLs, emails.
fn clean_boundary(s: &str, a: usize, b: usize) -> bool {
    let prev = s[..a].chars().next_back();
    let prev2 = s[..a].chars().rev().nth(1);
    let next = s[b..].chars().next();
    let next2 = s[b..].chars().nth(1);
    if let Some(p) = prev {
        if "$€£¥₹#+@/\\&=?_~%".contains(p) {
            return false;
        }
        // "10:45", "1,250", "v2.1" (but "OTP:123456" is fine).
        if (":,".contains(p) && prev2.is_some_and(|q| q.is_ascii_digit()))
            || (p == '.' && prev2.is_some_and(|q| q.is_ascii_alphanumeric()))
        {
            return false;
        }
    }
    // "# 12345", "$ 1234", "+1 954…"
    let prev_word = s[..a].trim_end_matches(' ');
    if prev_word.len() < a && prev_word.ends_with(['#', '$', '€', '£', '+']) {
        return false;
    }
    if let Some(n) = next {
        if "%@/\\_=&".contains(n) {
            return false;
        }
        if ".,:".contains(n) && next2.is_some_and(|q| q.is_ascii_alphanumeric()) {
            return false;
        }
    }
    if s[b..].trim_start_matches(' ').starts_with('%') {
        return false;
    }
    // Inside a URL or address.
    let ws = s[..a].rfind(' ').map_or(0, |i| i + 1);
    let we = s[b..].find(' ').map_or(s.len(), |i| b + i);
    let word = &s[ws..we];
    !(word.contains("://") || word.contains("www.") || word.contains('@'))
}

fn is_year(code: &str) -> bool {
    code.len() == 4
        && code
            .parse::<u32>()
            .is_ok_and(|y| (1900..=2099).contains(&y))
}

/// The last `n` words (letters/digits/hyphens), nearest first.
fn last_words(s: &str, n: usize) -> Vec<&str> {
    s.rsplit(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|w| !w.is_empty())
        .take(n)
        .collect()
}

fn last_word(s: &str) -> &str {
    last_words(s, 1).first().copied().unwrap_or("")
}

fn neg_before(before: &str) -> bool {
    // "…to your account. 123456 This code…": a new sentence.
    if before.trim_end().ends_with(['.', '!', '?']) {
        return false;
    }
    let w = last_words(before, 2);
    match w.as_slice() {
        [w1, rest @ ..] => {
            NEG_BEFORE.contains(w1)
                // "Account ID 12345", "Order No. 12345"
                || (matches!(*w1, "id" | "no" | "num" | "number" | "ending" | "code")
                    && rest.first().is_some_and(|w2| NEG_BEFORE_PAIR.contains(w2)))
        }
        [] => false,
    }
}

/// An identifier noun two words back.
const NEG_BEFORE_PAIR: &[&str] = &[
    "account",
    "order",
    "member",
    "customer",
    "client",
    "tracking",
    "invoice",
    "booking",
    "reservation",
    "confirmation",
];

fn unit_after(after: &str) -> bool {
    let a = after.trim_start_matches(' ');
    let w: String = a.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    UNITS.contains(&w.as_str())
}

/// "…code: X", "…code is X", "(OTP): X", "验证码：X", "Use code X".
/// The label right before a candidate, if any.
fn labeled_before(ctx: &Ctx, before: &str) -> Option<Label> {
    let trim = |s: &str| -> usize {
        s.trim_end_matches(|c: char| {
            matches!(
                c,
                ' ' | ':' | '：' | '=' | '-' | '—' | '–' | '"' | '\'' | '“' | '(' | '[' | '*'
            )
        })
        .len()
    };
    let mut end = trim(before);
    let mut explicit = before[end..].contains([':', '：', '=']);
    for _ in 0..2 {
        let b = &before[..end];
        let stripped = ["is", "es", "was", "below", "here", "now", "are"]
            .iter()
            .find_map(|w| {
                b.strip_suffix(w)
                    .filter(|r| r.ends_with(' '))
                    .map(|r| (*w, r))
            });
        match stripped {
            Some((w, r)) => {
                explicit |= matches!(w, "is" | "es" | "was" | "are");
                end = trim(r);
                explicit |= before[end..].contains([':', '：', '=']);
            }
            None => break,
        }
    }
    let b = &before[..end];
    // Only the label directly at the end counts.
    for label in LABELS {
        let Some(head) = b.strip_suffix(label) else {
            continue;
        };
        // Whole word: not "barcode", "zipcode".
        if head.ends_with(|c: char| c.is_alphanumeric()) && label.is_ascii() {
            continue;
        }
        return Some(Label {
            ok: qualifier_ok(ctx, head),
            explicit,
        });
    }
    None
}

/// "X is your … code", "X - Your … login code", "X es tu código".
fn labeled_after(ctx: &Ctx, after: &str) -> Option<bool> {
    let a = after.trim_start_matches([' ', ')', ']']);
    let dashed = a.starts_with(['-', '—', '–', ':']);
    let a = a.trim_start_matches([' ', '-', '—', '–', ':']);
    // "DELTA 123456 Use this code to verify…"
    if [
        "use this code",
        "use the code",
        "use the above code",
        "enter this code",
        "is your code",
    ]
    .iter()
    .any(|p| a.starts_with(p))
    {
        return Some(true);
    }
    let rest = [
        "is your ",
        "is the ",
        "is ur ",
        "es tu ",
        "es su ",
        "est votre ",
    ]
    .iter()
    .find_map(|p| a.strip_prefix(p))
    // "337046 - Your Spotify login code"
    .or_else(|| a.strip_prefix("your ").filter(|_| dashed))?;
    // Up to the end of the sentence, within a few words.
    let stop = rest
        .find(['.', '!', '?', '\n', ','])
        .unwrap_or(rest.len())
        .min(70);
    let phrase = &rest[..floor_char(rest, stop)];
    for label in LABELS {
        let mut from = 0;
        while let Some(p) = phrase[from..].find(label) {
            let at = from + p;
            let end = at + label.len();
            let whole_before = at == 0 || !phrase[..at].ends_with(|c: char| c.is_alphanumeric());
            let whole_after = !phrase[end..].starts_with(|c: char| c.is_alphanumeric());
            if whole_before && whole_after {
                return Some(qualifier_ok(ctx, &phrase[..at]));
            }
            from = end;
        }
    }
    None
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The word before the label must not make it a coupon/zip/booking code.
fn qualifier_ok(ctx: &Ctx, head: &str) -> bool {
    let q = last_word(head);
    if NEG_QUALIFIERS.contains(&q) {
        return false;
    }
    if q == "confirmation" {
        // Slack-style "confirm your email" codes, not booking or order refs.
        return !has_any(ctx.all_lc, BOOKING)
            && (has_any(ctx.all_lc, SECURITY)
                || ctx.all_lc.contains("confirm your")
                || ctx.subject_lc.contains("confirmation code"));
    }
    true
}

/// "Sign in to X" / "Your login request" mails with a button, no code.
fn is_magic_link(subj: &str, body: &str) -> bool {
    const SUBJECT: &[&str] = &[
        "sign in to",
        "sign-in link",
        "sign in link",
        "log in to",
        "login link",
        "log-in link",
        "magic link",
        "login request",
        "log in request",
        "sign-in request",
        "secure link",
        "password-free link",
        "passwordless",
        "sign in with this link",
        "your login link",
    ];
    // Security alerts about a sign-in that already happened.
    const ALERT: &[&str] = &[
        "new sign",
        "new login",
        "new device",
        "detected",
        "alert",
        "unusual",
        "attempt",
        "was this you",
        "recent",
        "suspicious",
        "successful",
        "from a",
        "activity",
    ];
    const BODY: &[&str] = &[
        "didn't request",
        "did not request",
        "didn’t request",
        "expire",
        "request to sign in",
        "request to log in",
        "only be used once",
        "one-time link",
        "password-free",
        "can only be used",
    ];
    has_any(subj, SUBJECT)
        && !has_any(subj, ALERT)
        && has_any(body, BODY)
        && has_any(
            body,
            &["sign in", "log in", "login", "sign-in", "link", "button"],
        )
}

#[cfg(test)]
#[path = "otp_tests.rs"]
mod tests;
