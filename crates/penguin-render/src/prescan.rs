//! One quick pass over the raw HTML to collect what ammonia's per-attribute
//! callback can't see:
//!
//! - the text of `<style>` elements (ammonia drops them; we sanitize the CSS
//!   ourselves and re-emit it),
//! - the first `<body>` tag's presentational attributes (fragment parsing
//!   drops `<body>`, and newsletters put their background color there),
//! - which image URLs are declared tiny or hidden (tracking pixels), which
//!   needs an `<img>`'s width/height/style together with its `src`.
//!
//! This pass only gathers hints. It never decides what markup survives:
//! ammonia parses the input with the full HTML5 tree builder and is the only
//! gate for elements and attributes, and the collected CSS goes through the
//! same sanitizer as inline styles. So this scanner follows the HTML
//! tokenizer closely where it matters (comments, raw-text elements, quoted
//! attributes) but doesn't need to be exact: a disagreement can at worst
//! miss a pixel or apply a style rule the browser would have ignored. It is
//! hand-rolled because a full tokenizer pass cost ~10 ms on a 2 MB message.

use std::borrow::Cow;
use std::collections::HashSet;

const MAX_STYLE_BYTES: usize = 1024 * 1024;
/// Real mail tops out around 20 attributes per tag; html5ever's duplicate check
/// is quadratic in this number (see `MAX_NESTING_DEPTH` in lib.rs).
pub(crate) const MAX_ATTRS: usize = 128;

#[derive(Debug, Default)]
pub(crate) struct Prescan {
    pub style_text: String,
    pub body_style: Option<String>,
    pub body_bgcolor: Option<String>,
    pub body_text_color: Option<String>,
    /// A `<meta name="color-scheme">` (or `supported-color-schemes`) that
    /// includes `dark`: the message says it works on a dark canvas.
    pub meta_dark_scheme: bool,
    /// `src` values of images sized ≤ 3×3 px or hidden.
    pub tiny_srcs: HashSet<String>,
    /// Upper bound on element nesting depth seen while scanning.
    pub max_depth: usize,
    /// Most attributes on a single tag.
    pub max_attrs: usize,
    /// Text content (tags removed, entities decoded), for the simplified
    /// fallback view. Only filled in when `collect_text` is set.
    pub text: String,
}

/// Elements that never have children.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "keygen", "link", "meta", "param",
    "source", "track", "wbr", "frame", "basefont", "bgsound", "image", "isindex",
];

/// Elements whose content is text, not markup.
const RAW_TEXT: &[&str] = &[
    "style",
    "script",
    "title",
    "textarea",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "noscript",
    "plaintext",
];

/// Elements whose end tag is optional and which the parser closes
/// implicitly (a new `<p>` or `<td>` closes the previous one), so they
/// can't nest without a counted container in between.
const IMPLIED_END: &[&str] = &[
    "p", "li", "dt", "dd", "tr", "td", "th", "option", "optgroup", "tbody", "thead", "tfoot",
    "colgroup", "caption", "rt", "rp", "rb", "rtc", "html", "head", "body",
];

/// Tracks an upper bound on the parser's stack of open elements. Parsing
/// cost grows with depth × tags (scope checks walk the stack), so this is
/// what the renderer's complexity guard looks at.
#[derive(Default)]
struct DepthTracker {
    open: std::collections::HashMap<String, usize>,
    depth: usize,
    max: usize,
}

impl DepthTracker {
    fn start(&mut self, name: &str) {
        // Raw-text elements are skipped whole by the scanner (their end
        // tag is consumed with the content), so they never stay open.
        if VOID.contains(&name) || IMPLIED_END.contains(&name) || RAW_TEXT.contains(&name) {
            return;
        }
        *self.open.entry(name.to_string()).or_default() += 1;
        self.depth += 1;
        self.max = self.max.max(self.depth);
    }

    fn end(&mut self, name: &str) {
        if let Some(n) = self.open.get_mut(name) {
            if *n > 0 {
                *n -= 1;
                self.depth -= 1;
            }
        }
    }
}

/// Parse an HTML length attribute/CSS value to pixels when it's a plain
/// number (optionally `px`). Percentages and other units are "not tiny".
fn px(v: &str) -> Option<f32> {
    let v = v.trim().trim_end_matches("!important").trim();
    let v = v.strip_suffix("px").unwrap_or(v).trim();
    v.parse::<f32>().ok().filter(|n| n.is_finite())
}

