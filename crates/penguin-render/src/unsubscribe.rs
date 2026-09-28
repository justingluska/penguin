//! Finds the unsubscribe link in a rendered (already sanitized) message
//! body, for mail without a usable List-Unsubscribe header.
//!
//! Input is our own `render_html` / `render_text` output, so it is
//! well-formed, serialized by html5ever (double-quoted attributes, `&amp;`
//! style escapes) and every link is http(s) or mailto. A small scanner is
//! enough: it walks tags and text once, skipping `<style>`.
//!
//! A link counts when (best first):
//! 1. its own text has a strong phrase ("Unsubscribe", "Opt out", "配信停止");
//! 2. its own text has a weak phrase ("Manage preferences");
//! 3. its text is short ("click here") and the text just around it in the
//!    same block has a strong phrase ("To stop receiving these emails,
//!    <a>click here</a>").
//!
//! Within a rank the last link wins: the footer. Only http(s) links are
//! returned; mailto, javascript:, data: and anything else never are.

/// How strongly a phrase means "unsubscribe".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    /// Unambiguous: link text or nearby text.
    Strong,
    /// Preference pages: only as the link's own text.
    Weak,
}

/// Unsubscribe phrases, lowercase, with hyphens as spaces ("opt out" also
/// matches "Opt-Out"). Latin/Cyrillic phrases must stand as whole words;
/// Chinese, Japanese and Korean ones match anywhere.
pub const UNSUBSCRIBE_PHRASES: &[(&str, Strength)] = &[
    // English
    ("unsubscribe", Strength::Strong),
    ("opt out", Strength::Strong),
    ("optout", Strength::Strong),
    ("stop receiving", Strength::Strong),
    ("remove me", Strength::Strong),
    ("manage preferences", Strength::Weak),
    ("manage your preferences", Strength::Weak),
    ("email preferences", Strength::Weak),
    ("update your preferences", Strength::Weak),
    ("update preferences", Strength::Weak),
    ("preference center", Strength::Weak),
    ("subscription preferences", Strength::Weak),
    ("subscription settings", Strength::Weak),
    ("manage subscription", Strength::Weak),
    ("manage your subscription", Strength::Weak),
    ("notification settings", Strength::Weak),
    ("email settings", Strength::Weak),
    // Spanish
    ("darse de baja", Strength::Strong),
    ("darte de baja", Strength::Strong),
    ("dar de baja", Strength::Strong),
    ("date de baja", Strength::Strong),
    ("cancelar suscripción", Strength::Strong),
    ("cancelar la suscripción", Strength::Strong),
    ("anular suscripción", Strength::Strong),
    ("preferencias de correo", Strength::Weak),
    // French
    ("se désabonner", Strength::Strong),
    ("désabonner", Strength::Strong),
    ("désabonnez", Strength::Strong),
    ("desabonner", Strength::Strong),
    ("se désinscrire", Strength::Strong),
    ("désinscrire", Strength::Strong),
    ("désinscrivez", Strength::Strong),
    ("désinscription", Strength::Strong),
    ("gérer vos préférences", Strength::Weak),
    // German
    ("abmelden", Strength::Strong),
    ("abbestellen", Strength::Strong),
    ("abmeldung", Strength::Strong),
    ("newsletter abbestellen", Strength::Strong),
    // Italian
    ("annulla iscrizione", Strength::Strong),
    ("annulla l'iscrizione", Strength::Strong),
    ("disiscriviti", Strength::Strong),
    ("cancella iscrizione", Strength::Strong),
    // Portuguese
    ("cancelar inscrição", Strength::Strong),
    ("cancelar a inscrição", Strength::Strong),
    ("descadastrar", Strength::Strong),
    ("cancelar assinatura", Strength::Strong),
    // Dutch, Scandinavian, Polish, Russian, Turkish
    ("afmelden", Strength::Strong),
    ("uitschrijven", Strength::Strong),
    ("avregistrera", Strength::Strong),
    ("avsluta prenumeration", Strength::Strong),
    ("afmeld", Strength::Strong),
    ("wypisz się", Strength::Strong),
    ("отписаться", Strength::Strong),
    ("отказаться от рассылки", Strength::Strong),
    ("abonelikten çık", Strength::Strong),
    // Chinese, Japanese, Korean
    ("退订", Strength::Strong),
    ("退訂", Strength::Strong),
    ("取消订阅", Strength::Strong),
    ("取消訂閱", Strength::Strong),
    ("配信停止", Strength::Strong),
    ("配信解除", Strength::Strong),
    ("購読解除", Strength::Strong),
    ("수신거부", Strength::Strong),
    ("수신 거부", Strength::Strong),
    ("구독 취소", Strength::Strong),
    ("구독취소", Strength::Strong),
];

/// Link text longer than this many words isn't "click here"-style, so
/// text around it doesn't count.
const SHORT_LINK_WORDS: usize = 5;
/// How much text before / after a link counts as "around" it.
const NEAR_CHARS: usize = 100;

/// The body's unsubscribe link (http or https), if any. See module docs.
pub fn find_unsubscribe_link(rendered: &str) -> Option<String> {
    let mut best: Option<(u8, String)> = None;
    for a in anchors(rendered) {
        if !is_web_url(&a.href) {
            continue;
        }
        let text = normalize(&a.text);
        let rank = if has_phrase(&text, Strength::Strong) {
            0
        } else if has_phrase(&text, Strength::Weak) {
            1
        } else if text.split(' ').filter(|w| !w.is_empty()).count() <= SHORT_LINK_WORDS
            && (has_phrase(&normalize(&a.before), Strength::Strong)
                || has_phrase(&normalize(&a.after), Strength::Strong))
        {
            2
        } else {
            continue;
        };
        // `<=`: a later link of the same rank wins (nearest the footer).
        if best.as_ref().is_none_or(|(r, _)| rank <= *r) {
            best = Some((rank, a.href));
        }
    }
    best.map(|(_, href)| href)
}

