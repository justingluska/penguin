//! Structured extraction (penguin-core `structured`) scored against the
//! eval corpus's own ground truth. The corpus was written independently of
//! the extractors: its receipts, shipping notices, bills, invoices, flights
//! and hotel bookings carry tags with the order number, amount, tracking
//! number, booking code or confirmation number, and 35–40% of receipts and
//! flights carry schema.org JSON-LD. Everything else in the mailbox (work
//! threads, newsletters, promotions, codes, invites…) should yield no
//! booking/order/bill facts.
//!
//!   cargo test -p penguin-eval --release --test extraction -- --nocapture

use std::collections::HashMap;

use penguin_core::structured::{extract, Extracted, MailInput, Source};
use penguin_eval::corpus::Corpus;

/// Whether a fact states the ground truth (None = not the kind asked for).
type Check = Box<dyn Fn(&Extracted) -> Option<bool>>;

#[derive(Default)]
struct Score {
    expected: usize,
    correct: usize,
    wrong: usize,
    missed: usize,
    spurious: usize,
    from_markup: usize,
}

fn tag<'a>(tags: &'a std::collections::BTreeSet<String>, key: &str) -> Option<&'a str> {
    tags.iter().find_map(|t| t.strip_prefix(&format!("{key}:")))
}

fn amount(s: &str) -> Option<f64> {
    s.replace(',', "").parse().ok()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.005
}

