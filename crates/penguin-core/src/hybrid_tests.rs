//! Hybrid ranking tests, with the `penguin-semantic` stand-ins: HashEmbedder
//! (a hashed bag of words, so "meaning" here is shared words without the
//! keyword index's all-words rule) and FlatIndex (exact search).

use std::sync::Arc;

use penguin_semantic::{chunk_message, ChunkRef, Embedder, FlatIndex, HashEmbedder, VectorIndex};

use crate::hybrid::{self, RouteKind, SemanticHandles};
use crate::types::*;
use crate::Store;

const DAY: i64 = 86_400_000;
const ME: &str = "me@home.example";

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn addr(name: &str, email: &str) -> Address {
    Address {
        name: Some(name.to_string()),
        email: email.to_string(),
    }
}

fn msg(id: &str, thread: &str, days_ago: i64, from: (&str, &str), subject: &str, body: &str) -> Message {
    Message {
        account_id: ME.into(),
        id: id.into(),
        thread_id: thread.into(),
        date: now() - days_ago * DAY,
        from: addr(from.0, from.1),
        to: vec![addr("Sam Okafor", ME)],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: subject.into(),
        snippet: body.chars().take(80).collect(),
        body_text: body.into(),
        body_html: None,
        label_ids: vec!["INBOX".into()],
        attachments: vec![],
        message_id_header: Some(format!("<{id}@mail.example>")),
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: false,
    }
}

fn with_labels(mut m: Message, labels: &[&str]) -> Message {
    m.label_ids = labels.iter().map(|s| s.to_string()).collect();
    m
}

fn newsletter(mut m: Message) -> Message {
    m.list_unsubscribe = Some("<mailto:unsub@deals.example>".into());
    m
}

const ANA: (&str, &str) = ("Ana Ruiz", "ana@ruiz.example");
const MIKE: (&str, &str) = ("Mike Delgado", "mike@realty.example");
const DEALS: (&str, &str) = ("Travel Deals", "news@deals.example");

fn corpus() -> Vec<Message> {
    vec![
        msg(
            "wifi",
            "t-wifi",
            40,
            ANA,
            "Cabin weekend",
            "Hi Sam! The wifi password for the cabin is on the fridge: pinecone42. Checkout is at 11.",
        ),
        msg(
            "wifi-mike",
            "t-wifi-mike",
            20,
            MIKE,
            "Unit 3B internet",
            "The building wifi changed last week; ask the front desk.",
        ),
        msg(
            "invoice",
            "t-invoice",
            60,
            MIKE,
            "Invoice INV-20417",
            "Please find invoice INV-20417 for the March repairs attached.",
        ),
        msg(
            "invoice-other",
            "t-invoice-other",
            5,
            MIKE,
            "Repairs estimate",
            "An estimate for the March repairs invoice will follow; no number yet.",
        ),
        msg(
            "lease",
            "t-lease",
            90,
            MIKE,
            "Lease renewal",
            "Your lease renewal for Unit 3B is attached. Rent stays the same.",
        ),
        newsletter(with_labels(
            msg(
                "deals",
                "t-deals",
                1,
                DEALS,
                "Cabin getaways",
                "Cabin getaways this fall, from $99 a night. Book now.",
            ),
            &["INBOX", "CATEGORY_PROMOTIONS"],
        )),
        with_labels(
            msg(
                "trashed",
                "t-trashed",
                2,
                ANA,
                "Old cabin note",
                "Old wifi password for the cabin, no longer valid.",
            ),
            &["TRASH"],
        ),
    ]
}

struct Fixture {
    store: Store,
    handles: SemanticHandles,
}

/// Index every message the way the indexer does: chunk the authored body,
/// embed each chunk as a passage.
fn index(store_msgs: &[Message], embedder: &dyn Embedder, idx: &dyn VectorIndex) {
    for m in store_msgs {
        let authored = crate::text::strip_quoted(&m.body_text);
        let chunks = chunk_message(&m.subject, &authored);
        let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
        for (i, v) in embedder.embed_passages(&refs).unwrap().into_iter().enumerate() {
            idx.upsert(
                ChunkRef {
                    account_id: m.account_id.clone(),
                    thread_id: m.thread_id.clone(),
                    message_id: m.id.clone(),
                    chunk: i as u32,
                    date: m.date,
                },
                v,
            )
            .unwrap();
        }
    }
}

