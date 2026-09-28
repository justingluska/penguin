//! Email → the passages that get one vector each.
//!
//! One message becomes a few short passages, never one giant vector:
//!
//! - **Chunk 0** is the subject plus the opening of the authored text; the
//!   indexer's version ([`chunk_mail`]) also names the sender and the
//!   attachments, which is how "the pdf Mike sent about the lease" finds it.
//! - **Chunks 1..** are successive windows over the rest of the authored
//!   text, overlapping a little and each starting with the subject, so a
//!   window about "the second floor" still knows it belongs to "Lease
//!   renewal – 14 Elm St".
//!
//! [`ChunkRef::chunk`](crate::ChunkRef) numbers are indexes into this cut,
//! so the indexer (which embeds the chunks, [`chunk_mail`]) and search
//! (which shows the best chunk as the result's passage, [`chunk_message`])
//! must cut the same text: `subject` as stored, and the message's *authored*
//! plain text (`penguin_core::text::strip_quoted` of the body, or the
//! snippet for headers-only mail). [`chunk_mail`] and [`chunk_message`]
//! agree on every chunk [`chunk_mail`] returns; they differ only in chunk
//! 0's sender/attachment lines and in bulk mail keeping chunk 0 alone.
//!
//! The text is cleaned of what carries no meaning for a vector: URLs
//! (reduced to their host), signature blocks after `-- `, "Sent from my
//! iPhone" (English, Spanish, Tagalog), unsubscribe and view-in-browser
//! footers, and long opaque tokens (base64, tracking ids). Windows are
//! counted in words, so the cut doesn't depend on a tokenizer; at ~1.3–2
//! tokens per English/Spanish/Tagalog word, 120 words stays inside the
//! model's 256-token limit.

/// Bump when chunking changes, so stored vectors are re-embedded.
pub const CHUNKER_VERSION: u32 = 2;

/// A final window adding fewer new words than this is merged into the
/// previous chunk.
const MIN_NEW_WORDS: usize = 8;

/// What the indexer knows about one message.
#[derive(Debug, Clone, Default)]
pub struct MailDoc<'a> {
    pub subject: &'a str,
    pub from_name: Option<&'a str>,
    pub from_email: &'a str,
    /// Quote-stripped plain text (see module docs).
    pub authored: &'a str,
    /// Names of real (non-inline) attachments.
    pub filenames: &'a [String],
    /// Newsletters, promotions, notifications: one chunk only.
    pub bulk: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ChunkConfig {
    /// Words of authored text in chunk 0 after the subject.
    pub opening_words: usize,
    /// Words per body window.
    pub window_words: usize,
    /// Words repeated between consecutive windows.
    pub overlap_words: usize,
    /// Upper bound on chunks per message (chunk 0 included).
    pub max_chunks: usize,
    /// Upper bound for bulk mail (newsletters): usually 1.
    pub max_chunks_bulk: usize,
    /// Characters of the subject kept (a runaway subject line can't eat
    /// the whole window).
    pub max_subject_chars: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        ChunkConfig {
            opening_words: 90,
            window_words: 120,
            overlap_words: 20,
            max_chunks: 8,
            max_chunks_bulk: 1,
            max_subject_chars: 200,
        }
    }
}

/// The passages of one message, chunk 0 first, as search re-cuts them to
/// show a passage: each is the subject line (when there is one), then its
/// piece of the authored text. Empty when there is no subject and no text.
pub fn chunk_message(subject: &str, authored: &str) -> Vec<String> {
    let cfg = ChunkConfig::default();
    let (subject, pieces) = cut(subject, authored, &cfg, cfg.max_chunks);
    pieces
        .into_iter()
        .map(|p| join_lines(&[&subject, &p]))
        .filter(|c| !c.is_empty())
        .collect()
}

