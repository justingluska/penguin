//! CSS sanitizer for `style` attributes and `<style>` blocks.
//!
//! The approach is allowlist-and-reserialize: input is split into
//! declarations (and, for stylesheets, rules), each piece is checked, and the
//! output is rebuilt only from pieces that passed. Anything we can't reason
//! about (escapes, unknown functions, unbalanced quotes) is dropped rather
//! than repaired, so the emitted CSS always tokenizes the way we checked it.
//!
//! Invariants of every string this module returns:
//! - no `\`, `<`, `>`, no at-rules other than our own `@media`, and no
//!   `{`/`}` outside the rule structure we emitted, and no comments, so it can't escape a `<style>`
//!   element or a quoted attribute, and can't hide keywords behind escapes;
//! - no `url()` other than what the caller's resolver returned;
//! - no function outside [`ALLOWED_FUNCTIONS`].

use std::borrow::Cow;

/// Maps a raw `url()` argument to a safe replacement URL, or `None` to drop it.
/// The returned URL must not contain `"`, `\`, `(`, `)` or whitespace (the
/// image resolver in `lib.rs` guarantees this).
pub(crate) type UrlResolver<'a> = dyn Fn(&str) -> Option<String> + 'a;

const MAX_STYLESHEET_BYTES: usize = 512 * 1024;
const MAX_SELECTOR_BYTES: usize = 2048;

/// Properties an email may set. Layout, box model, typography, color and
/// table properties; nothing that fetches (except the background family,
/// whose `url()` goes through the resolver), animates, or changes stacking
/// relative to anything outside the message.
const ALLOWED_PROPERTIES: &[&str] = &[
    "align-content",
    "align-items",
    "align-self",
    "background",
    "background-color",
    "background-image",
    "background-position",
    "background-position-x",
    "background-position-y",
    "background-repeat",
    "background-size",
    "background-clip",
    "background-origin",
    "border",
    "border-bottom",
    "border-bottom-color",
    "border-bottom-left-radius",
    "border-bottom-right-radius",
    "border-bottom-style",
    "border-bottom-width",
    "border-collapse",
    "border-color",
    "border-left",
    "border-left-color",
    "border-left-style",
    "border-left-width",
    "border-radius",
    "border-right",
    "border-right-color",
    "border-right-style",
    "border-right-width",
    "border-spacing",
    "border-style",
    "border-top",
    "border-top-color",
    "border-top-left-radius",
    "border-top-right-radius",
    "border-top-style",
    "border-top-width",
    "border-width",
    "bottom",
    "box-shadow",
    "box-sizing",
    "caption-side",
    "clear",
    "color",
    "column-gap",
    "direction",
    "display",
    "empty-cells",
    "flex",
    "flex-basis",
    "flex-direction",
    "flex-flow",
    "flex-grow",
    "flex-shrink",
    "flex-wrap",
    "float",
    "font",
    "font-family",
    "font-feature-settings",
    "font-kerning",
    "font-size",
    "font-stretch",
    "font-style",
    "font-variant",
    "font-weight",
    "gap",
    "height",
    "hyphens",
    "justify-content",
    "left",
    "letter-spacing",
    "line-height",
    "list-style",
    "list-style-position",
    "list-style-type",
    "margin",
    "margin-bottom",
    "margin-left",
    "margin-right",
    "margin-top",
    "max-height",
    "max-width",
    "min-height",
    "min-width",
    "mso-line-height-rule",
    "object-fit",
    "object-position",
    "opacity",
    "order",
    "outline",
    "outline-color",
    "outline-offset",
    "outline-style",
    "outline-width",
    "overflow",
    "overflow-wrap",
    "overflow-x",
    "overflow-y",
    "padding",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "padding-top",
    "position",
    "right",
    "row-gap",
    "table-layout",
    "text-align",
    "text-decoration",
    "text-decoration-color",
    "text-decoration-line",
    "text-decoration-style",
    "text-indent",
    "text-overflow",
    "text-shadow",
    "text-transform",
    "text-underline-offset",
    "top",
    "unicode-bidi",
    "vertical-align",
    "visibility",
    "white-space",
    "width",
    "word-break",
    "word-spacing",
    "word-wrap",
    "z-index",
    "-webkit-text-size-adjust",
    "-ms-text-size-adjust",
    "text-size-adjust",
    "-webkit-font-smoothing",
    "-moz-osx-font-smoothing",
];

