//! Text helpers used for indexing. OWNER: store/search agent.
//!
//! Everything here runs on every synced message, so it is hand-rolled,
//! allocation-light and linear in the input. `html_to_text` is a streaming
//! tag skipper, not a DOM builder: for indexing we only need the visible text,
//! and a tree builder (html5ever) costs 5-10x more on large newsletters.

/// Convert an HTML email body to readable plain text for indexing/snippets
/// (drop <style>/<script>/<head>, decode entities, collapse whitespace).
pub fn html_to_text(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = TextSink::with_capacity(html.len() / 3);
    let mut i = 0;
    while i < bytes.len() {
        let lt = match memchr::memchr(b'<', &bytes[i..]) {
            Some(off) => i + off,
            None => bytes.len(),
        };
        if lt > i {
            out.push_text(&html[i..lt]);
        }
        if lt >= bytes.len() {
            break;
        }
        i = lt;
        let rest = &bytes[i..];
        if rest.starts_with(b"<!--") {
            i = find_from(bytes, i + 4, b"-->").map_or(bytes.len(), |p| p + 3);
            continue;
        }
        if rest.len() > 1 && (rest[1] == b'!' || rest[1] == b'?') {
            i = skip_tag(bytes, i + 2);
            continue;
        }
        let closing = rest.len() > 1 && rest[1] == b'/';
        let name_start = i + 1 + closing as usize;
        let mut name_end = name_start;
        while name_end < bytes.len() && bytes[name_end].is_ascii_alphanumeric() {
            name_end += 1;
        }
        if name_end == name_start {
            // A bare "<" in text ("a < b"): keep it literally.
            out.push_text("<");
            i += 1;
            continue;
        }
        let mut name_buf = [0u8; 12];
        let name = lower_name(&bytes[name_start..name_end], &mut name_buf);
        let tag_end = skip_tag(bytes, name_end);
        if !closing && is_raw_skip(name) {
            // Skip everything up to the matching close tag. A <head> that is
            // never closed must not swallow the body: resume at <body> instead.
            i = match find_close_tag(bytes, tag_end, name) {
                Some(end) => end,
                None if name == b"head" => find_ci(bytes, tag_end, b"<body").unwrap_or(tag_end),
                None => bytes.len(),
            };
            continue;
        }
        match name {
            b"br" => out.line_break(),
            b"td" | b"th" if !closing => out.space(),
            b"p" | b"div" | b"tr" | b"table" | b"blockquote" | b"ul" | b"ol" | b"li" | b"h1"
            | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" | b"pre" | b"hr" | b"section" | b"article"
            | b"header" | b"footer" | b"center" | b"dl" | b"dt" | b"dd" | b"tbody" | b"thead"
            | b"form" | b"address" | b"figure" | b"body" => {
                out.newline(if name == b"p" { 2 } else { 1 })
            }
            _ => {}
        }
        i = tag_end;
    }
    out.finish()
}

/// Strip quoted reply history ("On … wrote:", leading ">" lines, Gmail quote
/// blocks) so search ranks authored text above quoted text.
pub fn strip_quoted(text: &str) -> String {
    split_quoted(text).0
}

/// Split a plain-text body into (authored, quoted). Quoted is the reply
/// history: `>` lines plus everything from the first reply attribution
/// ("On … wrote:", "-----Original Message-----", Outlook header blocks).
/// Forwarded messages are NOT quoted: a forward's payload is usually the very
/// thing the user searches for, so it stays in the authored part.
pub fn split_quoted(text: &str) -> (String, String) {
    let lines: Vec<&str> = text.lines().collect();
    let cut = find_quote_cut(&lines);
    let mut authored = String::with_capacity(text.len());
    let mut quoted = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx >= cut {
            push_line(&mut quoted, line.trim_start_matches(['>', ' ']));
            continue;
        }
        let t = line.trim_start();
        if t.starts_with('>') {
            push_line(&mut quoted, t.trim_start_matches(['>', ' ']));
        } else {
            push_line(&mut authored, line);
        }
    }
    let a = authored.trim_end().len();
    authored.truncate(a);
    let q = quoted.trim_end().len();
    quoted.truncate(q);
    (authored, quoted)
}

