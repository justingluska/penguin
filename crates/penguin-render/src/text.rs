//! Plain-text bodies → HTML: escape, linkify, fold quoted replies.
//!
//! Every byte of the input is escaped before it's emitted; the only markup
//! in the output is what this module writes itself.

use ammonia::Url;

pub(crate) fn escape(s: &str, out: &mut String) {
    // The characters to escape are ASCII, so runs between them are copied
    // whole (they end on char boundaries).
    let mut last = 0;
    for (i, b) in s.bytes().enumerate() {
        let entity = match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            b'\'' => "&#39;",
            _ => continue,
        };
        out.push_str(&s[last..i]);
        out.push_str(entity);
        last = i + 1;
    }
    out.push_str(&s[last..]);
}

fn is_url_char(c: char) -> bool {
    !c.is_whitespace() && !matches!(c, '<' | '>' | '"' | '`' | '\u{a0}') && !c.is_control()
}

/// Trim trailing punctuation that is almost always sentence punctuation, and
/// a closing bracket that has no opener inside the URL ("(see https://x.y)").
fn trim_url_end(url: &str) -> &str {
    let mut end = url.len();
    loop {
        let s = &url[..end];
        let Some(last) = s.chars().last() else { break };
        let unbalanced =
            |open: char, close: char| s.matches(close).count() > s.matches(open).count();
        let trim = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' => true,
            ')' => unbalanced('(', ')'),
            ']' => unbalanced('[', ']'),
            _ => false,
        };
        if !trim {
            break;
        }
        end -= last.len_utf8();
    }
    &url[..end]
}

fn is_email(word: &str) -> bool {
    let Some((local, domain)) = word.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local
            .chars()
            .all(|c| c.is_alphanumeric() || "._%+-".contains(c))
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
}

fn push_link(href: &str, label: &str, out: &mut String) {
    out.push_str("<a href=\"");
    escape(href, out);
    out.push_str("\" target=\"_blank\" rel=\"noopener noreferrer\">");
    escape(label, out);
    out.push_str("</a>");
}

/// Escape `text`, turning emails into mailto: links.
fn linkify_emails(text: &str, out: &mut String) {
    // Only a word with an @ can be an address.
    if !text.contains('@') {
        escape(text, out);
        return;
    }
    let mut last = 0;
    let is_word = |c: char| c.is_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-' | '@');
    let mut iter = text.char_indices().peekable();
    while let Some((start, c)) = iter.next() {
        if !is_word(c) {
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(i, c2)) = iter.peek() {
            if !is_word(c2) {
                break;
            }
            end = i + c2.len_utf8();
            iter.next();
        }
        let word = text[start..end].trim_end_matches('.');
        if is_email(word) {
            escape(&text[last..start], out);
            push_link(&format!("mailto:{word}"), word, out);
            last = start + word.len();
        }
    }
    escape(&text[last..], out);
}

/// Escape one line (or run of lines), linkifying http(s) URLs, `www.` hosts
/// and email addresses.
pub(crate) fn linkify(text: &str, out: &mut String) {
    let lower = text.to_ascii_lowercase();
    let mut last = 0;
    let mut i = 0;
    while i < text.len() {
        // Every link starts with "http" or "www.": skip to the next h or w
        // (ASCII, so a char boundary) instead of testing every position.
        let Some(skip) = lower.as_bytes()[i..]
            .iter()
            .position(|&b| b == b'h' || b == b'w')
        else {
            break;
        };
        i += skip;
        let rest = &lower[i..];
        let at_boundary = || {
            i == 0
                || !text[..i]
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric())
        };
        let scheme_len = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else if rest.starts_with("www.") && at_boundary() {
            0
        } else {
            i += 1;
            continue;
        };
        let raw_end = text[i..]
            .find(|c: char| !is_url_char(c))
            .map_or(text.len(), |e| i + e);
        let candidate = trim_url_end(&text[i..raw_end]);
        let href = if scheme_len == 0 {
            format!("https://{candidate}")
        } else {
            candidate.to_string()
        };
        let valid = candidate.len() > scheme_len + 3
            && Url::parse(&href)
                .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
        if !valid {
            i += 1;
            continue;
        }
        linkify_emails(&text[last..i], out);
        push_link(&href, candidate, out);
        i += candidate.len();
        last = i;
    }
    linkify_emails(&text[last..], out);
}