/// Properties whose value may contain `url()` (resolved as an image).
const URL_PROPERTIES: &[&str] = &["background", "background-image"];

/// CSS functions allowed in values. Everything else (`image-set`, `element`,
/// `paint`, `attr`, `var`, `env`, `expression`, …) drops the declaration.
const ALLOWED_FUNCTIONS: &[&str] = &[
    "url",
    "rgb",
    "rgba",
    "hsl",
    "hsla",
    "hwb",
    "lab",
    "lch",
    "oklab",
    "oklch",
    "color",
    "calc",
    "min",
    "max",
    "clamp",
    "linear-gradient",
    "radial-gradient",
    "conic-gradient",
    "repeating-linear-gradient",
    "repeating-radial-gradient",
    "repeating-conic-gradient",
];

/// Substrings that are never legitimate in email CSS values. Checked on the
/// lowercased, comment-free value (escapes are rejected separately).
const BANNED_SUBSTRINGS: &[&str] = &[
    "expression",
    "javascript:",
    "vbscript:",
    "livescript:",
    "behavior",
    "behaviour",
    "-moz-binding",
    "binding",
    "@import",
    "&#",
    "<!--",
    "-->",
];

fn allowed_property(p: &str) -> bool {
    static SET: std::sync::OnceLock<std::collections::HashSet<&'static str>> =
        std::sync::OnceLock::new();
    SET.get_or_init(|| ALLOWED_PROPERTIES.iter().copied().collect())
        .contains(p)
}

/// Remove `/* … */` comments. An unterminated comment swallows the rest, as
/// in a browser.
fn strip_comments(s: &str) -> Cow<'_, str> {
    if !s.contains("/*") {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        out.push(' ');
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return Cow::Owned(out),
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Split at `sep` characters that are outside quotes and parentheses.
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' => depth -= 1,
                _ if c == sep && depth <= 0 => {
                    parts.push(&s[start..i]);
                    start = i + c.len_utf8();
                }
                _ => {}
            },
        }
    }
    parts.push(&s[start..]);
    parts
}

fn is_property_name(p: &str) -> bool {
    !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
}

/// Viewport-height units make the auto-sized iframe grow every time it is
/// resized (content height depends on frame height), so they are dropped.
fn has_viewport_height_unit(lv: &str) -> bool {
    if !lv.contains('v') {
        return false;
    }
    let b = lv.as_bytes();
    ["vh", "vmin", "vmax", "dvh", "svh", "lvh", "vb"]
        .iter()
        .any(|unit| {
            lv.match_indices(unit).any(|(i, _)| {
                let before_is_num = i > 0 && (b[i - 1].is_ascii_digit() || b[i - 1] == b'.');
                let after = b.get(i + unit.len()).copied();
                let after_ok = !matches!(after, Some(c) if c.is_ascii_alphanumeric() || c == b'-');
                before_is_num && after_ok
            })
        })
}