fn push_line(buf: &mut String, line: &str) {
    if !buf.is_empty() {
        buf.push('\n');
    }
    buf.push_str(line);
}

const WROTE_SUFFIXES: &[&str] = &[
    "wrote:",
    "a écrit :",
    "a écrit:",
    "schrieb:",
    "escribió:",
    "ha scritto:",
    "schreef:",
    "napisał:",
    "skrev:",
    "kirjoitti:",
    "escreveu:",
];

fn find_quote_cut(lines: &[&str]) -> usize {
    for i in 0..lines.len() {
        let t = lines[i].trim();
        if t.is_empty() {
            continue;
        }
        let lower = t.to_lowercase();
        // "On Tue, Mar 3, 2026 at 10:00 AM Mike <mike@x> wrote:" (Gmail, Apple),
        // possibly wrapped over up to three lines.
        if lower.starts_with("on ")
            || lower.starts_with("le ")
            || lower.starts_with("am ")
            || lower.starts_with("el ")
            || lower.starts_with("il ")
            || lower.starts_with("op ")
        {
            for line in lines.iter().skip(i).take(3) {
                let l = line.trim().to_lowercase();
                if WROTE_SUFFIXES.iter().any(|s| l.ends_with(s))
                    || (l.contains(" schrieb ") && l.ends_with(':'))
                {
                    return i;
                }
            }
        }
        if WROTE_SUFFIXES.iter().any(|s| lower.ends_with(s)) && lower.contains('@') {
            return i;
        }
        let dashes = lower.trim_matches(|c: char| c == '-' || c == ' ');
        if dashes == "original message" && lower.contains("--") {
            return i;
        }
        // Outlook: a rule of underscores followed by a From: header block.
        if t.len() >= 10 && t.chars().all(|c| c == '_') {
            if lines
                .iter()
                .skip(i + 1)
                .take(3)
                .any(|l| starts_with_ci(l.trim(), "from:"))
            {
                return i;
            }
            continue;
        }
        // Outlook/Apple header block without a rule: From: + (Sent:|Date:) + (To:|Subject:).
        if starts_with_ci(t, "from:") && !is_forward_context(lines, i) {
            let window: Vec<&str> = lines.iter().skip(i + 1).take(5).map(|l| l.trim()).collect();
            let has_date = window
                .iter()
                .any(|l| starts_with_ci(l, "sent:") || starts_with_ci(l, "date:"));
            let has_to = window
                .iter()
                .any(|l| starts_with_ci(l, "to:") || starts_with_ci(l, "subject:"));
            if has_date && has_to {
                return i;
            }
        }
    }
    lines.len()
}

fn is_forward_context(lines: &[&str], i: usize) -> bool {
    lines[..i]
        .iter()
        .rev()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(|l| {
            let l = l.to_lowercase();
            l.contains("forwarded message") || l.starts_with("begin forwarded")
        })
        .unwrap_or(false)
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

// ---------- html internals ----------

fn is_raw_skip(name: &[u8]) -> bool {
    matches!(
        name,
        b"script" | b"style" | b"head" | b"title" | b"noscript" | b"template" | b"svg" | b"xml"
    )
}

fn lower_name<'a>(src: &[u8], buf: &'a mut [u8; 12]) -> &'a [u8] {
    let n = src.len().min(buf.len());
    for (d, s) in buf.iter_mut().zip(&src[..n]) {
        *d = s.to_ascii_lowercase();
    }
    // Names longer than the buffer are never interesting tags; the truncated
    // value cannot collide with a known name because none are 12+ bytes.
    &buf[..n]
}