fn fixture(msgs: Vec<Message>) -> Fixture {
    let store = Store::open_in_memory().unwrap();
    store
        .upsert_account(&Account {
            id: ME.into(),
            email: ME.into(),
            display_name: Some("Sam Okafor".into()),
            nickname: None,
            color: "#123456".into(),
            added_at: 1,
            ..Account::default()
        })
        .unwrap();
    store.upsert_messages(&msgs).unwrap();
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(512));
    let idx = Arc::new(FlatIndex::default());
    index(&msgs, embedder.as_ref(), idx.as_ref());
    Fixture {
        store,
        handles: SemanticHandles {
            embedder,
            index: idx,
            progress: None,
        },
    }
}

fn req(q: &str) -> SearchRequest {
    SearchRequest {
        query: q.into(),
        account_id: None,
        account_ids: None,
        limit: 50,
    }
}

impl Fixture {
    fn keyword(&self, q: &str) -> SearchResponse {
        self.store.search(&req(q)).unwrap()
    }
    fn hybrid(&self, q: &str) -> SearchResponse {
        self.store.search_hybrid(&req(q), Some(&self.handles)).unwrap()
    }
}

fn ids(r: &SearchResponse) -> Vec<&str> {
    r.hits.iter().map(|h| h.message_id.as_str()).collect()
}

/// The ranking a user sees: ids, scores, snippets, tags.
fn page(r: &SearchResponse) -> Vec<(String, String, Vec<String>, Option<String>)> {
    r.hits
        .iter()
        .map(|h| {
            (
                h.message_id.clone(),
                h.snippet_html.clone(),
                h.matched_by.clone(),
                h.passage.clone(),
            )
        })
        .collect()
}

#[test]
fn without_an_index_search_is_the_keyword_search_and_while_indexing_it_uses_what_is_embedded() {
    let f = fixture(corpus());
    for q in ["wifi password", "cabin wifi password please", "INV-20417", "from:mike", "lease"] {
        let kw = f.keyword(q);
        assert_eq!(kw.semantic, SemanticStatus::Off);
        assert_eq!(kw.semantic_progress, None);
        let none = f.store.search_hybrid(&req(q), None).unwrap();
        assert_eq!(page(&none), page(&kw), "{q}");
        assert_eq!(none.semantic, SemanticStatus::Off);
        let building = SemanticHandles {
            progress: Some(0.4),
            ..f.handles.clone()
        };
        let r = f.store.search_hybrid(&req(q), Some(&building)).unwrap();
        // The partial index is used as it stands (here it happens to hold
        // everything), so the page is the ready page; only the status differs.
        let ready = f.store.search_hybrid(&req(q), Some(&f.handles)).unwrap();
        assert_eq!(page(&r), page(&ready), "{q}");
        assert_eq!(r.semantic, SemanticStatus::Indexing);
        assert_eq!(r.semantic_progress, Some(0.4));
    }
    // Keyword hits say they matched by words; pure filters by nothing.
    assert_eq!(f.keyword("lease").hits[0].matched_by, vec!["words"]);
    assert!(f.keyword("from:mike").hits.iter().all(|h| h.matched_by.is_empty()));
}

#[test]
fn a_descriptive_query_finds_mail_the_keyword_index_misses() {
    let f = fixture(corpus());
    let q = "that cabin wifi password thing";
    // "thing" is in no message, so the all-words keyword search is empty.
    assert!(f.keyword(q).hits.is_empty());
    let r = f.hybrid(q);
    assert_eq!(r.semantic, SemanticStatus::Ready);
    assert_eq!(r.hits[0].message_id, "wifi", "{:?}", ids(&r));
    let top = &r.hits[0];
    assert_eq!(top.matched_by, vec!["meaning"]);
    let passage = top.passage.as_deref().unwrap();
    assert!(passage.contains("wifi password for the cabin"), "{passage}");
    assert!(passage.chars().count() <= hybrid::PASSAGE_CHARS);
    // The passage is the snippet, escaped, with the typed words marked.
    assert!(top.snippet_html.contains("<mark>wifi</mark>"), "{}", top.snippet_html);
    assert!(!top.snippet_html.contains("Cabin weekend"), "subject not repeated");
    // Trash stays out of meaning results as it does out of keyword results.
    assert!(!ids(&r).contains(&"trashed"));
}