/// Validate one declaration value and rewrite its `url()`s. Returns `None`
/// if the declaration must be dropped.
fn sanitize_value(prop: &str, value: &str, resolve: &UrlResolver<'_>) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 8 * 1024 * 1024 {
        return None;
    }
    if value.chars().any(|c| {
        matches!(c, '\\' | '<' | '>' | '{' | '}' | '`')
            || (c.is_control() && c != '\t' && c != '\n' && c != '\r')
    }) {
        return None;
    }
    // A `;` left after splitting sits inside quotes or parentheses (e.g. a
    // `data:image/png;base64,` URL), which is fine.
    let lv: Cow<'_, str> = if value.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(value.to_ascii_lowercase())
    } else {
        Cow::Borrowed(value)
    };
    if BANNED_SUBSTRINGS.iter().any(|b| lv.contains(b)) || has_viewport_height_unit(&lv) {
        return None;
    }

    // Property-specific keyword checks. `position: fixed/sticky` would let
    // a message pin an overlay over its own frame, so only flow positions
    // are allowed.
    let bare = lv.trim_end_matches("!important").trim();
    if prop == "position" && !matches!(bare, "static" | "relative" | "absolute") {
        return None;
    }

    // Walk the value: track quotes and parentheses, check every function
    // name, and rewrite url() arguments.
    let url_allowed = URL_PROPERTIES.contains(&prop);
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    let mut depth = 0i32;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'"' | b'\'' => {
                let end = value[i + 1..].find(c as char)? + i + 1;
                let s = &value[i..=end];
                if s.contains('\n') || s.contains('\r') {
                    return None;
                }
                out.push_str(s);
                i = end + 1;
            }
            b'(' => {
                // Function name = the identifier immediately before '('.
                let name_start = out
                    .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
                    .map(|p| p + 1)
                    .unwrap_or(0);
                let name = out[name_start..].to_ascii_lowercase();
                if name.is_empty() || !ALLOWED_FUNCTIONS.contains(&name.as_str()) {
                    return None;
                }
                if name == "url" {
                    if !url_allowed {
                        return None;
                    }
                    let close = value[i + 1..].find(')')? + i + 1;
                    let mut arg = value[i + 1..close].trim();
                    if (arg.starts_with('"') && arg.ends_with('"')
                        || arg.starts_with('\'') && arg.ends_with('\''))
                        && arg.len() >= 2
                    {
                        arg = &arg[1..arg.len() - 1];
                    }
                    if arg.contains(['"', '\'', '(']) {
                        return None;
                    }
                    out.truncate(name_start);
                    match resolve(arg) {
                        Some(url) => {
                            out.push_str("url(\"");
                            out.push_str(&url);
                            out.push_str("\")");
                        }
                        // A blocked image becomes `none`, which is valid
                        // wherever a background image is, so the rest of
                        // a `background` shorthand (the color) survives.
                        None => out.push_str("none"),
                    }
                    i = close + 1;
                    continue;
                }
                depth += 1;
                out.push('(');
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
                out.push(')');
                i += 1;
            }
            b'\n' | b'\r' | b'\t' => {
                out.push(' ');
                i += 1;
            }
            _ => {
                let ch = value[i..].chars().next()?;
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    if depth != 0 {
        return None;
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Sanitize a declaration list (a `style` attribute or a rule body).
/// Returns `prop:value;…` or an empty string.
pub(crate) fn sanitize_declarations(input: &str, resolve: &UrlResolver<'_>) -> String {
    let input = strip_comments(input);
    let mut out = String::new();
    for decl in split_top_level(&input, ';') {
        let Some((prop, value)) = decl.split_once(':') else {
            continue;
        };
        let prop = prop.trim();
        let prop: Cow<'_, str> = if prop.bytes().any(|b| b.is_ascii_uppercase()) {
            Cow::Owned(prop.to_ascii_lowercase())
        } else {
            Cow::Borrowed(prop)
        };
        if !is_property_name(&prop) || !allowed_property(&prop) {
            continue;
        }
        if let Some(v) = sanitize_value(&prop, value, resolve) {
            out.push_str(&prop);
            out.push(':');
            out.push_str(&v);
            out.push(';');
        }
    }
    out
}

fn balanced(s: &str, open: char, close: char) -> bool {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == open => depth += 1,
            None if c == close => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            None => {}
        }
    }
    depth == 0 && quote.is_none()
}

fn valid_selector(sel: &str) -> bool {
    !sel.is_empty()
        && sel.len() <= MAX_SELECTOR_BYTES
        && !sel.chars().any(|c| {
            matches!(c, '\\' | '<' | '@' | '{' | '}' | ';' | '`')
                || (c.is_control() && c != '\n' && c != '\t' && c != '\r')
        })
        && balanced(sel, '[', ']')
        && balanced(sel, '(', ')')
}