/// Skip to just past the `>` closing the tag, honoring quoted attribute values.
fn skip_tag(bytes: &[u8], mut i: usize) -> usize {
    let mut quote = 0u8;
    while i < bytes.len() {
        let b = bytes[i];
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'>' {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

fn find_from(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= bytes.len() {
        return None;
    }
    memchr::memmem::find(&bytes[from..], needle).map(|p| p + from)
}

fn find_ci(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    (from..bytes.len().saturating_sub(needle.len() - 1))
        .find(|&i| bytes[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Find the end of `</name ...>` (case-insensitive) starting at `from`.
fn find_close_tag(bytes: &[u8], from: usize, name: &[u8]) -> Option<usize> {
    let mut i = from;
    while let Some(p) = find_from(bytes, i, b"</") {
        let s = p + 2;
        let e = s + name.len();
        if e <= bytes.len()
            && bytes[s..e].eq_ignore_ascii_case(name)
            && bytes.get(e).is_none_or(|b| !b.is_ascii_alphanumeric())
        {
            return Some(skip_tag(bytes, e));
        }
        i = p + 2;
    }
    None
}

/// Accumulates visible text with whitespace collapsing: runs of spaces become
/// one space, block boundaries become at most two newlines.
struct TextSink {
    out: String,
    pending_space: bool,
    pending_newlines: u8,
}

impl TextSink {
    fn with_capacity(n: usize) -> Self {
        TextSink {
            out: String::with_capacity(n),
            pending_space: false,
            pending_newlines: 0,
        }
    }

    /// Block boundary: at least `n` newlines (adjacent blocks don't stack).
    fn newline(&mut self, n: u8) {
        if !self.out.is_empty() {
            self.pending_newlines = self.pending_newlines.max(n);
        }
        self.pending_space = false;
    }

    /// `<br>`: each one counts, up to a blank line.
    fn line_break(&mut self) {
        if !self.out.is_empty() {
            self.pending_newlines = (self.pending_newlines + 1).min(2);
        }
        self.pending_space = false;
    }

    fn space(&mut self) {
        if !self.out.is_empty() && self.pending_newlines == 0 {
            self.pending_space = true;
        }
    }

    fn push_char(&mut self, c: char) {
        if c.is_whitespace() || c == '\u{a0}' {
            self.space();
            return;
        }
        if is_invisible(c) {
            return;
        }
        if self.pending_newlines > 0 {
            for _ in 0..self.pending_newlines {
                self.out.push('\n');
            }
            self.pending_newlines = 0;
        } else if self.pending_space {
            self.out.push(' ');
        }
        self.pending_space = false;
        self.out.push(c);
    }

    fn push_text(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < s.len() {
            if bytes[i] == b'&' {
                if let Some((decoded, used)) = decode_entity(&s[i..]) {
                    for c in decoded.chars() {
                        self.push_char(c);
                    }
                    i += used;
                    continue;
                }
            }
            let c = s[i..].chars().next().expect("in bounds");
            self.push_char(c);
            i += c.len_utf8();
        }
    }

    fn finish(self) -> String {
        self.out
    }
}

/// Zero-width and formatting characters marketing mail uses to pad preheaders.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{034f}' | '\u{00ad}' | '\u{2060}'..='\u{2064}' | '\u{feff}' | '\u{180e}')
}

/// Decode one entity at the start of `s` ("&amp;", "&#39;", "&#x2019;").
/// Returns the decoded text and the number of bytes consumed.
pub fn decode_entity(s: &str) -> Option<(std::borrow::Cow<'static, str>, usize)> {
    let b = s.as_bytes();
    if b.len() < 3 || b[0] != b'&' {
        return None;
    }
    if b[1] == b'#' {
        let (radix, start) = if b.len() > 2 && (b[2] == b'x' || b[2] == b'X') {
            (16, 3)
        } else {
            (10, 2)
        };
        let mut end = start;
        while end < b.len() && end - start < 8 && (b[end] as char).is_digit(radix) {
            end += 1;
        }
        if end == start {
            return None;
        }
        let n = u32::from_str_radix(&s[start..end], radix).ok()?;
        let used = if b.get(end) == Some(&b';') {
            end + 1
        } else {
            end
        };
        let c = match n {
            0 => '\u{fffd}',
            // Windows-1252 smart quotes mis-declared as numeric references.
            0x80..=0x9f => cp1252(n),
            _ => char::from_u32(n).unwrap_or('\u{fffd}'),
        };
        return Some((std::borrow::Cow::Owned(c.to_string()), used));
    }
    let mut end = 1;
    while end < b.len() && end < 33 && b[end].is_ascii_alphanumeric() {
        end += 1;
    }
    if end == 1 {
        return None;
    }
    let name = &s[1..end];
    let has_semi = b.get(end) == Some(&b';');
    let val = named_entity(name)?;
    if !has_semi && !matches!(name, "amp" | "lt" | "gt" | "quot" | "nbsp" | "copy" | "reg") {
        return None;
    }
    Some((std::borrow::Cow::Borrowed(val), end + has_semi as usize))
}

fn cp1252(n: u32) -> char {
    match n {
        0x80 => '€',
        0x82 => '‚',
        0x84 => '„',
        0x85 => '…',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201c}',
        0x94 => '\u{201d}',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x99 => '™',
        _ => '\u{fffd}',
    }
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" | "AMP" => "&",
        "lt" | "LT" => "<",
        "gt" | "GT" => ">",
        "quot" | "QUOT" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "ensp" | "emsp" | "thinsp" => " ",
        "zwnj" | "zwj" | "shy" | "lrm" | "rlm" => "",
        "copy" | "COPY" => "©",
        "reg" | "REG" => "®",
        "trade" => "™",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "sbquo" => "‚",
        "ldquo" => "\u{201c}",
        "rdquo" => "\u{201d}",
        "bdquo" => "„",
        "laquo" => "«",
        "raquo" => "»",
        "lsaquo" => "‹",
        "rsaquo" => "›",
        "bull" => "•",
        "middot" => "·",
        "deg" => "°",
        "plusmn" => "±",
        "times" => "×",
        "divide" => "÷",
        "euro" => "€",
        "pound" => "£",
        "yen" => "¥",
        "cent" => "¢",
        "sect" => "§",
        "para" => "¶",
        "dagger" => "†",
        "Dagger" => "‡",
        "permil" => "‰",
        "prime" => "′",
        "Prime" => "″",
        "frac12" => "½",
        "frac14" => "¼",
        "frac34" => "¾",
        "sup1" => "¹",
        "sup2" => "²",
        "sup3" => "³",
        "iexcl" => "¡",
        "iquest" => "¿",
        "larr" => "←",
        "rarr" => "→",
        "uarr" => "↑",
        "darr" => "↓",
        "harr" => "↔",
        "check" => "✓",
        "hearts" => "♥",
        "star" => "☆",
        "Agrave" => "À",
        "Aacute" => "Á",
        "Acirc" => "Â",
        "Atilde" => "Ã",
        "Auml" => "Ä",
        "Aring" => "Å",
        "AElig" => "Æ",
        "Ccedil" => "Ç",
        "Egrave" => "È",
        "Eacute" => "É",
        "Ecirc" => "Ê",
        "Euml" => "Ë",
        "Igrave" => "Ì",
        "Iacute" => "Í",
        "Icirc" => "Î",
        "Iuml" => "Ï",
        "ETH" => "Ð",
        "Ntilde" => "Ñ",
        "Ograve" => "Ò",
        "Oacute" => "Ó",
        "Ocirc" => "Ô",
        "Otilde" => "Õ",
        "Ouml" => "Ö",
        "Oslash" => "Ø",
        "Ugrave" => "Ù",
        "Uacute" => "Ú",
        "Ucirc" => "Û",
        "Uuml" => "Ü",
        "Yacute" => "Ý",
        "THORN" => "Þ",
        "szlig" => "ß",
        "agrave" => "à",
        "aacute" => "á",
        "acirc" => "â",
        "atilde" => "ã",
        "auml" => "ä",
        "aring" => "å",
        "aelig" => "æ",
        "ccedil" => "ç",
        "egrave" => "è",
        "eacute" => "é",
        "ecirc" => "ê",
        "euml" => "ë",
        "igrave" => "ì",
        "iacute" => "í",
        "icirc" => "î",
        "iuml" => "ï",
        "eth" => "ð",
        "ntilde" => "ñ",
        "ograve" => "ò",
        "oacute" => "ó",
        "ocirc" => "ô",
        "otilde" => "õ",
        "ouml" => "ö",
        "oslash" => "ø",
        "ugrave" => "ù",
        "uacute" => "ú",
        "ucirc" => "û",
        "uuml" => "ü",
        "yacute" => "ý",
        "thorn" => "þ",
        "yuml" => "ÿ",
        "OElig" => "Œ",
        "oelig" => "œ",
        "Scaron" => "Š",
        "scaron" => "š",
        "Yuml" => "Ÿ",
        "fnof" => "ƒ",
        "circ" => "ˆ",
        "tilde" => "˜",
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "pi" => "π",
        "mu" => "μ",
        "sigma" => "σ",
        "omega" => "ω",
        "infin" => "∞",
        "ne" => "≠",
        "le" => "≤",
        "ge" => "≥",
        "asymp" => "≈",
        "minus" => "−",
        _ => return None,
    })
}

// ---------- search-side token helpers ----------

/// Lowercase and strip common Latin diacritics, mirroring FTS5's
/// `unicode61 remove_diacritics 2` closely enough for highlighting.
pub fn fold_char(c: char) -> char {
    let c = if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    };
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' | 'ŧ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

/// Tokenize like FTS5 unicode61: maximal runs of alphanumeric characters,
/// folded. Returns (byte_start, byte_end, folded_token).
pub fn tokens(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut cur = String::new();
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            if start.is_none() {
                start = Some(i);
            }
            cur.push(fold_char(c));
        } else if let Some(s) = start.take() {
            out.push((s, i, std::mem::take(&mut cur)));
        }
    }
    if let Some(s) = start {
        out.push((s, text.len(), cur));
    }
    out
}