fn is_web_url(href: &str) -> bool {
    let lower = href.trim().to_ascii_lowercase();
    let rest = match lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    {
        Some(r) => r,
        None => return false,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    !authority.is_empty() && !authority.contains('@')
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // kana
        | 0x3400..=0x9FFF // CJK ideographs
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() && !is_cjk(c)
}

/// Lowercase, hyphens/underscores/nbsp as spaces, whitespace collapsed.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = match c {
            '-' | '_' | '\u{a0}' | '\u{2010}'..='\u{2014}' => ' ',
            '\u{2019}' => '\'',
            c => c,
        };
        if c.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(c);
            space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn has_phrase(text: &str, strength: Strength) -> bool {
    UNSUBSCRIBE_PHRASES
        .iter()
        .filter(|(_, s)| *s == strength)
        .any(|(p, _)| contains_phrase(text, p))
}

/// `phrase` in `text`, as whole words where the phrase's edge is a word
/// character of a spaced script.
fn contains_phrase(text: &str, phrase: &str) -> bool {
    let first = phrase.chars().next().is_some_and(is_word_char);
    let last = phrase.chars().next_back().is_some_and(is_word_char);
    text.match_indices(phrase).any(|(i, _)| {
        let before_ok = !first
            || text[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !is_word_char(c));
        let after_ok = !last
            || text[i + phrase.len()..]
                .chars()
                .next()
                .is_none_or(|c| !is_word_char(c));
        before_ok && after_ok
    })
}

#[derive(Debug, Default)]
struct Anchor {
    href: String,
    text: String,
    before: String,
    after: String,
}

/// Tags that start or end a block: text on either side isn't "around" a link.
fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "td"
            | "th"
            | "tr"
            | "table"
            | "tbody"
            | "li"
            | "ul"
            | "ol"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
            | "center"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "details"
            | "summary"
            | "hr"
            | "pre"
            | "body"
    )
}

fn push_capped_tail(buf: &mut String, text: &str) {
    buf.push_str(text);
    if buf.len() > NEAR_CHARS * 4 {
        let mut cut = buf.len() - NEAR_CHARS * 4;
        while !buf.is_char_boundary(cut) {
            cut += 1;
        }
        buf.drain(..cut);
    }
}

fn anchors(html: &str) -> Vec<Anchor> {
    let mut out: Vec<Anchor> = Vec::new();
    // Text since the last block boundary (for the next link's `before`).
    let mut recent = String::new();
    let mut open: Option<Anchor> = None;
    // Index in `out` of the last closed link still collecting `after`.
    let mut trailing: Option<usize> = None;
    let mut rest = html;
    while !rest.is_empty() {
        let lt = rest.find('<').unwrap_or(rest.len());
        if lt > 0 {
            let text = decode_entities(&rest[..lt]);
            if let Some(a) = open.as_mut() {
                a.text.push_str(&text);
            } else if let Some(i) = trailing {
                let after = &mut out[i].after;
                after.push_str(&text);
                if after.chars().count() >= NEAR_CHARS {
                    trailing = None;
                }
            }
            push_capped_tail(&mut recent, &text);
            rest = &rest[lt..];
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        if tag.starts_with('!') || tag.starts_with('?') {
            continue;
        }
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        match (name.as_str(), closing) {
            ("style" | "script" | "title" | "head", false) => {
                let end = format!("</{name}");
                rest = match find_ascii_ci(rest, &end) {
                    Some(i) => &rest[i..],
                    None => "",
                };
            }
            ("a", false) => {
                trailing = None;
                open = Some(Anchor {
                    href: attr(tag, "href").unwrap_or_default(),
                    before: tail_chars(&recent, NEAR_CHARS),
                    ..Anchor::default()
                });
            }
            ("a", true) => {
                if let Some(a) = open.take() {
                    out.push(a);
                    trailing = Some(out.len() - 1);
                }
            }
            (n, _) if is_block(n) => {
                recent.clear();
                trailing = None;
                // A block inside a link (rare): keep the link open.
            }
            ("br", _) => push_capped_tail(&mut recent, " "),
            _ => {}
        }
    }
    if let Some(a) = open.take() {
        out.push(a);
    }
    out
}

fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(n)).collect()
}

fn find_ascii_ci(hay: &str, needle: &str) -> Option<usize> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// A double-quoted attribute's decoded value (html5ever always quotes so).
fn attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    let pat = format!("{name}=\"");
    loop {
        let i = find_ascii_ci(rest, &pat)?;
        let preceded_by_space = rest[..i].ends_with(|c: char| c.is_ascii_whitespace());
        let value_start = i + pat.len();
        let end = rest[value_start..].find('"')? + value_start;
        if preceded_by_space {
            return Some(decode_entities(&rest[value_start..end]));
        }
        rest = &rest[end + 1..];
    }
}

/// The escapes html5ever's serializer writes, plus numeric references.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let window = &rest.as_bytes()[..rest.len().min(12)];
        let Some(semi) = window.iter().position(|&b| b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..semi];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
#[path = "unsubscribe_tests.rs"]
mod tests;