#[test]
fn extraction_against_the_corpus_ground_truth() {
    let c = Corpus::generate(42, chrono::Utc::now().timestamp_millis());
    let mine: Vec<String> = c
        .messages
        .iter()
        .filter(|m| m.label_ids.iter().any(|l| l == "SENT"))
        .map(|m| m.from.email.to_lowercase())
        .collect();
    // Facts per conversation.
    let mut facts: HashMap<(String, String), Vec<(Extracted, Source)>> = HashMap::new();
    let started = std::time::Instant::now();
    for m in &c.messages {
        let (authored, _) = penguin_core::text::split_quoted(&m.body_text);
        let input = MailInput {
            subject: &m.subject,
            from_email: &m.from.email,
            from_name: m.from.name.as_deref(),
            date: m.date,
            text: &authored,
            html: m.body_html.as_deref(),
            bulk: m.list_unsubscribe.is_some()
                || m.label_ids.iter().any(|l| {
                    matches!(
                        l.as_str(),
                        "CATEGORY_PROMOTIONS"
                            | "CATEGORY_UPDATES"
                            | "CATEGORY_FORUMS"
                            | "CATEGORY_SOCIAL"
                    )
                }),
            sent: m.label_ids.iter().any(|l| l == "SENT" || l == "DRAFT")
                || mine.contains(&m.from.email.to_lowercase()),
        };
        for f in extract(&input) {
            facts
                .entry((m.account_id.clone(), m.thread_id.clone()))
                .or_default()
                .push((f.fact, f.source));
        }
    }
    let secs = started.elapsed().as_secs_f64();

    let mut score: HashMap<&'static str, Score> = HashMap::new();
    let mut contacts = 0usize;
    // A few examples of each kind of error, for reading (PENGUIN_EXTRACT_EXAMPLES=1).
    let show = std::env::var("PENGUIN_EXTRACT_EXAMPLES").is_ok();
    let mut examples: HashMap<String, usize> = HashMap::new();
    let mut example = |what: String, subject: &str, got: &[&(Extracted, Source)]| {
        let n = examples.entry(what.clone()).or_default();
        *n += 1;
        if show && *n <= 3 {
            println!(
                "{what}: {subject:?}\n    {:?}",
                got.iter().map(|(f, _)| f).collect::<Vec<_>>()
            );
        }
    };
    for t in &c.threads {
        let got = facts
            .get(&(t.account_id.clone(), t.thread_id.clone()))
            .cloned()
            .unwrap_or_default();
        let kind = tag(&t.tags, "kind").unwrap_or("");
        // What a reader would write down for this conversation.
        let (name, check): (&'static str, Option<Check>) = match kind {
            "receipt" => {
                let num = tag(&t.tags, "order").unwrap_or("").to_string();
                let amt = tag(&t.tags, "amount").and_then(amount);
                // The amount when the corpus knows it, and the order number.
                // Tickets bought are an event booking with the order number.
                let ok_amt = move |m: Option<&penguin_core::structured::Money>| match amt {
                    Some(a) => m.is_some_and(|m| close(m.value, a)),
                    None => true,
                };
                (
                    "receipt → order",
                    Some(Box::new(move |f: &Extracted| match f {
                        Extracted::Order(o) => Some(
                            ok_amt(o.total.as_ref())
                                && o.order_number
                                    .as_deref()
                                    .is_none_or(|n| n.eq_ignore_ascii_case(&num)),
                        ),
                        Extracted::Reservation(r) => Some(
                            ok_amt(r.total.as_ref())
                                && r.confirmation
                                    .as_deref()
                                    .is_some_and(|n| n.eq_ignore_ascii_case(&num)),
                        ),
                        _ => None,
                    })),
                )
            }
            "shipping" => {
                let tr = tag(&t.tags, "tracking").unwrap_or("").to_string();
                (
                    "shipping → shipment",
                    Some(Box::new(move |f: &Extracted| match f {
                        Extracted::Shipment(s) => Some(
                            s.tracking_number
                                .as_deref()
                                .is_some_and(|n| n.eq_ignore_ascii_case(&tr)),
                        ),
                        _ => None,
                    })),
                )
            }
            "bill" => {
                let amt = tag(&t.tags, "amount").and_then(amount);
                // The amount when the corpus knows it.
                let ok = move |m: Option<&penguin_core::structured::Money>| match amt {
                    Some(a) => m.is_some_and(|m| close(m.value, a)),
                    None => m.is_some(),
                };
                (
                    "bill → bill or receipt",
                    Some(Box::new(move |f: &Extracted| match f {
                        Extracted::Bill(b) => Some(ok(b.amount_due.as_ref())),
                        Extracted::Order(o) => Some(ok(o.total.as_ref())),
                        _ => None,
                    })),
                )
            }
            "invoice" if t.has("direction:in") => {
                let num = tag(&t.tags, "invoice").unwrap_or("").to_string();
                (
                    "invoice received → bill",
                    Some(Box::new(move |f: &Extracted| match f {
                        Extracted::Bill(b) => Some(
                            b.invoice_number
                                .as_deref()
                                .is_some_and(|n| n.eq_ignore_ascii_case(&num)),
                        ),
                        _ => None,
                    })),
                )
            }
            "flight" => {
                let pnr = tag(&t.tags, "pnr").unwrap_or("").to_string();
                (
                    "flight → flight",
                    Some(Box::new(move |f: &Extracted| match f {
                        Extracted::Flight(x) => Some(
                            x.confirmation
                                .as_deref()
                                .is_some_and(|n| n.eq_ignore_ascii_case(&pnr))
                                && x.flight_number.is_some()
                                && x.arrive_airport.is_some(),
                        ),
                        _ => None,
                    })),
                )
            }
            "hotel" => {
                let conf = tag(&t.tags, "hotelconf").unwrap_or("").to_string();
                (
                    "hotel → stay",
                    Some(Box::new(move |f: &Extracted| match f {
                        Extracted::Lodging(l) => Some(
                            l.confirmation
                                .as_deref()
                                .is_some_and(|n| n.eq_ignore_ascii_case(&conf)),
                        ),
                        _ => None,
                    })),
                )
            }
            _ => ("everything else (should be none)", None),
        };
        let s = score.entry(name).or_default();
        contacts += got
            .iter()
            .filter(|(f, _)| matches!(f, Extracted::Contact(_)))
            .count();
        let facts: Vec<&(Extracted, Source)> = got
            .iter()
            .filter(|(f, _)| !matches!(f, Extracted::Contact(_)))
            .collect();
        match check {
            None => {
                if !facts.is_empty() {
                    example(format!("spurious in {kind}"), &t.subject, &facts);
                }
                s.spurious += facts.len()
            }
            Some(check) => {
                s.expected += 1;
                let verdicts: Vec<(bool, Source)> = facts
                    .iter()
                    .filter_map(|(f, src)| check(f).map(|v| (v, *src)))
                    .collect();
                if let Some((_, src)) = verdicts.iter().find(|(v, _)| *v) {
                    s.correct += 1;
                    if *src != Source::Pattern {
                        s.from_markup += 1;
                    }
                } else if verdicts.is_empty() {
                    s.missed += 1;
                    example(format!("missed {name}"), &t.subject, &facts);
                } else {
                    s.wrong += 1;
                    example(format!("wrong {name}"), &t.subject, &facts);
                }
                // Other kinds of fact in this conversation are spurious.
                let other: Vec<&(Extracted, Source)> = facts
                    .iter()
                    .copied()
                    .filter(|(f, _)| check(f).is_none())
                    .collect();
                if !other.is_empty() {
                    example(format!("other facts in {name}"), &t.subject, &other);
                }
                s.spurious += other.len();
            }
        }
    }
    println!(
        "\nExtraction on the eval corpus: {} messages in {:.2}s ({:.0} messages/s)\n",
        c.messages.len(),
        secs,
        c.messages.len() as f64 / secs
    );
    println!("| ground truth | conversations | correct | wrong | missed | other facts | precision | recall | from markup |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    let mut names: Vec<&&str> = score.keys().collect();
    names.sort();
    let (mut tc, mut tw, mut tm, mut ts, mut te) = (0, 0, 0, 0, 0);
    for n in names {
        let s = &score[*n];
        let p = s.correct as f64 / (s.correct + s.wrong + s.spurious).max(1) as f64;
        let r = s.correct as f64 / s.expected.max(1) as f64;
        println!(
            "| {n} | {} | {} | {} | {} | {} | {:.3} | {} | {} |",
            s.expected,
            s.correct,
            s.wrong,
            s.missed,
            s.spurious,
            p,
            if s.expected > 0 {
                format!("{r:.3}")
            } else {
                "—".into()
            },
            s.from_markup
        );
        tc += s.correct;
        tw += s.wrong;
        tm += s.missed;
        ts += s.spurious;
        te += s.expected;
    }
    let p = tc as f64 / (tc + tw + ts).max(1) as f64;
    let r = tc as f64 / te.max(1) as f64;
    println!("| **all** | {te} | {tc} | {tw} | {tm} | {ts} | {p:.3} | {r:.3} | |");
    println!("\n(contact details found in {contacts} conversations; not scored: the corpus has no ground truth for signatures)");
    // A floor, not a target: this test is the measurement.
    assert!(p >= 0.8, "precision {p:.3}");
}