/// Escape text for inclusion in HTML.
pub fn escape_html(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_basic() {
        let t = html_to_text("<html><head><title>X</title><style>p{color:red}</style></head><body><p>Hello&nbsp;<b>world</b> &amp; friends</p><p>Second</p></body></html>");
        assert_eq!(t, "Hello world & friends\n\nSecond");
    }

    #[test]
    fn html_scripts_comments_and_links() {
        let t = html_to_text("<div>a<script>var x = '<p>no</p>';</script>b<!-- hidden <p> -->c</div><a href=\"https://x.example/?a=1&b=2\">Click here</a>");
        assert_eq!(t, "abc\nClick here");
    }

    #[test]
    fn html_entities() {
        let t = html_to_text(
            "It&#39;s &#x2019;quoted&rsquo; &lt;tag&gt; &euro;5 &unknown; &amp caf&eacute;",
        );
        assert_eq!(t, "It's ’quoted’ <tag> €5 &unknown; & café");
    }

    #[test]
    fn html_literal_lt_and_attrs_with_gt() {
        let t = html_to_text("<p title=\"a > b\">1 < 2</p><img alt='x>y' src=x>done");
        assert_eq!(t, "1 < 2\n\ndone");
    }

    #[test]
    fn html_tables_and_br() {
        let t = html_to_text("<table><tr><td>Invoice</td><td>INV-2041</td></tr><tr><td>Total</td><td>$40</td></tr></table>line1<br>line2<BR/>line3");
        assert_eq!(t, "Invoice INV-2041\nTotal $40\nline1\nline2\nline3");
    }

    #[test]
    fn html_invisible_preheader_chars() {
        let t = html_to_text("<div>Sale\u{200c}\u{200c}&zwnj; now\u{00ad}</div>");
        assert_eq!(t, "Sale now");
    }

    #[test]
    fn html_uppercase_and_unclosed() {
        let t = html_to_text("<HTML><STYLE>x</STYLE><P>Hi</P><script>never closed");
        assert_eq!(t, "Hi");
    }

    #[test]
    fn html_unclosed_head_keeps_body() {
        assert_eq!(
            html_to_text("<head><meta charset=utf-8><BODY><p>Visible</p>"),
            "Visible"
        );
    }

    #[test]
    fn html_large_input_is_linear() {
        let chunk = "<tr><td style=\"padding:0\"><a href=\"https://x.example\">Deal &amp; more</a></td></tr>";
        let big = chunk.repeat(20_000);
        let start = std::time::Instant::now();
        let t = html_to_text(&big);
        assert!(t.starts_with("Deal & more"));
        assert!(start.elapsed().as_millis() < 500);
    }

    #[test]
    fn quoted_gmail() {
        let body = "Sounds good, see you Friday.\n\nOn Tue, Mar 3, 2026 at 10:00 AM Mike Delgado <mike@acme.example> wrote:\n> Can we meet Friday?\n> Thanks";
        let (a, q) = split_quoted(body);
        assert_eq!(a, "Sounds good, see you Friday.");
        assert!(q.contains("Can we meet Friday?"));
        assert_eq!(strip_quoted(body), a);
    }

    #[test]
    fn quoted_wrapped_attribution() {
        let body = "Yes.\nOn Tue, Mar 3, 2026 at 10:00 AM Mike Delgado <\nmike@acme.example> wrote:\n\nold text";
        assert_eq!(strip_quoted(body), "Yes.");
    }

    #[test]
    fn quoted_apple() {
        let body = "Great\n\nSent from my iPhone\n\nOn Mar 3, 2026, at 10:00, Ana Ruiz <ana@x.example> wrote:\n\n\u{feff}Old";
        assert_eq!(strip_quoted(body), "Great\n\nSent from my iPhone");
    }

    #[test]
    fn quoted_outlook() {
        let body = "Approved.\n\n________________________________\nFrom: Priya Nair <priya@x.example>\nSent: Monday, March 2, 2026 9:14 AM\nTo: Team\nSubject: Budget\n\nPlease approve";
        assert_eq!(strip_quoted(body), "Approved.");
        let body2 = "Ok\n-----Original Message-----\nFrom: a\nold";
        assert_eq!(strip_quoted(body2), "Ok");
        let body3 = "Ok\nFrom: Priya <p@x.example>\nDate: Mon\nTo: me\n\nold";
        assert_eq!(strip_quoted(body3), "Ok");
    }

    #[test]
    fn quoted_inline_gt_lines() {
        let body = "> question one\nanswer one\n> question two\nanswer two";
        let (a, q) = split_quoted(body);
        assert_eq!(a, "answer one\nanswer two");
        assert_eq!(q, "question one\nquestion two");
    }

    #[test]
    fn forwards_stay_authored() {
        let body = "FYI\n\n---------- Forwarded message ---------\nFrom: Lease Office <office@x.example>\nDate: Mon, Mar 2, 2026\nSubject: Lease renewal\nTo: me\n\nYour lease renewal is attached.";
        assert!(strip_quoted(body).contains("Your lease renewal is attached."));
    }

    #[test]
    fn localized_attribution() {
        assert_eq!(
            strip_quoted("Merci\nLe 3 mars 2026, Ana <a@x.example> a écrit :\nvieux"),
            "Merci"
        );
        assert_eq!(
            strip_quoted("Danke\nAm 3. März 2026 schrieb Ana <a@x.example>:\n> alt"),
            "Danke"
        );
    }

    #[test]
    fn tokens_fold() {
        let t = tokens("Café INV-2041 naïve_x");
        let words: Vec<&str> = t.iter().map(|x| x.2.as_str()).collect();
        assert_eq!(words, vec!["cafe", "inv", "2041", "naive", "x"]);
        assert_eq!(&"Café INV-2041"[t[1].0..t[1].1], "INV");
    }

    #[test]
    fn escape() {
        let mut s = String::new();
        escape_html("<script>alert('x')</script>&\"", &mut s);
        assert_eq!(
            s,
            "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;&amp;&quot;"
        );
    }
}