#[test]
fn filters_and_exclusions_apply_to_the_meaning_side() {
    let f = fixture(corpus());
    let r = f.hybrid("from:ana that cabin wifi password thing");
    assert_eq!(ids(&r), vec!["wifi"]);
    let r = f.hybrid("that cabin wifi password thing -fridge");
    assert!(!ids(&r).contains(&"wifi"), "{:?}", ids(&r));
    let r = f.hybrid("that cabin wifi password thing in:trash");
    assert_eq!(ids(&r), vec!["trashed"]);
    let r = f.hybrid("that cabin wifi password thing before:2000-01-01");
    assert!(r.hits.is_empty());
    let r = f.hybrid("that cabin wifi password thing is:newsletter");
    assert_eq!(ids(&r), vec!["deals"]);
}

#[test]
fn exact_syntax_runs_exactly_as_before() {
    let f = fixture(corpus());
    for q in [
        "\"wifi password\"",
        "wifi OR invoice",
        "(cabin OR lease) password",
        "from:mike",
        "is:unread",
        "has:attachment",
    ] {
        assert_eq!(page(&f.hybrid(q)), page(&f.keyword(q)), "{q}");
    }
}

#[test]
fn codes_and_names_stay_keyword_led() {
    let f = fixture(corpus());
    let r = f.hybrid("INV-20417");
    assert_eq!(r.hits[0].message_id, "invoice");
    assert!(r.hits[0].matched_by.contains(&"words".to_string()));
    // Only the message holding the code matches it by words; anything the
    // meaning side adds ranks below.
    for h in &r.hits[1..] {
        assert!(!h.matched_by.contains(&"words".to_string()));
    }
}

#[test]
fn a_hit_found_both_ways_says_so() {
    let f = fixture(corpus());
    let r = f.hybrid("lease renewal rent");
    let top = &r.hits[0];
    assert_eq!(top.message_id, "lease");
    assert_eq!(top.matched_by, vec!["words", "meaning"]);
    // Matched by words: the keyword snippet stays; the passage is filled.
    assert!(top.snippet_html.contains("<mark>"));
    assert!(top.passage.is_some());
}

#[test]
fn bulk_mail_found_only_by_meaning_ranks_below_personal_mail() {
    // Same text, so the same similarity; the newsletter is also newer.
    let text = "Late checkout is possible and the hot tub opens at noon.";
    let f = fixture(vec![
        msg("personal", "t-personal", 30, ANA, "Cabin", text),
        newsletter(with_labels(
            msg("bulk", "t-bulk", 1, DEALS, "Cabin", text),
            &["INBOX", "CATEGORY_PROMOTIONS"],
        )),
    ]);
    let r = f.hybrid("cabin hot tub timing thing");
    assert_eq!(ids(&r), vec!["personal", "bulk"]);
}

#[test]
fn the_model_is_asked_about_the_words_not_the_operators() {
    struct Recording {
        inner: HashEmbedder,
        seen: std::sync::Mutex<Vec<String>>,
    }
    impl Embedder for Recording {
        fn model_id(&self) -> &str {
            "recording-operators-test"
        }
        fn dims(&self) -> usize {
            self.inner.dims()
        }
        fn embed_query(&self, text: &str) -> penguin_semantic::Result<Vec<f32>> {
            self.seen.lock().unwrap().push(text.to_string());
            self.inner.embed_query(text)
        }
        fn embed_passages(&self, texts: &[&str]) -> penguin_semantic::Result<Vec<Vec<f32>>> {
            self.inner.embed_passages(texts)
        }
    }
    let f = fixture(corpus());
    let rec = Arc::new(Recording {
        inner: HashEmbedder::new(512),
        seen: Default::default(),
    });
    let handles = SemanticHandles {
        embedder: rec.clone(),
        ..f.handles.clone()
    };
    let r = f
        .store
        .search_hybrid(&req("from:ana that cabin wifi password thing -fridge"), Some(&handles))
        .unwrap();
    assert_eq!(*rec.seen.lock().unwrap(), vec!["that cabin wifi password thing"]);
    // The chips are still there for the UI.
    assert_eq!(r.chips.len(), 2, "{:?}", r.chips);
}