fn is_tiny_or_hidden(width: Option<&str>, height: Option<&str>, style: Option<&str>) -> bool {
    let mut w = width.and_then(px);
    let mut h = height.and_then(px);
    let mut hidden = false;
    if let Some(style) = style {
        for decl in style.split(';') {
            let Some((p, v)) = decl.split_once(':') else {
                continue;
            };
            let p = p.trim().to_ascii_lowercase();
            let v = v.trim().to_ascii_lowercase();
            let v = v.trim_end_matches("!important").trim();
            match p.as_str() {
                "width" | "max-width" => w = px(v).or(w),
                "height" | "max-height" => h = px(v).or(h),
                "display" if v == "none" => hidden = true,
                "visibility" if v == "hidden" => hidden = true,
                "opacity" if px(v) == Some(0.0) => hidden = true,
                _ => {}
            }
        }
    }
    let small = |d: Option<f32>| d.is_some_and(|n| n <= 3.0);
    let line = |d: Option<f32>, other: Option<f32>| other.is_none() && d.is_some_and(|n| n <= 1.0);
    hidden || (small(w) && small(h)) || line(w, h) || line(h, w)
}

/// Decode the character references that realistically appear in URLs and
/// sizes. Anything else is left encoded (the lookup then just misses).
fn decode_entities(v: &str) -> Cow<'_, str> {
    if !v.contains('&') {
        return Cow::Borrowed(v);
    }
    let mut out = String::with_capacity(v.len());
    let mut rest = v;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .map(|e| e + 1);
        let (name, consumed) = match end {
            Some(e) if rest[e..].starts_with(';') => (&rest[1..e], e + 1),
            Some(e) => (&rest[1..e], e),
            None => (&rest[1..], rest.len()),
        };
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16)
                .ok()
                .and_then(char::from_u32),
            n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[consumed..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

/// Position just after the next case-insensitive `</name`, or the end.
fn find_end_tag(h: &[u8], from: usize, name: &str) -> (usize, usize) {
    let n = name.len();
    let mut i = from;
    while let Some(off) = memchr(b'<', &h[i..]) {
        let p = i + off;
        if h.get(p + 1) == Some(&b'/')
            && h.get(p + 2..p + 2 + n)
                .is_some_and(|s| s.eq_ignore_ascii_case(name.as_bytes()))
            && h.get(p + 2 + n)
                .is_none_or(|&b| is_space(b) || b == b'>' || b == b'/')
        {
            return (p, p + 2 + n);
        }
        i = p + 1;
    }
    (h.len(), h.len())
}

fn memchr(needle: u8, hay: &[u8]) -> Option<usize> {
    hay.iter().position(|&b| b == needle)
}

/// Parse a start tag's attributes from `i` (just after the tag name).
/// Returns the attributes (names lowercased, raw values) and the position
/// after the closing `>`.
fn parse_attrs(html: &str, mut i: usize) -> (Vec<(String, &str)>, usize) {
    let h = html.as_bytes();
    let mut attrs = Vec::new();
    loop {
        while i < h.len() && (is_space(h[i]) || h[i] == b'/') {
            i += 1;
        }
        if i >= h.len() {
            return (attrs, i);
        }
        if h[i] == b'>' {
            return (attrs, i + 1);
        }
        let start = i;
        i += 1; // a leading '=' is part of the name, per the tokenizer
        while i < h.len() && !is_space(h[i]) && !matches!(h[i], b'/' | b'>' | b'=') {
            i += 1;
        }
        let name = html[start..i].to_ascii_lowercase();
        while i < h.len() && is_space(h[i]) {
            i += 1;
        }
        let mut value = "";
        if h.get(i) == Some(&b'=') {
            i += 1;
            while i < h.len() && is_space(h[i]) {
                i += 1;
            }
            match h.get(i) {
                Some(&q @ (b'"' | b'\'')) => {
                    let end = memchr(q, &h[i + 1..]).map_or(h.len(), |e| i + 1 + e);
                    value = &html[i + 1..end];
                    i = (end + 1).min(h.len());
                }
                Some(_) => {
                    let start = i;
                    while i < h.len() && !is_space(h[i]) && h[i] != b'>' {
                        i += 1;
                    }
                    value = &html[start..i];
                }
                None => {}
            }
        }
        // Duplicates keep the first value, as in the tokenizer. Past
        // MAX_ATTRS the tag is hostile anyway (see `render_html`), so stop
        // de-duplicating to stay linear.
        if attrs.len() > MAX_ATTRS || !attrs.iter().any(|(n, _)| *n == name) {
            attrs.push((name, value));
        }
    }
}

pub(crate) fn prescan(html: &str, collect_text: bool) -> Prescan {
    let h = html.as_bytes();
    let mut out = Prescan::default();
    let mut depth = DepthTracker::default();
    let mut seen_body = false;
    let mut i = 0;
    loop {
        let Some(off) = memchr(b'<', &h[i..]) else {
            if collect_text {
                out.text.push_str(&decode_entities(&html[i..]));
            }
            break;
        };
        let lt = i + off;
        if collect_text {
            out.text.push_str(&decode_entities(&html[i..lt]));
        }
        i = lt + 1;
        let rest = &h[lt..];
        if rest.starts_with(b"<!--") {
            // Comment: ends at the first "-->" (or "--!>"); "<!-->" and
            // "<!--->" close immediately, as in the tokenizer.
            let body = lt + 4;
            if h.get(body) == Some(&b'>') || h.get(body..body + 2) == Some(b"->") {
                continue;
            }
            let end = html[body..].find("-->").map(|e| body + e + 3);
            let end_bang = html[body..].find("--!>").map(|e| body + e + 4);
            i = match (end, end_bang) {
                (Some(a), Some(b)) => a.min(b),
                (a, b) => a.or(b).unwrap_or(h.len()),
            };
            continue;
        }
        if matches!(rest.get(1), Some(b'!' | b'?' | b'/')) {
            // Doctype, bogus comment, end tag: skip to '>'.
            let end = memchr(b'>', &h[lt..]).map_or(h.len(), |e| lt + e);
            if rest.get(1) == Some(&b'/') {
                let name = html[lt + 2..end]
                    .split(|c: char| c.is_ascii_whitespace() || c == '/')
                    .next()
                    .unwrap_or("");
                depth.end(&name.to_ascii_lowercase());
                if collect_text
                    && matches!(
                        name.to_ascii_lowercase().as_str(),
                        "p" | "div" | "tr" | "li" | "table" | "h1" | "h2" | "h3"
                    )
                {
                    out.text.push('\n');
                }
            }
            i = (end + 1).min(h.len());
            continue;
        }
        let name_end = lt
            + 1
            + rest[1..]
                .iter()
                .position(|&b| is_space(b) || b == b'/' || b == b'>')
                .unwrap_or(rest.len() - 1);
        let name = &html[lt + 1..name_end];
        if name.is_empty() || !name.as_bytes()[0].is_ascii_alphabetic() {
            continue;
        }
        let (attrs, after) = parse_attrs(html, name_end);
        i = after;
        out.max_attrs = out.max_attrs.max(attrs.len());
        let attr = |n: &str| {
            attrs
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| decode_entities(v))
        };
        let lname = name.to_ascii_lowercase();
        depth.start(&lname);
        if collect_text && lname == "br" {
            out.text.push('\n');
        }
        match lname.as_str() {
            "img" => {
                if let Some(src) = attr("src") {
                    let (w, hh, st) = (attr("width"), attr("height"), attr("style"));
                    if is_tiny_or_hidden(w.as_deref(), hh.as_deref(), st.as_deref()) {
                        out.tiny_srcs.insert(src.trim().to_string());
                    }
                }
            }
            "meta" => {
                let name = attr("name").map(|n| n.trim().to_ascii_lowercase());
                if matches!(
                    name.as_deref(),
                    Some("color-scheme" | "supported-color-schemes")
                ) {
                    let content = attr("content")
                        .map(|c| c.to_ascii_lowercase())
                        .unwrap_or_default();
                    out.meta_dark_scheme |=
                        content.contains("dark") && !content.contains("only light");
                }
            }
            "body" if !seen_body => {
                seen_body = true;
                out.body_style = attr("style").map(Cow::into_owned);
                out.body_bgcolor = attr("bgcolor").map(Cow::into_owned);
                out.body_text_color = attr("text").map(Cow::into_owned);
            }
            "style" => {
                let (content_end, resume) = find_end_tag(h, i, "style");
                let css = &html[i..content_end];
                if out.style_text.len() + css.len() <= MAX_STYLE_BYTES {
                    out.style_text.push_str(css);
                    out.style_text.push('\n');
                }
                i = resume;
            }
            "plaintext" => break,
            // Raw-text / RCDATA elements: their content is not markup.
            n if RAW_TEXT.contains(&n) => {
                i = find_end_tag(h, i, &lname).1;
            }
            _ => {}
        }
    }
    out.max_depth = depth.max;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_styles_body_and_pixels() {
        let p = prescan(
            r##"<html><head><style>.a{color:red}</style><title><style>.no{}</style></title>
            <!--[if mso]><style>.mso{width:600px}</style><![endif]--></head>
            <body bgcolor="#eeeeee" style="margin:0"><img src="https://t.example/p.gif?a=1&amp;b=2" width="1" height="1">
            <img src="https://t.example/q.gif" style="display:none"><img src="https://c.example/hero.jpg" width="600" height="1">
            <img title="a > b" src='https://t.example/r.gif' style="width:0px;height:0px"><IMG SRC=https://t.example/s.gif WIDTH=2 HEIGHT=2>
            <script>"<img src='https://s.example/x' width=1 height=1>"</script>
            <p title="<img src=https://s.example/y width=1 height=1>">x</p>"##,
            false,
        );
        assert_eq!(p.style_text.trim(), ".a{color:red}");
        assert_eq!(p.body_bgcolor.as_deref(), Some("#eeeeee"));
        assert_eq!(p.body_style.as_deref(), Some("margin:0"));
        assert!(!p.meta_dark_scheme);
        for tiny in [
            "https://t.example/p.gif?a=1&b=2",
            "https://t.example/q.gif",
            "https://t.example/r.gif",
            "https://t.example/s.gif",
        ] {
            assert!(p.tiny_srcs.contains(tiny), "{tiny} {:?}", p.tiny_srcs);
        }
        for not_tiny in [
            "https://c.example/hero.jpg",
            "https://s.example/x",
            "https://s.example/y",
        ] {
            assert!(!p.tiny_srcs.contains(not_tiny), "{not_tiny}");
        }
    }

    #[test]
    fn depth_bound() {
        assert_eq!(prescan(&"<div>".repeat(100), false).max_depth, 100);
        assert_eq!(prescan(&"<div></div>".repeat(100), false).max_depth, 1);
        assert_eq!(prescan(&"<p>x".repeat(100), false).max_depth, 0);
        // Stray end tags of other names don't hide open elements.
        assert_eq!(
            prescan(&"<span>".repeat(50).add_str(&"</div>".repeat(50)), false).max_depth,
            50
        );
        assert_eq!(prescan(&"<div/>".repeat(10), false).max_depth, 10);
        assert_eq!(
            prescan(
                "<table><tr><td><table><tr><td>x</td></tr></table></td></tr></table>",
                false
            )
            .max_depth,
            2
        );
    }

    trait AddStr {
        fn add_str(self, s: &str) -> String;
    }
    impl AddStr for String {
        fn add_str(mut self, s: &str) -> String {
            self.push_str(s);
            self
        }
    }

    #[test]
    fn color_scheme_meta() {
        for (html, dark) in [
            (r#"<meta name="color-scheme" content="light dark">"#, true),
            (
                r#"<META NAME="supported-color-schemes" CONTENT="dark">"#,
                true,
            ),
            (r#"<meta name="color-scheme" content="light">"#, false),
            (r#"<meta name="color-scheme" content="only light">"#, false),
            (r#"<meta name="theme-color" content="dark">"#, false),
            (
                r#"<p title='<meta name="color-scheme" content="dark">'>x</p>"#,
                false,
            ),
        ] {
            assert_eq!(prescan(html, false).meta_dark_scheme, dark, "{html}");
        }
    }

    #[test]
    fn entity_decoding() {
        assert_eq!(
            decode_entities("a&amp;b&#38;c&#x26;d&nbsp&unknown;"),
            "a&b&c&d\u{a0}&unknown;"
        );
        assert_eq!(decode_entities("&"), "&");
        assert_eq!(decode_entities("&#xZZ;"), "&#xZZ;");
    }

    #[test]
    fn malformed_input_terminates() {
        for s in [
            "<",
            "<!--",
            "<img src=\"x",
            "<style>",
            "<body",
            "<a <b <c",
            "<img src='",
            "</",
            "<!",
            "\u{fffd}<é>",
        ] {
            prescan(s, true);
        }
    }
}