/// `@media` preludes: plain media queries only (color-scheme queries are
/// sorted out by [`scheme_query`] first).
fn valid_media_prelude(p: &str) -> bool {
    let lp = p.to_ascii_lowercase();
    !lp.is_empty()
        && lp.len() <= 512
        && lp
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " ,:()-._\t\n\r".contains(c))
        && !lp.contains("prefers-color-scheme")
        && balanced(&lp, '(', ')')
}

/// What a whitespace-normalized `@media` prelude says about color schemes.
#[derive(Debug, PartialEq)]
enum SchemeQuery {
    /// No `prefers-color-scheme` in it: an ordinary query.
    Plain,
    /// Exactly one `(prefers-color-scheme: dark)` test and no `not`: the
    /// prelude with that test replaced by an always-true one, for the dark
    /// sheet (which the UI switches on as a whole).
    Dark(String),
    /// Light-only, negated or otherwise unclear: dropped. The canvas is light
    /// unless the UI turns the dark sheet on, so light rules add nothing.
    Drop,
}

fn scheme_query(query: &str) -> SchemeQuery {
    const FEATURE: &str = "(prefers-color-scheme";
    let lq = query.to_ascii_lowercase();
    let Some(start) = lq.find(FEATURE) else {
        return SchemeQuery::Plain;
    };
    let tail = start + FEATURE.len();
    let dark_end = lq[tail..].find(')').and_then(|close| {
        let value = lq[tail..tail + close].trim_start().strip_prefix(':')?;
        (value.trim() == "dark").then_some(tail + close)
    });
    match dark_end {
        Some(end)
            if lq.matches("prefers-color-scheme").count() == 1
                && !lq
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|w| w == "not") =>
        {
            // ASCII lowercasing keeps byte offsets, so they index `query` too.
            SchemeQuery::Dark(format!(
                "{}(min-width:0){}",
                &query[..start],
                &query[end + 1..]
            ))
        }
        _ => SchemeQuery::Drop,
    }
}

/// Whether a raw stylesheet declares that the message supports a dark
/// canvas: `color-scheme` (or Apple's older `supported-color-schemes`) with
/// `dark` in the value, e.g. `:root{color-scheme:light dark}`. The property
/// itself isn't allowed through; this only feeds the renderer's flag.
pub(crate) fn declares_dark_scheme(css: &str) -> bool {
    let lc = css.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lc[from..].find("color-scheme") {
        let at = from + i;
        from = at + "color-scheme".len();
        if lc[..at].ends_with("prefers-") {
            continue;
        }
        let rest = lc[from..].strip_prefix('s').unwrap_or(&lc[from..]);
        let Some(value) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        let value = &value[..value.find([';', '}', '"', '\'']).unwrap_or(value.len())];
        if value.contains("dark") && !value.contains("only light") {
            return true;
        }
    }
    false
}

/// Scan from `start` (just after a `{`) to the matching `}`; returns the index
/// of that `}`. Quotes are respected; `None` if unterminated.
fn matching_brace(s: &str, start: usize) -> Option<usize> {
    let mut depth = 1i32;
    let mut quote: Option<char> = None;
    for (i, c) in s[start..].char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(start + i);
                    }
                }
                _ => {}
            },
        }
    }
    None
}

/// Find the first `stop` char at top level (outside quotes/brackets/parens).
fn find_top_level(s: &str, stops: &[char]) -> Option<(usize, char)> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                _ if depth <= 0 && stops.contains(&c) => return Some((i, c)),
                _ => {}
            },
        }
    }
    None
}

