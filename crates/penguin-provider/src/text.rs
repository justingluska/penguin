//! Text helpers every provider's conversion needs.

/// Characters in a message snippet (what the list shows under a subject).
pub const SNIPPET_CHARS: usize = 200;

/// The first [`SNIPPET_CHARS`] characters of `text`, whitespace collapsed
/// to single spaces. Reads only as far as it needs: bodies can be
/// megabytes, and this runs on every synced message.
pub fn snippet(text: &str) -> String {
    let mut out = String::with_capacity(SNIPPET_CHARS + 8);
    let mut n = 0;
    'words: for word in text.split_whitespace() {
        if n > 0 {
            if n == SNIPPET_CHARS {
                break;
            }
            out.push(' ');
            n += 1;
        }
        for c in word.chars() {
            if n == SNIPPET_CHARS {
                break 'words;
            }
            out.push(c);
            n += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_collapses_whitespace_and_stops_at_200_characters() {
        // What the snippet always was: every word joined by one space, cut.
        let reference = |t: &str| -> String {
            t.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(SNIPPET_CHARS)
                .collect()
        };
        let long_word = "é".repeat(250);
        let exact = format!("{} {}", "a".repeat(198), "b");
        let space_at_200 = format!("{} {}", "a".repeat(199), "tail");
        let words = "word ".repeat(100);
        let nbsp = "x\u{00a0}y ".repeat(80);
        for t in [
            "",
            "   ",
            "  Café\r\n\tnumbers   inside  ",
            long_word.as_str(),
            exact.as_str(),
            space_at_200.as_str(),
            words.as_str(),
            nbsp.as_str(),
        ] {
            assert_eq!(snippet(t), reference(t), "{t:?}");
        }
    }
}
