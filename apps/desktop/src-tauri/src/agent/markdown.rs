//! Markdown → email HTML for agent-written drafts (`format: "markdown"`).
//!
//! A small, email-shaped subset rather than a full CommonMark parser (no new
//! dependency; docs/SECURITY.md → Supply chain): paragraphs, `#` headings,
//! `-`/`*`/`+` and `1.` lists, `>` quotes, fenced code, `---` rules, and
//! inline `**bold**`, `*italic*`/`_italic_`, `` `code` `` and
//! `[text](https://…)` links. One difference from CommonMark, on purpose: a
//! single newline inside a paragraph is a line break, because that's what
//! a sign-off ("Thanks,\nAda") means in an email.
//!
//! Every character of input is escaped; the only markup in the output is
//! what this module writes, and links are http(s) or mailto only. The
//! result still goes through `penguin_render::sanitize_outgoing_html` on
//! save and send, like everything the composer writes.

/// Render `md` as an HTML fragment.
pub fn to_html(md: &str) -> String {
    let md = md.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = md.lines().collect();
    let mut out = String::with_capacity(md.len() * 3 / 2);
    blocks(&lines, &mut out);
    out
}

fn blocks(lines: &[&str], out: &mut String) {
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.is_empty() {
            i += 1;
            continue;
        }
        // Fenced code: everything up to the closing fence, verbatim.
        if let Some(fence) = fence_of(t) {
            i += 1;
            let mut code = Vec::new();
            while i < lines.len() && !lines[i].trim_start().starts_with(fence) {
                code.push(lines[i]);
                i += 1;
            }
            i += 1; // the closing fence (or the end)
            out.push_str("<pre><code>");
            out.push_str(&escape(&code.join("\n")));
            out.push_str("</code></pre>");
            continue;
        }
        if is_rule(t) {
            out.push_str("<hr>");
            i += 1;
            continue;
        }
        if let Some((level, text)) = heading(t) {
            out.push_str(&format!("<h{level}>{}</h{level}>", inline(text)));
            i += 1;
            continue;
        }
        if t.starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let l = lines[i].trim_start()[1..]
                    .strip_prefix(' ')
                    .unwrap_or(&lines[i].trim_start()[1..]);
                inner.push(l);
                i += 1;
            }
            out.push_str("<blockquote>");
            blocks(&inner, out);
            out.push_str("</blockquote>");
            continue;
        }
        if let Some(ordered) = list_item(t).map(|(o, _)| o) {
            out.push_str(if ordered { "<ol>" } else { "<ul>" });
            while i < lines.len() {
                let Some((o, text)) = list_item(lines[i].trim()) else {
                    // A non-blank line that isn't an item continues the
                    // previous item (a wrapped line).
                    if !lines[i].trim().is_empty()
                        && lines[i].starts_with([' ', '\t'])
                        && out.ends_with("</li>")
                    {
                        out.truncate(out.len() - "</li>".len());
                        out.push_str("<br>");
                        out.push_str(&inline(lines[i].trim()));
                        out.push_str("</li>");
                        i += 1;
                        continue;
                    }
                    break;
                };
                if o != ordered {
                    break;
                }
                out.push_str("<li>");
                out.push_str(&inline(text));
                out.push_str("</li>");
                i += 1;
            }
            out.push_str(if ordered { "</ol>" } else { "</ul>" });
            continue;
        }
        // A paragraph: up to a blank line or the start of another block.
        let mut para = Vec::new();
        while i < lines.len() {
            let t = lines[i].trim();
            if t.is_empty()
                || fence_of(t).is_some()
                || is_rule(t)
                || heading(t).is_some()
                || t.starts_with('>')
                || (!para.is_empty() && list_item(t).is_some())
            {
                break;
            }
            para.push(inline(t));
            i += 1;
        }
        out.push_str("<p>");
        out.push_str(&para.join("<br>"));
        out.push_str("</p>");
    }
}

fn fence_of(t: &str) -> Option<&'static str> {
    if t.starts_with("```") {
        Some("```")
    } else if t.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn is_rule(t: &str) -> bool {
    let c: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    c.len() >= 3
        && (c.chars().all(|x| x == '-')
            || c.chars().all(|x| x == '*')
            || c.chars().all(|x| x == '_'))
}

fn heading(t: &str) -> Option<(usize, &str)> {
    let level = t.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((level, rest.trim().trim_end_matches('#').trim_end()))
}

/// (ordered, text) for a list item line.
fn list_item(t: &str) -> Option<(bool, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(marker) {
            return Some((false, rest.trim()));
        }
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let rest = &t[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((true, r.trim()));
        }
    }
    None
}

pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
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
    out
}

/// Links only to the web or an address; anything else stays text.
fn safe_href(url: &str) -> Option<&str> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    let ok = (lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:"))
        && !u.chars().any(|c| c.is_whitespace() || c.is_control());
    ok.then_some(u)
}