/// `dark` receives `(prefers-color-scheme: dark)` blocks. It is `None`
/// inside a block, where at-rules are dropped.
fn sanitize_rules(
    css: &str,
    resolve: &UrlResolver<'_>,
    out: &mut String,
    mut dark: Option<&mut String>,
) {
    let mut rest = css;
    loop {
        rest = rest.trim_start();
        let written = out.len() + dark.as_deref().map_or(0, String::len);
        if rest.is_empty() || written > MAX_STYLESHEET_BYTES {
            return;
        }
        if let Some(after_at) = rest.strip_prefix('@') {
            let Some((pos, stop)) = find_top_level(after_at, &['{', ';']) else {
                return;
            };
            if stop == ';' {
                // @import, @charset, @namespace: all dropped.
                rest = &after_at[pos + 1..];
                continue;
            }
            let Some(end) = matching_brace(after_at, pos + 1) else {
                return;
            };
            let prelude = &after_at[..pos];
            let name_len = prelude
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .unwrap_or(prelude.len());
            let (name, query) = prelude.split_at(name_len);
            if let Some(dark) = dark
                .as_deref_mut()
                .filter(|_| name.eq_ignore_ascii_case("media"))
            {
                let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
                let target = match scheme_query(&query) {
                    SchemeQuery::Plain => Some((&mut *out, query)),
                    SchemeQuery::Dark(q) => Some((dark, q)),
                    SchemeQuery::Drop => None,
                };
                if let Some((dest, query)) = target.filter(|(_, q)| valid_media_prelude(q)) {
                    let mut inner = String::new();
                    sanitize_rules(&after_at[pos + 1..end], resolve, &mut inner, None);
                    if !inner.is_empty() {
                        dest.push_str("@media ");
                        dest.push_str(&query);
                        dest.push('{');
                        dest.push_str(&inner);
                        dest.push('}');
                    }
                }
            }
            // @font-face, @keyframes, @supports, @page, @layer, nested @media: dropped.
            rest = &after_at[end + 1..];
            continue;
        }
        let Some((pos, stop)) = find_top_level(rest, &['{', '}']) else {
            return;
        };
        if stop == '}' {
            // Stray close brace: skip it.
            rest = &rest[pos + 1..];
            continue;
        }
        let Some(end) = matching_brace(rest, pos + 1) else {
            return;
        };
        let selector = rest[..pos].trim();
        let body = &rest[pos + 1..end];
        rest = &rest[end + 1..];
        if !valid_selector(selector) {
            continue;
        }
        let decls = sanitize_declarations(body, resolve);
        if decls.is_empty() {
            continue;
        }
        out.push_str(&selector.split_whitespace().collect::<Vec<_>>().join(" "));
        out.push('{');
        out.push_str(&decls);
        out.push('}');
    }
}

/// A message's sanitized stylesheets.
#[derive(Debug, Default)]
pub(crate) struct Stylesheets {
    /// Everything but the dark-mode blocks.
    pub main: String,
    /// `@media (prefers-color-scheme: dark)` blocks, each re-emitted with
    /// that test replaced by `(min-width:0)`. The document puts them in a
    /// `<style>` that stays off until the UI switches it on.
    pub dark: String,
}