fn quote_depth_strip(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let rest = t.strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// An attribution line that introduces a quote ("On Tue, Ada wrote:").
fn is_attribution(line: &str) -> bool {
    let t = line.trim();
    // Every form below ends with a colon; most lines don't.
    if !t.ends_with(':') {
        return false;
    }
    let l = t.to_lowercase();
    (l.ends_with("wrote:")
        || l.ends_with("a écrit :")
        || l.ends_with("schrieb:")
        || l.ends_with("escribió:"))
        && t.len() < 300
}

fn is_forward_marker(line: &str) -> bool {
    let t = line.trim().trim_matches('-').trim().to_ascii_lowercase();
    t == "original message" || t == "forwarded message"
}

fn push_fold(inner_html: &str, out: &mut String) {
    out.push_str("<details class=\"pg-quote\"><summary title=\"Show quoted text\">\u{2026}</summary><blockquote>");
    out.push_str(inner_html);
    out.push_str("</blockquote></details>");
}

/// Render plain text: unquoted runs stay as text, each run of `>` lines is
/// folded (recursively, so nested quotes fold inside the outer fold).
pub(crate) fn render_body(text: &str, depth: usize, out: &mut String) {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut plain: Vec<&str> = Vec::new();
    // Blank lines next to a fold would render as extra gaps around the
    // (block-level) <details>, so they're trimmed at chunk edges.
    let flush = |plain: &mut Vec<&str>, out: &mut String| {
        while plain.last().is_some_and(|l| l.trim().is_empty()) {
            plain.pop();
        }
        if !plain.is_empty() {
            linkify(&plain.join("\n"), out);
        }
        plain.clear();
    };
    let mut after_fold = false;
    let mut i = 0;
    while i < lines.len() {
        // "-----Original Message-----" (Outlook style, not `>`-quoted): fold
        // everything after it.
        if depth == 0 && lines[i].trim().starts_with('-') && is_forward_marker(lines[i]) {
            flush(&mut plain, out);
            let mut inner = String::new();
            linkify(&lines[i..].join("\n"), &mut inner);
            push_fold(&inner, out);
            return;
        }
        let starts_quote = quote_depth_strip(lines[i]).is_some();
        let attribution_then_quote = is_attribution(lines[i])
            && lines[i + 1..]
                .iter()
                .find(|l| !l.trim().is_empty())
                .is_some_and(|l| quote_depth_strip(l).is_some());
        if !starts_quote && !attribution_then_quote {
            if !(after_fold && plain.is_empty() && lines[i].trim().is_empty()) {
                plain.push(lines[i]);
            }
            i += 1;
            continue;
        }
        flush(&mut plain, out);
        let mut inner = String::new();
        if attribution_then_quote {
            inner.push_str("<div class=\"pg-attribution\">");
            linkify(lines[i], &mut inner);
            inner.push_str("</div>");
            i += 1;
            while i < lines.len() && lines[i].trim().is_empty() {
                i += 1;
            }
        }
        let mut quoted = Vec::new();
        while i < lines.len() {
            if let Some(q) = quote_depth_strip(lines[i]) {
                quoted.push(q);
                i += 1;
            } else if lines[i].trim().is_empty()
                && lines
                    .get(i + 1)
                    .is_some_and(|l| quote_depth_strip(l).is_some())
            {
                quoted.push("");
                i += 1;
            } else {
                break;
            }
        }
        if depth < 32 {
            render_body(&quoted.join("\n"), depth + 1, &mut inner);
        } else {
            linkify(&quoted.join("\n"), &mut inner);
        }
        push_fold(&inner, out);
        after_fold = true;
    }
    flush(&mut plain, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lk(s: &str) -> String {
        let mut o = String::new();
        linkify(s, &mut o);
        o
    }

    #[test]
    fn linkifies_and_escapes() {
        assert_eq!(
            lk("see https://a.example/x?y=1&z=2. <b>"),
            "see <a href=\"https://a.example/x?y=1&amp;z=2\" target=\"_blank\" rel=\"noopener noreferrer\">https://a.example/x?y=1&amp;z=2</a>. &lt;b&gt;"
        );
        assert_eq!(
            lk("(www.a.example/p)"),
            "(<a href=\"https://www.a.example/p\" target=\"_blank\" rel=\"noopener noreferrer\">www.a.example/p</a>)"
        );
        assert_eq!(
            lk("mail ada@lovelace.example."),
            "mail <a href=\"mailto:ada@lovelace.example\" target=\"_blank\" rel=\"noopener noreferrer\">ada@lovelace.example</a>."
        );
        assert_eq!(lk("javascript:alert(1)"), "javascript:alert(1)");
        assert_eq!(
            lk("http://\"onmouseover=alert(1)"),
            "http://&quot;onmouseover=alert(1)"
        );
    }
}