/// Inline spans of one line. Unmatched markers stay literal.
fn inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let mut i = 0;
    let find = |from: usize, pat: &[char]| -> Option<usize> {
        (from..chars.len().saturating_sub(pat.len() - 1)).find(|&j| chars[j..j + pat.len()] == *pat)
    };
    let text = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
    while i < chars.len() {
        let c = chars[i];
        // Backslash escapes a punctuation character.
        if c == '\\' && i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() {
            out.push_str(&escape(&chars[i + 1].to_string()));
            i += 2;
            continue;
        }
        if c == '`' {
            if let Some(end) = find(i + 1, &['`']) {
                out.push_str("<code>");
                out.push_str(&escape(&text(i + 1, end)));
                out.push_str("</code>");
                i = end + 1;
                continue;
            }
        }
        if c == '[' {
            if let Some(close) = find(i + 1, &[']', '(']) {
                if let Some(end) = find(close + 2, &[')']) {
                    let label = text(i + 1, close);
                    let url = text(close + 2, end);
                    if let Some(href) = safe_href(&url) {
                        out.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            escape(href),
                            inline(&label)
                        ));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        if (c == '*' || c == '_') && chars[i..].starts_with(&[c, c]) {
            let pat = [c, c];
            if let Some(end) = find(i + 2, &pat).filter(|&e| e > i + 2) {
                out.push_str(&format!("<strong>{}</strong>", inline(&text(i + 2, end))));
                i = end + 2;
                continue;
            }
        }
        if (c == '*' || c == '_') && i + 1 < chars.len() && !chars[i + 1].is_whitespace() {
            // `_` only at a word start, so snake_case_names stay as typed.
            let word_start = i == 0 || !chars[i - 1].is_alphanumeric();
            if c == '*' || word_start {
                let close = (i + 2..chars.len()).find(|&j| {
                    chars[j] == c
                        && !chars[j - 1].is_whitespace()
                        && (c == '*' || j + 1 == chars.len() || !chars[j + 1].is_alphanumeric())
                });
                if let Some(end) = close {
                    out.push_str(&format!("<em>{}</em>", inline(&text(i + 1, end))));
                    i = end + 1;
                    continue;
                }
            }
        }
        out.push_str(&escape(&c.to_string()));
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_shaped_markdown() {
        let md = "Hi Bo,\n\nThe plan:\n\n- **Monday**: kickoff\n- *Tuesday*: review the [doc](https://docs.acme.example/p?a=1&b=2)\n\n1. one\n2. two\n\n> quoted\n> text\n\n## Next\n\n```\nlet x = <y>;\n```\n\n---\n\nThanks,\nAda";
        assert_eq!(
            to_html(md),
            "<p>Hi Bo,</p><p>The plan:</p><ul><li><strong>Monday</strong>: kickoff</li>\
             <li><em>Tuesday</em>: review the <a href=\"https://docs.acme.example/p?a=1&amp;b=2\">doc</a></li></ul>\
             <ol><li>one</li><li>two</li></ol><blockquote><p>quoted<br>text</p></blockquote>\
             <h2>Next</h2><pre><code>let x = &lt;y&gt;;</code></pre><hr><p>Thanks,<br>Ada</p>"
        );
    }

    #[test]
    fn everything_is_escaped_and_only_safe_links_become_links() {
        let html = to_html("<script>alert(1)</script> [x](javascript:alert(1)) [y](data:text/html,hi) \"q\" 'a' & b");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<a "), "{html}");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&quot;q&quot; &#39;a&#39; &amp; b"));
        let html =
            to_html("[mail me](mailto:ada@penguin.example) [x](https://a.example/\" onclick=\"x)");
        assert!(
            html.contains("<a href=\"mailto:ada@penguin.example\">mail me</a>"),
            "{html}"
        );
        // A quote can't break out of the attribute.
        assert!(!html.contains("\" onclick"), "{html}");
    }

    #[test]
    fn literal_markers_stay_literal() {
        assert_eq!(to_html("a * b * c"), "<p>a * b * c</p>");
        assert_eq!(
            to_html("snake_case_name and 2*3"),
            "<p>snake_case_name and 2*3</p>"
        );
        assert_eq!(to_html("\\*not em\\*"), "<p>*not em*</p>");
        assert_eq!(
            to_html("unclosed `code and **bold"),
            "<p>unclosed `code and **bold</p>"
        );
        assert_eq!(to_html("#hashtag"), "<p>#hashtag</p>");
        assert_eq!(to_html(""), "");
    }

    #[test]
    fn output_survives_the_outgoing_sanitizer_unchanged() {
        let html = to_html(
            "# Hi\n\n**Bold** and *it* with `code`.\n\n- a\n- b\n\n> q\n\n[l](https://a.example/)",
        );
        assert_eq!(penguin_render::sanitize_outgoing_html(&html), html);
    }
}