/// Sanitize the contents of `<style>` elements into stylesheets safe to
/// place inside our own `<style>` elements.
pub(crate) fn sanitize_stylesheet(css: &str, resolve: &UrlResolver<'_>) -> Stylesheets {
    let css = strip_comments(css).replace("<!--", " ").replace("-->", " ");
    let mut sheets = Stylesheets::default();
    sanitize_rules(&css, resolve, &mut sheets.main, Some(&mut sheets.dark));
    sheets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    fn decl(s: &str) -> String {
        sanitize_declarations(s, &none)
    }

    #[test]
    fn keeps_ordinary_email_styles() {
        assert_eq!(
            decl("color: #333; font-family: 'Helvetica Neue', Arial; padding: 0 10px !important"),
            "color:#333;font-family:'Helvetica Neue', Arial;padding:0 10px !important;"
        );
        assert_eq!(
            decl("background: #fff url(https://x.example/a.png) no-repeat"),
            "background:#fff none no-repeat;"
        );
        assert_eq!(decl("width: calc(100% - 20px)"), "width:calc(100% - 20px);");
    }

    #[test]
    fn drops_dangerous_declarations() {
        for bad in [
            "width: expression(alert(1))",
            "background: url(javascript:alert(1))",
            "behavior: url(x.htc)",
            "-moz-binding: url(x.xml#xss)",
            "position: fixed",
            "position:sticky",
            "height: 100vh",
            "color: var(--x)",
            "background-image: image-set('x.png' 1x)",
            "font-family: \"unterminated",
            "width: e\\78 pression(1)",
            "color: </style><script>",
            "list-style-image: url(https://x.example/t.gif)",
            "cursor: url(https://x.example/c.cur), auto",
            "content: url(https://x.example/a.png)",
            "width: ex/**/pression(alert(1))",
        ] {
            assert_eq!(decl(bad), "", "should drop: {bad}");
        }
        // An escaped `;` splits differently for us than for a browser; the
        // pieces are each validated, so the worst case is a harmless leftover.
        assert_eq!(
            decl("color: red\\; background: url(https://x.example/a.png)"),
            "background:none;"
        );
    }

    #[test]
    fn stylesheet_rules_are_rebuilt() {
        let css = "<!-- @import url(https://evil.example/x.css); body{margin:0} .a > b{color:red;position:fixed} \
                   @media only screen and (max-width:600px){.c{width:100%!important}} \
                   @media (prefers-color-scheme: dark){.c{color:#fff}} @font-face{font-family:x;src:url(https://evil.example/f.woff)} \
                   a[href^=\"x\"]{color:blue} x</style><script>{color:red} -->";
        let out = sanitize_stylesheet(css, &none);
        assert_eq!(
            out.main,
            "body{margin:0;}.a > b{color:red;}@media only screen and (max-width:600px){.c{width:100%!important;}}a[href^=\"x\"]{color:blue;}"
        );
        assert_eq!(out.dark, "@media (min-width:0){.c{color:#fff;}}");
    }

    #[test]
    fn dark_scheme_blocks_go_to_the_dark_sheet() {
        let css = "@media screen and (prefers-color-scheme:dark) and (max-width:600px){.a{background:#111!important;position:fixed}} \
                   @media (PREFERS-COLOR-SCHEME : DARK){.b{color:#eee}} \
                   @media (prefers-color-scheme: light){.c{color:#000}} \
                   @media not all and (prefers-color-scheme: dark){.d{color:red}} \
                   @media (prefers-color-scheme: dark), (prefers-color-scheme: light){.e{color:red}} \
                   @media (prefers-color-scheme: dark){@media (max-width:1px){.f{color:red}} .g{background:url(https://x.example/a.png)}} \
                   @media (prefers-color-scheme: dark){.h{color:red</style><script>alert(1)</script>}}";
        let out = sanitize_stylesheet(css, &none);
        assert_eq!(out.main, "");
        assert_eq!(
            out.dark,
            "@media screen and (min-width:0) and (max-width:600px){.a{background:#111!important;}}\
             @media (min-width:0){.b{color:#eee;}}\
             @media (min-width:0){.g{background:none;}}"
        );
        assert_eq!(scheme_query("print"), SchemeQuery::Plain);
        assert_eq!(scheme_query("(prefers-color-scheme)"), SchemeQuery::Drop);
        assert_eq!(
            scheme_query("(prefers-color-scheme: dark"),
            SchemeQuery::Drop
        );
    }

    #[test]
    fn dark_scheme_declarations() {
        for yes in [
            ":root{color-scheme:light dark}",
            ":root { Color-Scheme : dark; }",
            ":root{supported-color-schemes:light dark;}",
        ] {
            assert!(declares_dark_scheme(yes), "{yes}");
        }
        for no in [
            "",
            ":root{color-scheme:light}",
            ":root{color-scheme:only light}",
            "@media (prefers-color-scheme: dark){.a{color:#fff}}",
            ".color-scheme-dark{color:red}",
            ":root{color-scheme:light} .dark{color:red}",
        ] {
            assert!(!declares_dark_scheme(no), "{no}");
        }
    }

    #[test]
    fn viewport_units_detection() {
        assert!(has_viewport_height_unit("100vh"));
        assert!(has_viewport_height_unit("calc(10px + 5.5vmax)"));
        assert!(!has_viewport_height_unit("100vw"));
        assert!(!has_viewport_height_unit("solid"));
    }
}