#[test]
fn misspelled_words_are_respelled_from_the_index() {
    let f = fixture(corpus());
    let rw = |q: &str| f.store.read(|c| hybrid::rewrite(c, q, now())).unwrap();
    let r = rw("cabn wifi pasword ");
    assert_eq!(r.raw, "cabin wifi password ");
    // Both corrections occur together with "wifi", so the model gets them
    // too. Alone, a correction has no context to confirm it: the model gets
    // the word as typed (it may be a real word the mail doesn't use).
    assert_eq!(r.embed_raw, "cabin wifi password ");
    let alone = rw("pasword ");
    assert_eq!((alone.raw.as_str(), alone.embed_raw.as_str()), ("password ", "pasword "));
    assert_eq!(
        r.respelled,
        vec![("cabn".to_string(), "cabin".to_string()), ("pasword".into(), "password".into())]
    );
    // Words that exist, quotes, operators, codes and short words stay.
    for q in ["wifi password", "\"wifi pasword\"", "from:mikee lease", "INV-2041", "cbn wifi"] {
        assert_eq!(rw(q).raw, q, "{q}");
    }
    // A word still being typed counts when some term starts with it.
    assert_eq!(rw("wifi passw").raw, "wifi passw");
    // A word the mail writes joined with its neighbour is not a typo ("sour
    // dough" is sourdough, not "your rough"): the lexicon joins it.
    assert_eq!(rw("cabin pass word ").raw, "cabin pass word ");
    assert_eq!(f.keyword("pass word ").hits[0].message_id, "wifi");
    // Both find the message: keyword search corrects a word no mail
    // contains too (lexicon.rs), with or without meaning.
    assert_eq!(f.keyword("cabin wifi pasword ").hits[0].message_id, "wifi");
    assert_eq!(f.hybrid("cabin wifi pasword ").hits[0].message_id, "wifi");
}

#[test]
fn a_date_in_words_ranks_instead_of_filtering() {
    let n = now();
    let spring = crate::query::parse("date:\"last spring\"", n);
    let mid = (spring.after.unwrap() + spring.before.unwrap()) / 2;
    let text = "Your cleaning with Dr. Park is confirmed.";
    let mut then = msg("then", "t-then", 0, ANA, "Dentist", text);
    then.date = mid;
    let recent = msg("recent", "t-recent", 1, ANA, "Dentist", text);
    let f = fixture(vec![then, recent]);
    let rw = f.store.read(|c| hybrid::rewrite(c, "dentist cleaning last spring", n)).unwrap();
    assert_eq!(rw.raw, "dentist cleaning");
    assert_eq!(
        rw.soft_date,
        Some(hybrid::SoftDate {
            after: spring.after,
            before: spring.before
        })
    );
    // Keyword search needs the words "last" and "spring": nothing.
    assert!(f.keyword("dentist cleaning last spring").hits.is_empty());
    // Hybrid: both, the one from last spring first although older.
    assert_eq!(ids(&f.hybrid("dentist cleaning last spring")), vec!["then", "recent"]);
    // A date alone, or as an operator, is left to the parser.
    for q in ["last spring", "dentist date:\"last spring\""] {
        let r = f.store.read(|c| hybrid::rewrite(c, q, n)).unwrap();
        assert_eq!((r.raw.as_str(), r.soft_date), (q, None));
    }
}

#[test]
fn a_failing_model_falls_back_to_keywords() {
    struct Broken;
    impl Embedder for Broken {
        fn model_id(&self) -> &str {
            "broken"
        }
        fn dims(&self) -> usize {
            8
        }
        fn embed_query(&self, _: &str) -> penguin_semantic::Result<Vec<f32>> {
            Err(penguin_semantic::SemanticError::Embed("boom".into()))
        }
        fn embed_passages(&self, _: &[&str]) -> penguin_semantic::Result<Vec<Vec<f32>>> {
            Err(penguin_semantic::SemanticError::Embed("boom".into()))
        }
    }
    let f = fixture(corpus());
    let broken = SemanticHandles {
        embedder: Arc::new(Broken),
        ..f.handles.clone()
    };
    let q = "lease renewal rent";
    let r = f.store.search_hybrid(&req(q), Some(&broken)).unwrap();
    assert_eq!(page(&r), page(&f.keyword(q)));
    assert_eq!(r.semantic, SemanticStatus::Ready);
}