/// The passages the indexer embeds for one message: [`chunk_message`] with
/// the sender and attachment names in chunk 0, and one chunk for bulk
/// mail. Never empty: a message with no text still gets its card.
pub fn chunk_mail(doc: &MailDoc, cfg: &ChunkConfig) -> Vec<String> {
    let max = if doc.bulk { cfg.max_chunks_bulk } else { cfg.max_chunks }.max(1);
    let (subject, mut pieces) = cut(doc.subject, doc.authored, cfg, cfg.max_chunks);
    pieces.truncate(max);
    let from = match doc.from_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) if !doc.from_email.is_empty() => {
            format!("From: {} <{}>", collapse_ws(name), doc.from_email)
        }
        Some(name) => format!("From: {}", collapse_ws(name)),
        None => format!("From: {}", doc.from_email),
    };
    let files: Vec<&str> = doc
        .filenames
        .iter()
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .take(10)
        .collect();
    let files = if files.is_empty() {
        String::new()
    } else {
        format!("Attachments: {}", files.join(", "))
    };
    if pieces.is_empty() {
        pieces.push(String::new());
    }
    pieces
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i == 0 {
                join_lines(&[&subject, &from, &files, p])
            } else {
                join_lines(&[&subject, p])
            }
        })
        .collect()
}

/// The cut both functions share: the clipped subject, and the authored
/// text of each chunk (chunk 0's opening, then the windows).
fn cut(subject: &str, authored: &str, cfg: &ChunkConfig, max: usize) -> (String, Vec<String>) {
    let subject = clip_chars(&collapse_ws(subject), cfg.max_subject_chars);
    let words = meaningful_words(authored);
    let opening = words.len().min(cfg.opening_words);
    let mut pieces = vec![words[..opening].join(" ")];
    let step = cfg.window_words.saturating_sub(cfg.overlap_words).max(1);
    // Windows start a little before the end of the opening, so the sentence
    // cut there appears whole in chunk 1.
    let mut start = opening.saturating_sub(cfg.overlap_words);
    let mut covered = opening;
    while covered < words.len() && pieces.len() < max.max(1) {
        let end = (start + cfg.window_words).min(words.len());
        // A tail that adds only a few new words joins the previous chunk
        // instead of becoming a near-duplicate window.
        if end - covered < MIN_NEW_WORDS {
            let last = pieces.last_mut().expect("chunk 0 exists");
            if !last.is_empty() {
                last.push(' ');
            }
            last.push_str(&words[covered..end].join(" "));
            break;
        }
        pieces.push(words[start..end].join(" "));
        covered = end;
        start += step;
    }
    if subject.is_empty() && pieces.len() == 1 && pieces[0].is_empty() {
        pieces.clear();
    }
    (subject, pieces)
}

fn join_lines(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// The authored text as words, minus signatures, footers, URLs and noise.
fn meaningful_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        // RFC 3676 signature separator ("-- "); trimmed it is "--".
        if line == "-- " || t == "--" || t == "—" {
            break;
        }
        if is_boilerplate_line(t) {
            continue;
        }
        for w in t.split_whitespace() {
            if let Some(w) = clean_word(w) {
                words.push(w);
            }
        }
    }
    words
}

/// Footer and client-signature lines that say nothing about the message.
fn is_boilerplate_line(t: &str) -> bool {
    if t.is_empty() {
        return true;
    }
    let lower = t.to_lowercase();
    const PREFIXES: &[&str] = &[
        "sent from my iphone",
        "sent from my ipad",
        "sent from my android",
        "sent from my samsung",
        "sent from outlook",
        "sent from mail for windows",
        "get outlook for",
        "enviado desde mi iphone",
        "enviado desde mi",
        "ipinadala mula sa",
        "view this email in your browser",
        "view in browser",
        "view it in your browser",
        "view online",
        "ver en el navegador",
        "unsubscribe",
        "to unsubscribe",
        "if you no longer wish",
        "you are receiving this",
        "you received this email because",
        "this email was sent to",
        "update your preferences",
        "manage your preferences",
        "manage preferences",
        "privacy policy",
        "cancelar suscripción",
        "darse de baja",
        "©",
        "(c) 20",
        "copyright ©",
    ];
    if PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    // Rules and decorations: "-----", "=====", "* * *", "|".
    t.chars().all(|c| !c.is_alphanumeric())
}