#[test]
fn routes_follow_the_shape_of_the_query() {
    let f = fixture(corpus());
    let route = |q: &str| {
        let parsed = crate::query::parse(q, now());
        f.store
            .read(|c| {
                let plan = crate::search::compile_for_tests(c, &parsed)?;
                hybrid::route(c, &parsed, &plan, q, &crate::HybridParams::default())
            })
            .unwrap()
            .kind
    };
    assert_eq!(route("from:mike is:unread"), RouteKind::Off);
    assert_eq!(route("\"early termination\""), RouteKind::Off);
    assert_eq!(route("lease OR rent"), RouteKind::Off);
    assert_eq!(route("INV-20417"), RouteKind::Keyword);
    assert_eq!(route("order 88213"), RouteKind::Keyword);
    assert_eq!(route("ana@ruiz.example"), RouteKind::Keyword);
    assert_eq!(route("escrow"), RouteKind::Keyword);
    assert_eq!(route("mike delgado"), RouteKind::Off);
    assert_eq!(route("mike"), RouteKind::Off);
    assert_eq!(route("lease renewal"), RouteKind::Balanced);
    assert_eq!(route("from:mike lease renewal"), RouteKind::Balanced);
    assert_eq!(route("that thing about the lease renewal"), RouteKind::Semantic);
    assert_eq!(route("when is the dentist appointment"), RouteKind::Semantic);
    assert_eq!(route("flight to lisbon in may"), RouteKind::Semantic);
    assert_eq!(route("wifi password cabin?"), RouteKind::Semantic);
}

#[test]
fn the_embedded_text_is_the_query_without_operators() {
    let t = |q: &str| hybrid::semantic_text(&crate::query::parse(q, now()), q);
    assert_eq!(t("from:mike flight to lisbon").as_deref(), Some("flight to lisbon"));
    assert_eq!(
        t("date:\"last week\" that lease renewal thing ").as_deref(),
        Some("that lease renewal thing")
    );
    assert_eq!(t("lease renewal -draft has:pdf").as_deref(), Some("lease renewal"));
    // A last word still being typed is left out while others carry meaning.
    assert_eq!(t("lease re").as_deref(), Some("lease"));
    assert_eq!(t("lease ren").as_deref(), Some("lease ren"));
    assert_eq!(t("from:mike is:unread"), None);
}

#[test]
fn the_passage_comes_from_the_matching_chunk() {
    let filler = "We talked about the garden, the fence and the weather at length. ".repeat(20);
    let body = format!(
        "{filler}The spare key for the storage unit is in the blue lockbox by the gate. {filler}"
    );
    let f = fixture(vec![msg("long", "t-long", 3, ANA, "Notes from Saturday", &body)]);
    let r = f.hybrid("where is the spare storage key");
    let hit = &r.hits[0];
    let passage = hit.passage.as_deref().unwrap();
    assert!(passage.contains("spare key for the storage unit"), "{passage}");
    assert!(passage.starts_with('\u{2026}'), "mid-message passages start with an ellipsis");
}

#[test]
fn passages_survive_breaks_after_sentence_ends() {
    let body = "Are you coming?\nThe cabin wifi password is pinecone42.\n\nSee you!\r\n";
    let f = fixture(vec![msg("br", "t-br", 3, ANA, "Weekend", body)]);
    let r = f.hybrid("cabin wifi password thing");
    let passage = r.hits[0].passage.as_deref().unwrap();
    assert!(passage.starts_with("\u{2026}The cabin wifi password"), "{passage}");
}

#[test]
fn passages_are_plain_text_and_snippets_are_escaped() {
    let f = fixture(vec![msg(
        "html",
        "t-html",
        3,
        ANA,
        "Script notes",
        "The <script>alert(1)</script> tag & the cabin wifi password are in the doc.",
    )]);
    let r = f.hybrid("cabin wifi password thing");
    let hit = &r.hits[0];
    assert!(hit.passage.as_deref().unwrap().contains("<script>"), "plain text");
    assert!(!hit.snippet_html.contains("<script>"));
    assert!(hit.snippet_html.contains("&lt;script&gt;"));
}