/// A word as the embedder should see it: URLs become their host ("zoom.us"),
/// long opaque tokens (base64, tracking ids, hashes) disappear.
fn clean_word(w: &str) -> Option<String> {
    let lower = w.to_ascii_lowercase();
    let url = ["http://", "https://", "www."]
        .iter()
        .find_map(|p| lower.find(p).map(|i| (i, *p)));
    if let Some((i, p)) = url {
        let rest = &w[i + if p == "www." { 0 } else { p.len() }..];
        let host: String = rest
            .split(['/', '?', '#', '>', ')', ']', '"', '\''])
            .next()
            .unwrap_or("")
            .trim_start_matches("www.")
            .chars()
            .take(60)
            .collect();
        let before = w[..i].trim_matches(|c: char| !c.is_alphanumeric());
        return match (before.is_empty(), host.is_empty()) {
            (true, true) => None,
            (true, false) => Some(host),
            (false, true) => Some(before.to_string()),
            (false, false) => Some(format!("{before} {host}")),
        };
    }
    let alnum = w.chars().filter(|c| c.is_alphanumeric()).count();
    if alnum == 0 && w.chars().count() > 3 {
        return None;
    }
    // Base64 blobs, tracking ids, hashes. Real words over 30 characters are
    // rare in every language we care about.
    if w.chars().count() > 30 {
        return None;
    }
    Some(w.to_string())
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc<'a>(subject: &'a str, authored: &'a str, files: &'a [String]) -> MailDoc<'a> {
        MailDoc {
            subject,
            from_name: Some("Rosa Dalisay"),
            from_email: "rosa@harbor.example",
            authored,
            filenames: files,
            bulk: false,
        }
    }

    fn words(n: usize) -> String {
        (0..n).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ")
    }

    fn small() -> ChunkConfig {
        ChunkConfig {
            opening_words: 10,
            window_words: 20,
            overlap_words: 5,
            max_chunks: 100,
            ..ChunkConfig::default()
        }
    }

    #[test]
    fn chunk0_is_subject_sender_files_and_opening() {
        let files = vec!["lease-2026.pdf".to_string(), " ".to_string()];
        let c = chunk_mail(
            &doc("Lease renewal", "Hi Sam,\nThe new lease is attached.", &files),
            &ChunkConfig::default(),
        );
        assert_eq!(
            c,
            vec!["Lease renewal\nFrom: Rosa Dalisay <rosa@harbor.example>\nAttachments: lease-2026.pdf\nHi Sam, The new lease is attached."
                .to_string()]
        );
        assert_eq!(
            chunk_message("Lease renewal", "Hi Sam,\n\nthe  renewal is attached."),
            vec!["Lease renewal\nHi Sam, the renewal is attached.".to_string()]
        );
    }

    #[test]
    fn empty_input() {
        let c = chunk_mail(&doc("", "", &[]), &ChunkConfig::default());
        assert_eq!(c, vec!["From: Rosa Dalisay <rosa@harbor.example>".to_string()]);
        assert_eq!(chunk_message("", "  "), Vec::<String>::new());
        assert_eq!(chunk_message("Only a subject", ""), vec!["Only a subject".to_string()]);
    }

    #[test]
    fn long_body_windows_overlap_and_carry_the_subject() {
        let body = words(60);
        let c = chunk_mail(&doc("Plan", &body, &[]), &small());
        // Opening w0..w9; windows start at 5, step 15: 5-24, 20-39, 35-54,
        // then 50-59 adds 5 new words and joins 35-54.
        assert!(c[0].ends_with("w9"));
        assert!(c[1].starts_with("Plan\nw5 "));
        assert!(c[1].ends_with(" w24"));
        assert!(c[2].starts_with("Plan\nw20 "));
        let last = c.last().unwrap();
        assert!(last.ends_with("w59"), "{last}");
        let all = c.join(" ");
        for i in 0..60 {
            assert!(all.contains(&format!("w{i}")), "w{i}");
        }
    }

    #[test]
    fn caps_chunks_and_bulk_gets_one() {
        let body = words(5000);
        let cfg = ChunkConfig::default();
        let c = chunk_mail(&doc("Digest", &body, &[]), &cfg);
        assert_eq!(c.len(), cfg.max_chunks);
        let mut d = doc("Digest", &body, &[]);
        d.bulk = true;
        assert_eq!(chunk_mail(&d, &cfg).len(), 1);
        // Deterministic: the indexer and search must agree on numbering.
        assert_eq!(chunk_message("Digest", &body), chunk_message("Digest", &body));
    }

    #[test]
    fn indexer_and_search_cuts_agree_on_every_chunk() {
        for n in [0usize, 5, 89, 90, 97, 98, 150, 400, 2000] {
            let body = words(n);
            let files = vec!["a.pdf".to_string()];
            let mail = chunk_mail(&doc("Topic", &body, &files), &ChunkConfig::default());
            let search = chunk_message("Topic", &body);
            assert!(mail.len() <= search.len().max(1), "n={n}");
            for (i, (m, s)) in mail.iter().zip(&search).enumerate() {
                // Same text once chunk 0's sender/attachment lines are left out.
                let m: Vec<&str> = m
                    .lines()
                    .filter(|l| !l.starts_with("From: ") && !l.starts_with("Attachments: "))
                    .collect();
                assert_eq!(m.join("\n"), *s, "n={n} chunk {i}");
            }
        }
    }

    #[test]
    fn a_short_tail_joins_the_previous_chunk() {
        let cfg = small();
        let c = chunk_mail(&doc("S", &words(40), &[]), &cfg);
        assert_eq!(c.len(), 3, "{c:?}");
        let c = chunk_mail(&doc("S", &words(30), &[]), &cfg);
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(c[1].ends_with("w24 w25 w26 w27 w28 w29"), "{}", c[1]);
        let c = chunk_mail(&doc("S", &words(13), &[]), &cfg);
        assert_eq!(c.len(), 1, "{c:?}");
        assert!(c[0].ends_with("w9 w10 w11 w12"), "{}", c[0]);
    }

    #[test]
    fn drops_signatures_footers_urls_and_blobs() {
        let body = "Can we move the call to Thursday? Link: https://zoom.example/j/123?pwd=abc\n\
                    Sent from my iPhone\n\
                    -- \n\
                    Rosa Dalisay | Harbor Realty | +1 555 0100";
        let c = chunk_mail(&doc("Call", body, &[]), &ChunkConfig::default());
        assert_eq!(
            c[0],
            "Call\nFrom: Rosa Dalisay <rosa@harbor.example>\nCan we move the call to Thursday? Link: zoom.example"
        );
        let body = "Hola equipo\n=========\nZm9vYmFyYmF6cXV4Zm9vYmFyYmF6cXV4Zm9vYmFyYmF6cXV4\nUnsubscribe from this list\nView this email in your browser";
        let c = chunk_mail(&doc("Boletín", body, &[]), &ChunkConfig::default());
        assert!(c[0].ends_with("\nHola equipo"), "{}", c[0]);
    }

    #[test]
    fn missing_name_uses_the_address_and_long_subjects_are_clipped() {
        let long = "x".repeat(500);
        let mut d = doc(&long, "", &[]);
        d.from_name = None;
        let c = chunk_mail(&d, &ChunkConfig::default());
        assert_eq!(c[0], format!("{}\nFrom: rosa@harbor.example", "x".repeat(200)));
    }
}
