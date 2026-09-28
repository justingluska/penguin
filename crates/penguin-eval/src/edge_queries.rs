//! The edge-case query set: ten categories built from the taxonomy in
//! docs/SEARCH-CASES.md (what people type into Gmail, Outlook, Apple Mail,
//! Fastmail, notmuch, mu and the rest, and what fails for them). Targets are
//! the edge-case mail in `corpus/edge.rs` plus the rest of the mailbox;
//! judgments are rules over conversation facts, as in `queries.rs`.
//!
//! Queries in the `date` category spell dates out for the day the corpus is
//! generated (`on:2026-08-14`), so their text is built from `now`.
//!
//! Facet `expect-empty`: the right answer is no result at all (conflicting
//! or impossible filters); such a query scores 1 when nothing comes back.

use chrono::{Datelike, Duration, NaiveDate};

use crate::corpus::edge::{at, last_weekend, movers_day, KESTREL_LATE};
use crate::judge::Category::*;
use crate::judge::*;
use crate::window::{local_ms, today, Window};

fn q(
    id: &'static str,
    category: Category,
    text: impl Into<String>,
    facets: &'static [&'static str],
    rules: Vec<(Cond, u8)>,
) -> QuerySpec {
    QuerySpec {
        id,
        text: text.into(),
        category,
        facets,
        rules,
    }
}

const KI: &[&str] = &["known-item"];
const BROAD: &[&str] = &["broad"];
const NONE: &[&str] = &[];
const EMPTY: &[&str] = &[EXPECT_EMPTY];
const XL: &[&str] = &["cross-lingual"];
const QU: &[&str] = &["question"];
const XLQ: &[&str] = &["cross-lingual", "question"];

/// The plant is the answer.
fn p(id: &'static str) -> Vec<(Cond, u8)> {
    vec![(t(id), 3)]
}

pub fn edge_queries(now: i64) -> Vec<QuerySpec> {
    let mut v = Vec::new();
    v.extend(syntax());
    v.extend(place());
    v.extend(attachment());
    v.extend(date(now));
    v.extend(people());
    v.extend(format());
    v.extend(morphology());
    v.extend(messy());
    v.extend(mixed(now));
    v.extend(recall());
    v
}

fn marco_or_julia_invoices() -> Vec<(Cond, u8)> {
    vec![(all([t("kind:invoice"), any([t("person:marco"), t("person:julia")])]), 2)]
}

fn theo_invoices() -> Vec<(Cond, u8)> {
    vec![(all([t("kind:invoice"), t("person:theo")]), 2)]
}

/// Every recipe Carmen sent is an answer; the grandmother's paella is the
/// one the other queries are about.
fn carmen_recipes() -> Vec<(Cond, u8)> {
    vec![(t("plant:paella"), 3), (all([t("from:carmen.moreno@correo.example"), t("topic:receta")]), 2)]
}

fn lease() -> Vec<(Cond, u8)> {
    p("plant:lease")
}

#[rustfmt::skip]
fn syntax() -> Vec<QuerySpec> {
    vec![
        q("sy-or-precedence", Syntax, "from:marco OR from:julia invoice", BROAD, marco_or_julia_invoices()),
        q("sy-braces", Syntax, "{from:marco from:julia} invoice", BROAD, marco_or_julia_invoices()),
        q("sy-braces-words", Syntax, "invoice {bianchi fernhill}", BROAD, marco_or_julia_invoices()),
        q("sy-text-or", Syntax, "invoice (bianchi OR fernhill)", BROAD, marco_or_julia_invoices()),
        q("sy-from-group", Syntax, "from:(marco OR julia) invoice", BROAD, marco_or_julia_invoices()),
        q("sy-not-group", Syntax, "invoice -(crestline OR bianchi)", BROAD, vec![(all([t("kind:invoice"), not(t("person:theo")), not(t("person:marco"))]), 2)]),
        q("sy-neg-op-group", Syntax, "-(from:theo OR from:marco) invoice", BROAD, vec![(all([t("kind:invoice"), not(t("from:theo@crestline.example")), not(t("from:marco@bianchiwines.example"))]), 2)]),
        q("sy-kql-not", Syntax, "invoice NOT crestline", BROAD, vec![(all([t("kind:invoice"), not(t("person:theo"))]), 2)]),
        q("sy-exclude-caps", Syntax, "invoice -CRESTLINE", BROAD, vec![(all([t("kind:invoice"), not(t("person:theo"))]), 2)]),
        q("sy-and", Syntax, "invoice AND crestline", BROAD, theo_invoices()),
        q("sy-and-lower", Syntax, "lease and renewal", KI, lease()),
        q("sy-or-lower", Syntax, "lease or renewal", KI, lease()),
        q("sy-phrase-exclude", Syntax, "crestline invoice -\"past due\"", BROAD, vec![(all([t("kind:invoice"), t("person:theo"), not(t("plant:inv-20417"))]), 2)]),
        q("sy-subject-group", Syntax, "subject:(lease renewal)", KI, lease()),
        q("sy-wildcard", Syntax, "invoic* crestline", BROAD, theo_invoices()),
        q("sy-wildcard-brand", Syntax, "deskcr*", NONE, vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 2)]),
        q("sy-around", Syntax, "rent AROUND 5 june", KI, lease()),
        q("sy-two-phrases", Syntax, "\"renewal agreement\" \"12 months\"", KI, lease()),
        q("sy-dangling-or", Syntax, "lease renewal OR", KI, lease()),
        q("sy-leading-or", Syntax, "OR lease renewal", KI, lease()),
        q("sy-reversed-phrase", Syntax, "\"renewal lease\"", EMPTY, vec![]),
        q("sy-plus", Syntax, "+lease +renewal", KI, lease()),
        q("sy-from-not-delgado", Syntax, "from:mike -delgado", BROAD, vec![(t("from:mike.chen@northwind.example"), 2)]),
        q("sy-exclude-address", Syntax, "mike -from:mike.delgado@harborrealty.example", BROAD, vec![(t("from:mike.chen@northwind.example"), 2), (t("person:mike-w"), 1)]),
        q("sy-phrase-or", Syntax, "(\"lease renewal\" OR \"parking spot\") from:mike", NONE, vec![(t("plant:lease"), 3), (t("plant:parking"), 3)]),
        q("sy-nested", Syntax, "((lease OR parking) (renewal OR spot)) from:mike", NONE, vec![(t("plant:lease"), 3), (t("plant:parking"), 3)]),
        q("sy-group-pdf", Syntax, "from:mike (lease OR plumber) has:pdf", KI, lease()),
        q("sy-unbalanced-open", Syntax, "(lease renewal", KI, lease()),
        q("sy-unbalanced-close", Syntax, "lease renewal)", KI, lease()),
        q("sy-open-quote", Syntax, "\"early termination", KI, lease()),
        q("sy-empty-op", Syntax, "lease from:", KI, lease()),
        q("sy-space-after-colon", Syntax, "from: theo past due", KI, p("plant:inv-20417")),
        q("sy-uppercase-op", Syntax, "FROM:theo SUBJECT:invoice", BROAD, theo_invoices()),
        q("sy-smart-quotes", Syntax, "\u{201c}early termination\u{201d}", KI, lease()),
        q("sy-or-chain", Syntax, "from:ana OR from:ravi OR from:greta", BROAD, vec![(any([t("from:ana.sousa@lindenpartners.example"), t("from:ravi@bluepeak.example"), t("from:greta.holm@fjordsoft.example")]), 2)]),
        q("sy-neg-has", Syntax, "from:jess -has:attachment cabin", KI, p("plant:cabin")),
    ]
}

#[rustfmt::skip]
fn place() -> Vec<QuerySpec> {
    let tax = || vec![(t("ulabel:Label_Tax_Docs_2025"), 2)];
    let fernhill = || vec![(t("ulabel:Label_Clients_Fernhill_Bakery"), 2)];
    let atlas_fin = || vec![(t("ulabel:Label_Work_Atlas_Finance"), 2)];
    let lisbon = || vec![(t("ulabel:Label_Travel_Lisbon_2026"), 2)];
    let follow = || vec![(t("ulabel:Label_Follow-up"), 2)];
    vec![
        q("pl-label", Place, "label:receipts", BROAD, vec![(t("ulabel:Label_Receipts"), 2)]),
        q("pl-label-words", Place, "label:Receipts deskcraft", NONE, vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 1)]),
        q("pl-label-case", Place, "LABEL:RECEIPTS parcelmart", BROAD, vec![(all([t("ulabel:Label_Receipts"), t("merchant:parcelmart")]), 2)]),
        q("pl-label-quoted-space", Place, "label:\"tax docs 2025\"", BROAD, tax()),
        q("pl-label-dashed", Place, "label:tax-docs-2025", BROAD, tax()),
        q("pl-label-underscore", Place, "label:tax_docs_2025", BROAD, tax()),
        q("pl-label-unquoted-space", Place, "label:tax docs 2025", BROAD, tax()),
        q("pl-label-space-words", Place, "label:\"Tax Docs 2025\" refund", KI, p("plant:tax")),
        q("pl-nested-slash", Place, "label:\"clients/fernhill bakery\"", BROAD, fernhill()),
        q("pl-nested-dash", Place, "label:clients-fernhill-bakery", BROAD, fernhill()),
        q("pl-nested-leaf", Place, "label:\"fernhill bakery\"", BROAD, fernhill()),
        q("pl-nested-partial", Place, "label:fernhill", BROAD, fernhill()),
        q("pl-nested-3", Place, "label:work/atlas/finance", BROAD, atlas_fin()),
        q("pl-nested-suffix", Place, "label:atlas/finance", BROAD, atlas_fin()),
        q("pl-nested-last", Place, "label:finance", BROAD, atlas_fin()),
        q("pl-label-travel", Place, "label:\"travel/lisbon 2026\"", BROAD, lisbon()),
        q("pl-label-travel-dash", Place, "label:travel-lisbon-2026", BROAD, lisbon()),
        q("pl-label-hyphen", Place, "label:follow-up", BROAD, follow()),
        q("pl-label-hyphen-space", Place, "label:\"follow up\"", BROAD, follow()),
        q("pl-label-family-words", Place, "label:family receta", NONE, vec![(t("plant:paella"), 3), (all([t("ulabel:Label_Family"), t("topic:receta")]), 2)]),
        q("pl-neg-label", Place, "-label:receipts deskcraft", KI, vec![(t("plant:desk-ship"), 3)]),
        q("pl-inbox-from", Place, "in:inbox from:priya", BROAD, vec![(t("inbox-from:priya.shah@northwind.example"), 2)]),
        q("pl-unread", Place, "is:unread", BROAD, vec![(t("is:unread"), 2)]),
        q("pl-inbox-unread", Place, "in:inbox is:unread", BROAD, vec![(all([t("in:inbox"), t("is:unread")]), 2)]),
        q("pl-starred", Place, "in:starred", BROAD, vec![(t("is:starred"), 2)]),
        q("pl-starred-budget", Place, "is:starred budget", BROAD, vec![(all([t("is:starred"), t("topic:budget")]), 2)]),
        q("pl-sent-to", Place, "in:sent to:julia", BROAD, vec![(t("sent-to:julia@fernhillbakery.example"), 2)]),
        q("pl-is-sent-to", Place, "is:sent to:julia", BROAD, vec![(t("sent-to:julia@fernhillbakery.example"), 2)]),
        q("pl-archived-lease", Place, "-in:inbox lease renewal", KI, lease()),
        q("pl-done-lease", Place, "in:done lease renewal", KI, lease()),
        q("pl-trash-hidden", Place, "\"comeback offer\"", EMPTY, vec![]),
        q("pl-trash", Place, "in:trash comeback", KI, p("plant:gym-trash")),
        q("pl-bin", Place, "in:bin comeback", KI, p("plant:gym-trash")),
        q("pl-anywhere", Place, "in:anywhere comeback offer", KI, p("plant:gym-trash")),
        q("pl-spam", Place, "in:spam gift card", KI, p("plant:spam-prize")),
        q("pl-junk", Place, "in:junk prize", KI, p("plant:spam-prize")),
        q("pl-spam-hidden", Place, "\"prize claim form\"", EMPTY, vec![]),
        q("pl-category-social", Place, "category:social trailmates", BROAD, vec![(all([t("category:social"), t("merchant:trailmates")]), 2)]),
        q("pl-category-promos", Place, "category:promotions skyward", BROAD, vec![(all([t("kind:promotion"), t("merchant:skyward")]), 2)]),
        q("pl-category-updates", Place, "category:updates skyward", BROAD, vec![(all([t("category:updates"), t("airline:skyward")]), 2)]),
        q("pl-category-primary", Place, "category:primary jess", BROAD, vec![(all([t("category:personal"), t("person:jess")]), 2)]),
        q("pl-account-personal", Place, "account:personal flight", BROAD, vec![(all([t("kind:flight"), t("account:personal")]), 2)]),
        q("pl-account-address", Place, "account:hello@morenostudio.example julia", BROAD, vec![(all([t("account:studio"), t("person:julia")]), 2)]),
        q("pl-account-work", Place, "account:work atlas budget", BROAD, vec![(all([t("account:work"), t("topic:budget"), t("project:atlas")]), 2)]),
    ]
}

#[rustfmt::skip]
fn attachment() -> Vec<QuerySpec> {
    vec![
        q("at-presentation", Attachment, "has:presentation", KI, p("plant:atlas-roadmap")),
        q("at-presentation-words", Attachment, "has:presentation atlas roadmap", KI, p("plant:atlas-roadmap")),
        q("at-filename-ext", Attachment, "filename:pptx", KI, p("plant:atlas-roadmap")),
        q("at-filename-full", Attachment, "filename:Q3_report_final_v2.xlsx", KI, p("plant:q3-report")),
        q("at-filename-words", Attachment, "filename:\"q3 report\"", KI, p("plant:q3-report")),
        q("at-filename-short", Attachment, "filename:q3", BROAD, vec![(file("q3"), 2)]),
        q("at-filename-xlsx", Attachment, "filename:xlsx atlas", BROAD, vec![(all([t("project:atlas"), file(".xlsx")]), 2)]),
        q("at-filename-spaces", Attachment, "filename:\"Scan 2026-03-14\"", KI, p("plant:scan-addendum")),
        q("at-filename-scan", Attachment, "filename:scan", KI, p("plant:scan-addendum")),
        q("at-filename-docx", Attachment, "filename:.docx", BROAD, vec![(file(".docx"), 2)]),
        q("at-has-doc", Attachment, "has:doc from:kowalski", KI, p("plant:kowalski")),
        q("at-has-document", Attachment, "has:document will draft", KI, p("plant:kowalski")),
        q("at-larger-25", Attachment, "larger:25M", BROAD, vec![(t("larger:25m"), 2)]),
        q("at-larger-mb", Attachment, "larger:10mb", BROAD, vec![(t("larger:10m"), 2)]),
        q("at-size-gt", Attachment, "size:>10M", BROAD, vec![(t("larger:10m"), 2)]),
        q("at-size-bytes", Attachment, "size:30000000", BROAD, vec![(t("larger:25m"), 2)]),
        q("at-larger-zip", Attachment, "larger:15M zip", KI, p("plant:site-photos")),
        q("at-larger-from", Attachment, "larger:5M from:chloe", KI, p("plant:board-deck")),
        q("at-smaller", Attachment, "smaller:100K has:pdf from:ledgerly", KI, p("plant:tax-1099")),
        q("at-image-words", Attachment, "has:image washing machine", KI, p("plant:heic")),
        q("at-heic", Attachment, "filename:heic", KI, p("plant:heic")),
        q("at-octet-pdf", Attachment, "has:pdf from:marco", KI, p("plant:octet-pdf")),
        q("at-filename-po", Attachment, "filename:PO-5816", KI, p("plant:octet-pdf")),
        q("at-csv", Attachment, "filename:csv", KI, p("plant:csv")),
        q("at-spreadsheet-csv", Attachment, "has:spreadsheet ledgerly", KI, p("plant:csv")),
        q("at-filename-mov", Attachment, "filename:mov", KI, p("plant:wedding-toast")),
        q("at-attachment-words", Attachment, "has:attachment toast video", KI, p("plant:wedding-toast")),
        q("at-kql-hasattachment", Attachment, "hasattachment:yes lease", KI, lease()),
        q("at-kql-attachment", Attachment, "attachment:roadmap", KI, p("plant:atlas-roadmap")),
        q("at-zip-fernhill", Attachment, "has:attachment filename:zip fernhill", KI, p("plant:fernhill-final")),
        q("at-ics", Attachment, "filename:ics", BROAD, vec![(t("kind:invite"), 2)]),
        q("at-jpg-cadiz", Attachment, "filename:jpg cadiz", KI, p("plant:jose-cadiz")),
        q("at-filename-case", Attachment, "filename:INVITATION-LETTER-TOKYO.PDF", KI, p("plant:visa")),
    ]
}

/// "14th", "1st", "22nd", "3rd".
fn ordinal(d: u32) -> String {
    let suffix = match (d % 10, d % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{d}{suffix}")
}

fn day_window(d: NaiveDate) -> Window {
    Window::Abs(local_ms(d), local_ms(d + Duration::days(1)))
}

#[rustfmt::skip]
fn date(now: i64) -> Vec<QuerySpec> {
    let t0 = today(now);
    let d = movers_day(now);
    let days = (t0 - d).num_days();
    let weeks = days / 7 + 1;
    let f = |x: NaiveDate, fmt: &str| x.format(fmt).to_string();
    let movers = || p("plant:movers");
    // Open windows up to today also hold the movers' follow-up: both are answers.
    let movers_since = || vec![(t("plant:movers"), 3), (t("plant:movers-review"), 2)];
    let on_day = || vec![(t("plant:movers"), 3), (touches(day_window(d)), 2)];
    let late = t0 - Duration::days(KESTREL_LATE);
    let early = late + Duration::days(1);
    let epoch = at(now, KESTREL_LATE, 23, 55) / 1000;
    let lastq = {
        let q0 = NaiveDate::from_ymd_opt(t0.year(), (t0.month0() / 3) * 3 + 1, 1).unwrap();
        let prev = q0.checked_sub_months(chrono::Months::new(3)).unwrap();
        format!("q{} {}", prev.month0() / 3 + 1, prev.year())
    };
    let since_month = t0.with_day(1).unwrap().checked_sub_months(chrono::Months::new(2)).unwrap();
    let sent_invoices_lastq = || vec![(all([t("kind:invoice"), t("direction:out"), within(Window::LastQuarter)]), 2)];
    let _ = last_weekend(now);
    vec![
        q("da-iso", Date, format!("movers date:{}", f(d, "%Y-%m-%d")), KI, movers()),
        q("da-on-us", Date, format!("movers on:{}", f(d, "%m/%d/%Y")), KI, movers()),
        q("da-gmail-range", Date, format!("movers after:{} before:{}", f(d - Duration::days(1), "%Y/%m/%d"), f(d + Duration::days(1), "%Y/%m/%d")), KI, movers()),
        q("da-long", Date, format!("movers date:\"{}\"", f(d, "%B %-d, %Y")), KI, movers()),
        q("da-day-first", Date, format!("movers date:\"{}\"", f(d, "%-d %B %Y")), KI, movers()),
        q("da-ordinal", Date, format!("movers date:\"{} of {}\"", ordinal(d.day()), f(d, "%B")), KI, movers()),
        q("da-abbrev-dot", Date, format!("movers date:\"{}. {}\"", f(d, "%b"), d.day()), KI, movers()),
        q("da-short-year", Date, format!("movers date:{}", f(d, "%-m/%-d/%y")), KI, movers()),
        q("da-month-year", Date, format!("movers date:\"{}\"", f(d, "%B %Y")), KI, movers()),
        q("da-range-dots", Date, format!("movers date:{}..{}", f(d - Duration::days(2), "%Y-%m-%d"), f(d + Duration::days(2), "%Y-%m-%d")), KI, movers()),
        q("da-range-to", Date, format!("movers date:\"{} to {}\"", f(d - Duration::days(3), "%b %-d"), f(d, "%b %-d")), KI, movers()),
        q("da-on-iso-only", Date, format!("on:{}", f(d, "%Y-%m-%d")), NONE, on_day()),
        q("da-on-eu-only", Date, format!("on:{}", f(d, "%d/%m/%Y")), NONE, on_day()),
        q("da-on-dotted-only", Date, format!("on:{}", f(d, "%d.%m.%Y")), NONE, on_day()),
        q("da-newer-than", Date, format!("movers newer_than:{}d", days + 5), NONE, movers_since()),
        q("da-older-than", Date, "movers older_than:30d", KI, movers()),
        q("da-older-than-none", Date, format!("\"bluebird movers\" older_than:{}d", days + 10), EMPTY, vec![]),
        q("da-since-weeks", Date, format!("movers since:\"{weeks} weeks ago\""), NONE, movers_since()),
        q("da-last-n-days", Date, format!("movers date:\"last {} days\"", days + 3), NONE, movers_since()),
        q("da-past-weeks", Date, format!("movers date:\"past {weeks} weeks\""), NONE, movers_since()),
        q("da-kestrel-on-late", Date, format!("kestrel on:{}", f(late, "%Y-%m-%d")), KI, p("plant:kestrel-late")),
        q("da-kestrel-on-early", Date, format!("kestrel on:{}", f(early, "%Y-%m-%d")), KI, p("plant:kestrel-early")),
        q("da-kestrel-after", Date, format!("kestrel after:{}", f(early, "%Y/%m/%d")), KI, p("plant:kestrel-early")),
        q("da-kestrel-before", Date, format!("kestrel before:{}", f(early, "%Y/%m/%d")), KI, p("plant:kestrel-late")),
        q("da-kestrel-until", Date, format!("kestrel until:{}", f(late, "%Y-%m-%d")), KI, p("plant:kestrel-late")),
        q("da-kestrel-epoch", Date, format!("kestrel after:{epoch}"), KI, p("plant:kestrel-early")),
        q("da-compost-2w", Date, "compost date:\"2 weeks ago\"", KI, p("plant:compost")),
        q("da-compost-two", Date, "compost date:\"two weeks ago\"", KI, p("plant:compost")),
        q("da-compost-couple", Date, "compost date:\"a couple of weeks ago\"", KI, p("plant:compost")),
        q("da-compost-words", Date, "compost two weeks ago", KI, p("plant:compost")),
        q("da-brunch-weekend", Date, "brunch date:\"last weekend\"", KI, p("plant:brunch")),
        q("da-brunch-words", Date, "brunch pics last weekend", KI, p("plant:brunch")),
        q("da-today", Date, "from:ledgerly date:today", KI, p("plant:otp-bank")),
        q("da-yesterday", Date, "código date:yesterday", KI, p("plant:otp-es")),
        q("da-last-quarter", Date, "in:sent invoice date:\"last quarter\"", BROAD, sent_invoices_lastq()),
        q("da-quarter-name", Date, format!("in:sent invoice date:\"{lastq}\""), BROAD, sent_invoices_lastq()),
        q("da-since-month", Date, format!("from:swiftcab since:{}", f(since_month, "%B").to_lowercase()), BROAD,
          vec![(all([t("merchant:swiftcab"), touches(Window::Abs(local_ms(since_month), now + 86_400_000))]), 2)]),
    ]
}

#[rustfmt::skip]
fn people() -> Vec<QuerySpec> {
    let jose = || vec![(t("person:jose"), 2)];
    let plus = || vec![(t("plus:shopping"), 2)];
    let ledgerly = || vec![(t("merchant:ledgerly"), 2)];
    vec![
        q("pe-jose-ascii", People, "jose nunez", BROAD, jose()),
        q("pe-jose-accents", People, "José Núñez", BROAD, jose()),
        q("pe-jose-mixed", People, "Jose Núñez", BROAD, jose()),
        q("pe-nunez-accent", People, "núñez", BROAD, jose()),
        q("pe-from-nunez", People, "from:nunez", BROAD, jose()),
        q("pe-soren-ascii", People, "soren odegard", KI, p("plant:soren")),
        q("pe-soren-exact", People, "Søren Ødegård", KI, p("plant:soren")),
        q("pe-odegard", People, "odegard", KI, p("plant:soren")),
        q("pe-cjk-surname", People, "田中", KI, p("plant:tanaka-cjk")),
        q("pe-cjk-from", People, "from:田中", KI, p("plant:tanaka-cjk")),
        q("pe-cjk-romaji", People, "misaki tanaka", KI, p("plant:tanaka-cjk")),
        q("pe-obrien-apostrophe", People, "o'brien", KI, p("plant:obrien")),
        q("pe-obrien-joined", People, "obrien", KI, p("plant:obrien")),
        q("pe-obrien-curly", People, "O\u{2019}Brien", KI, p("plant:obrien")),
        q("pe-obrien-from", People, "from:o'brien", KI, p("plant:obrien")),
        q("pe-siobhan-ascii", People, "siobhan", KI, p("plant:obrien")),
        q("pe-siobhan-accent", People, "siobhán", KI, p("plant:obrien")),
        q("pe-annemarie-hyphen", People, "anne-marie", KI, p("plant:anne-marie")),
        q("pe-annemarie-space", People, "anne marie dubois", KI, p("plant:anne-marie")),
        q("pe-annemarie-joined", People, "annemarie", KI, p("plant:anne-marie")),
        q("pe-hyphen-domain", People, "atelier-lune.example", KI, p("plant:anne-marie")),
        q("pe-bob", People, "bob kowalski", KI, p("plant:kowalski")),
        q("pe-robert", People, "robert kowalski", KI, p("plant:kowalski")),
        q("pe-rob-nickname", People, "rob kowalski", KI, p("plant:kowalski")),
        q("pe-evans-address", People, "revans", KI, p("plant:evans")),
        q("pe-dr-evans", People, "dr evans", KI, p("plant:evans")),
        q("pe-dr-dot", People, "Dr. Ruth Evans", KI, p("plant:evans")),
        q("pe-plus-full", People, "to:alexmoreno+shopping@mailbox.example", BROAD, plus()),
        q("pe-plus-local", People, "alexmoreno+shopping", BROAD, plus()),
        q("pe-plus-tag", People, "to:+shopping", BROAD, plus()),
        q("pe-deliveredto", People, "deliveredto:alexmoreno+shopping@mailbox.example", BROAD, plus()),
        q("pe-plus-to-me", People, "to:me lumen lamps", NONE, vec![(t("plant:lamp-order"), 2), (t("plant:lamp-ship"), 2)]),
        q("pe-idn-domain", People, "müller-bau.example", KI, p("plant:muller")),
        q("pe-muller-ascii", People, "jurgen muller", KI, p("plant:muller")),
        q("pe-muller-ue", People, "juergen mueller", KI, p("plant:muller")),
        q("pe-strasse", People, "hauptstrasse 12", KI, p("plant:muller")),
        q("pe-domain-subdomains", People, "domain:ledgerly.example", BROAD, ledgerly()),
        q("pe-from-subdomain", People, "from:@notify.ledgerly.example", NONE, vec![(t("plant:ledgerly-statement"), 2), (t("plant:csv"), 2)]),
        q("pe-bare-domain", People, "ledgerly.example", BROAD, ledgerly()),
        q("pe-work-subdomain", People, "domain:northwind.example tokyo", NONE, vec![(t("plant:tanaka-cjk"), 3), (t("plant:visa"), 2)]),
        q("pe-emoji-only", People, "🎉", KI, p("plant:party")),
        q("pe-emoji-word", People, "🎉 party", KI, p("plant:party")),
        q("pe-ampersand", People, "birch & bloom", KI, p("plant:birch-bloom")),
        q("pe-and-word", People, "birch and bloom", KI, p("plant:birch-bloom")),
        q("pe-joined-company", People, "birchandbloom", KI, p("plant:birch-bloom")),
        q("pe-amp-nospace", People, "birch&bloom", KI, p("plant:birch-bloom")),
        q("pe-company-amp", People, "thistle & page", KI, p("plant:uk-cancelled")),
    ]
}

#[rustfmt::skip]
fn format() -> Vec<QuerySpec> {
    let desk = || vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 2)];
    vec![
        q("fo-ups-joined", Format, "1Z4F8A620311872290", KI, p("plant:lamp-ship")),
        q("fo-ups-spaced", Format, "1Z 4F8 A62 03 1187 2290", KI, p("plant:lamp-ship")),
        q("fo-ups-lower", Format, "1z4f8a620311872290", KI, p("plant:lamp-ship")),
        q("fo-usps-spaced", Format, "9400 1118 9956 2537 8663 61", KI, p("plant:usps")),
        q("fo-usps-joined", Format, "9400111899562537866361", KI, p("plant:usps")),
        q("fo-inv-nodash", Format, "INV20417", KI, p("plant:inv-20417")),
        q("fo-inv-space", Format, "inv 20417", KI, p("plant:inv-20417")),
        q("fo-inv-hash", Format, "#20417", KI, p("plant:inv-20417")),
        q("fo-order-nodash", Format, "DC55120", NONE, desk()),
        q("fo-order-hash", Format, "order #DC-55120", NONE, desk()),
        q("fo-flight-joined", Format, "SK2210", KI, p("plant:lisbon-flight")),
        q("fo-code-nodash", Format, "NA883104", KI, p("plant:lisbon-stay")),
        q("fo-amount-plain", Format, "2450", KI, lease()),
        q("fo-amount-dollar", Format, "$2450", KI, lease()),
        q("fo-amount-cents", Format, "$2,450.00", KI, lease()),
        q("fo-amount-big-plain", Format, "18400", KI, p("plant:inv-20417")),
        q("fo-amount-big-cents", Format, "$18,400.00", KI, p("plant:inv-20417")),
        q("fo-amount-millions", Format, "$1.2M atlas", KI, p("plant:atlas-budget")),
        q("fo-percent", Format, "9% expansion", KI, p("plant:q3-report")),
        q("fo-eur-plain", Format, "1600 euros", KI, p("plant:valencia-flat")),
        q("fo-eur-symbol", Format, "€1,600", KI, p("plant:valencia-flat")),
        q("fo-eur-us-decimal", Format, "1234.56", KI, p("plant:hotel-madrid")),
        q("fo-eur-exact", Format, "1.234,56 €", KI, p("plant:hotel-madrid")),
        q("fo-eur-us-format", Format, "1,234.56", KI, p("plant:hotel-madrid")),
        q("fo-phone-digits", Format, "4155550138", KI, p("plant:dana-number")),
        q("fo-phone-dashes", Format, "415-555-0138", KI, p("plant:dana-number")),
        q("fo-phone-dots", Format, "415.555.0138", KI, p("plant:dana-number")),
        q("fo-phone-intl", Format, "+1 415 555 0138", KI, p("plant:dana-number")),
        q("fo-phone-uk-national", Format, "020 7946 0958", KI, p("plant:london-hotel")),
        q("fo-phone-uk-intl", Format, "+44 20 7946 0958", KI, p("plant:london-hotel")),
        q("fo-phone-es-joined", Format, "915550123", KI, p("plant:hotel-madrid")),
        q("fo-url-full", Format, "https://portal.fjordsoft.example/v2/migration-guide", KI, p("plant:fjordsoft-url")),
        q("fo-url-host", Format, "portal.fjordsoft.example", KI, p("plant:fjordsoft-url")),
        q("fo-url-path", Format, "fjordsoft.example/v2", KI, p("plant:fjordsoft-url")),
        q("fo-url-slug", Format, "migration-guide", NONE, vec![(t("plant:fjordsoft-url"), 3), (t("plant:api-sunset"), 1)]),
        q("fo-version", Format, "2.14.3", KI, p("plant:deploy-version")),
        q("fo-version-v", Format, "v2.14.3", KI, p("plant:deploy-version")),
        q("fo-case-slash", Format, "CX-2026/0419", KI, p("plant:baggage")),
        q("fo-case-spaced", Format, "cx 2026 0419", KI, p("plant:baggage")),
        q("fo-email-in-body", Format, "security@northwind.example", KI, p("plant:phishing")),
    ]
}

#[rustfmt::skip]
fn morphology() -> Vec<QuerySpec> {
    vec![
        q("mo-invoices", Morphology, "crestline invoices", BROAD, theo_invoices()),
        q("mo-invoices-first", Morphology, "invoices crestline", BROAD, theo_invoices()),
        q("mo-receipt-singular", Morphology, "receipt swiftcab", BROAD, vec![(all([t("kind:receipt"), t("merchant:swiftcab")]), 2)]),
        q("mo-bills", Morphology, "harbor mobile bills", BROAD, vec![(all([t("kind:bill"), t("merchant:harbormobile")]), 2)]),
        q("mo-statements", Morphology, "cedar valley statements", BROAD, vec![(all([t("kind:bill"), t("merchant:cedarvalley")]), 2)]),
        q("mo-codes", Morphology, "ledgerly verification codes", BROAD, vec![(all([t("kind:code"), t("merchant:ledgerly")]), 2)]),
        q("mo-orders", Morphology, "deskcraft orders", NONE, vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 2)]),
        q("mo-flights", Morphology, "lisbon flights", NONE, vec![(t("plant:lisbon-flight"), 3), (all([t("kind:flight"), t("city:lisbon")]), 2)]),
        q("mo-shipping", Morphology, "deskcraft shipping", NONE, vec![(t("plant:desk-ship"), 3), (t("plant:desk-order"), 2)]),
        q("mo-ships", Morphology, "lamp ships", KI, p("plant:lamp-ship")),
        q("mo-cancel", Morphology, "gym cancel", KI, p("plant:gym")),
        q("mo-renewing", Morphology, "renewing lease", KI, lease()),
        q("mo-approve", Morphology, "approve atlas budget", NONE, vec![(t("plant:atlas-budget"), 3), (all([t("topic:budget"), t("project:atlas")]), 1)]),
        q("mo-refunding", Morphology, "refunding headphones", KI, p("plant:refund")),
        q("mo-results", Morphology, "lab result", KI, p("plant:evans")),
        q("mo-us-canceled", Morphology, "order canceled", NONE, vec![(t("plant:uk-cancelled"), 3)]),
        q("mo-us-favorite", Morphology, "favorite titles", KI, p("plant:uk-cancelled")),
        q("mo-us-catalog", Morphology, "autumn catalog", KI, p("plant:uk-cancelled")),
        q("mo-us-color", Morphology, "color proofs", KI, p("plant:colour-proofs")),
        q("mo-us-gray", Morphology, "gray flour bag", KI, p("plant:colour-proofs")),
        q("mo-us-organize", Morphology, "organize a call julia", KI, p("plant:colour-proofs")),
        q("mo-us-license", Morphology, "driving license renewal", KI, p("plant:licence")),
        q("mo-us-center", Morphology, "licensing center", KI, p("plant:licence")),
        q("mo-us-theater", Morphology, "theater tickets", KI, p("plant:theatre")),
        q("mo-us-neighbor", Morphology, "neighbor dana", KI, p("plant:dana-number")),
        q("mo-us-favorite-sunset", Morphology, "favorite sunset", KI, p("plant:tahoe")),
        q("mo-oncall", Morphology, "oncall handoff", KI, p("plant:pto")),
        q("mo-checkin", Morphology, "lisbon checkin", NONE, vec![(t("plant:lisbon-flight"), 3), (all([t("kind:flight"), t("city:lisbon")]), 2)]),
        q("mo-wifi-hyphen", Morphology, "wi-fi password", KI, p("plant:cabin")),
        q("mo-pickup-hyphen", Morphology, "bike pick-up", KI, p("plant:bike")),
        q("mo-pickup-space", Morphology, "bike pick up", KI, p("plant:bike")),
        q("mo-setup", Morphology, "laptop setup", KI, p("plant:laptop")),
        q("mo-e-mail", Morphology, "phishing e-mails", KI, p("plant:phishing")),
        q("mo-possessive", Morphology, "sofias party", KI, p("plant:party")),
        q("mo-possessive-ben", Morphology, "bens wedding", NONE, vec![(t("plant:wedding"), 3), (t("plant:wedding-toast"), 2)]),
        // CJK is written without spaces: a word inside a run of characters.
        q("mo-cjk-substring", Morphology, "会議室", KI, p("plant:tanaka-cjk")),
        q("mo-cjk-prefix", Morphology, "東京オフィス", KI, p("plant:tanaka-cjk")),
        q("mo-cjk-with-latin", Morphology, "会議室 room b", KI, p("plant:tanaka-cjk")),
    ]
}

#[rustfmt::skip]
fn messy() -> Vec<QuerySpec> {
    vec![
        q("me-short-vet", Messy, "vet", KI, p("plant:vet")),
        q("me-short-pto", Messy, "pto", KI, p("plant:pto")),
        q("me-short-msa", Messy, "msa", KI, p("plant:linden-msa")),
        q("me-short-cpa", Messy, "cpa", KI, p("plant:tax")),
        q("me-long-pasted", Messy, "Monthly rent goes to $2,450 starting June 1. Please sign and send it back by May 15.", KI, lease()),
        q("me-long-rambling", Messy, "i'm looking for the email from the landlord's agent with the lease renewal agreement that i had to sign and send back", KI, lease()),
        q("me-long-question", Messy, "can you find me the message where the plumber is coming to swap out something in my unit on thursday morning", KI, p("plant:plumber")),
        q("me-re-subject", Messy, "Re: Invoice MS-2026-007 \u{2014} Bianchi Wines", NONE, vec![(t("plant:bianchi-late"), 3), (t("invoice:ms-2026-007"), 2)]),
        q("me-fwd-subject", Messy, "Fwd: Signed MSA attached", KI, p("plant:linden-msa")),
        q("me-re-fw-subject", Messy, "RE: FW: Past due: invoice INV-20417", KI, p("plant:inv-20417")),
        q("me-external-tag", Messy, "[External] Past due: invoice INV-20417", KI, p("plant:inv-20417")),
        q("me-reply-chain", Messy, "Re: Re: Re: Team retreat logistics: Pine Lodge", KI, p("plant:retreat")),
        q("me-german-prefix", Messy, "AW: Nordlys pilot: kickoff notes", KI, p("plant:soren")),
        q("me-spanish-prefix", Messy, "RV: Presupuesto para la nueva web", KI, p("plant:castillo-quote")),
        q("me-exclaim", Messy, "lease renewal!!!", KI, lease()),
        q("me-question-marks", Messy, "lease renewal???", KI, lease()),
        q("me-ellipsis", Messy, "...lease renewal...", KI, lease()),
        q("me-slash", Messy, "lease/renewal", KI, lease()),
        q("me-commas", Messy, "lease, renewal;", KI, lease()),
        q("me-hyphen-join", Messy, "lease-renewal", KI, lease()),
        q("me-caps", Messy, "LEASE RENEWAL", KI, lease()),
        q("me-camel", Messy, "LeaseRenewal", KI, lease()),
        q("me-spaces", Messy, "   lease     renewal   ", KI, lease()),
        q("me-tab", Messy, "lease\trenewal", KI, lease()),
        q("me-transposed", Messy, "crestlnie invoice", BROAD, theo_invoices()),
        q("me-transposed-2", Messy, "plumebr thursday", KI, p("plant:plumber")),
        q("me-adjacent-key", Messy, "invoixe crestline", BROAD, theo_invoices()),
        q("me-doubled", Messy, "retreeat pine lodge", KI, p("plant:retreat")),
        q("me-missing-space", Messy, "leaserenewal", KI, lease()),
        q("me-split-word", Messy, "lea se renewal", KI, lease()),
        q("me-emoji-noise", Messy, "lease renewal 🙏", KI, lease()),
        q("me-url-noise", Messy, "lease renewal https://", KI, lease()),
        q("me-apostrophe", Messy, "can't wait to celebrate", KI, p("plant:wedding")),
        q("me-apostrophe-curly", Messy, "can\u{2019}t wait to celebrate", KI, p("plant:wedding")),
        q("me-trailing-op", Messy, "lease has:", KI, lease()),
    ]
}

#[rustfmt::skip]
fn mixed(now: i64) -> Vec<QuerySpec> {
    let future = (today(now) + Duration::days(30)).format("%Y/%m/%d").to_string();
    vec![
        q("mx-from-nl", Mixed, "from:mike the pdf about the lease", KI, lease()),
        q("mx-has-question", Mixed, "has:pdf what did grace send", KI, p("plant:tax")),
        q("mx-sent-nl", Mixed, "in:sent the logo files i sent julia", KI, p("plant:fernhill-final")),
        q("mx-from-season", Mixed, "from:jess photos last summer", KI, p("plant:tahoe")),
        q("mx-label-nl", Mixed, "label:\"tax docs 2025\" refund amount", KI, p("plant:tax")),
        q("mx-account-question", Mixed, "account:work what's my asset tag", KI, p("plant:laptop")),
        q("mx-date-op-nl", Mixed, "date:\"last spring\" lease pdf from mike", KI, lease()),
        q("mx-attachment-nl", Mixed, "has:attachment wedding toast video", KI, p("plant:wedding-toast")),
        q("mx-from-cross", Mixed, "from:carmen recipe", NONE, carmen_recipes()),
        q("mx-from-es", Mixed, "from:carmen receta", NONE, carmen_recipes()),
        q("mx-to-nl", Mixed, "to:julia the final logo", KI, p("plant:fernhill-final")),
        q("mx-larger-nl", Mixed, "larger:10M the zip from tom", KI, p("plant:site-photos")),
        q("mx-filename-nl", Mixed, "filename:pptx roadmap for monday", KI, p("plant:atlas-roadmap")),
        q("mx-inbox-question", Mixed, "in:inbox when is my self review due", KI, p("plant:self-review")),
        q("mx-filter-meaning", Mixed, "from:kwame money approved for the project", KI, p("plant:atlas-budget")),
        q("mx-account-meaning", Mixed, "account:personal plane tickets to portugal", KI, p("plant:lisbon-flight")),
        q("mx-neg-meaning", Mixed, "hotel in lisbon -nestaway", BROAD, vec![(all([t("kind:hotel"), t("city:lisbon"), not(t("merchant:nestaway"))]), 2)]),
        q("mx-cf-sent-from-other", Mixed, "in:sent from:theo", EMPTY, vec![]),
        q("mx-cf-pdf-no-attachment", Mixed, "has:pdf -has:attachment", EMPTY, vec![]),
        q("mx-cf-before-corpus", Mixed, "lease before:2015-01-01", EMPTY, vec![]),
        q("mx-cf-future", Mixed, format!("invoice after:{future}"), EMPTY, vec![]),
        q("mx-cf-older-newer", Mixed, "older_than:30d newer_than:10d", EMPTY, vec![]),
        q("mx-cf-read-unread", Mixed, "is:unread is:read", EMPTY, vec![]),
        q("mx-cf-two-seasons", Mixed, "lease date:\"last spring\" date:\"last summer\"", EMPTY, vec![]),
        q("mx-cf-two-senders", Mixed, "from:priya from:kwame budget", EMPTY, vec![]),
        q("mx-cf-label-missing", Mixed, "label:nonexistent invoice", EMPTY, vec![]),
        q("mx-cf-trash-lease", Mixed, "in:trash lease renewal", EMPTY, vec![]),
        q("mx-cf-phrase-missing", Mixed, "\"quarterly unicorn review\"", EMPTY, vec![]),
        q("mx-cf-unknown-sender", Mixed, "from:zelda invoice", EMPTY, vec![]),
        q("mx-cf-unknown-account", Mixed, "account:school budget", EMPTY, vec![]),
        q("mx-cf-larger-huge", Mixed, "larger:1G", EMPTY, vec![]),
    ]
}

#[rustfmt::skip]
fn recall() -> Vec<QuerySpec> {
    vec![
        q("rc-rent-going-up", Recall, "rent going up", KI, lease()),
        q("rc-where-mike-said", Recall, "that email where mike said the rent was going up", KI, lease()),
        q("rc-door-code", Recall, "the one with the door code", KI, p("plant:cabin")),
        q("rc-boiler", Recall, "the email about the boiler", KI, p("plant:plumber")),
        q("rc-laptop-desk", Recall, "something about my laptop waiting at the front desk", KI, p("plant:laptop")),
        q("rc-greenleaf", Recall, "the greenleaf person asking about seat pricing", KI, p("plant:obrien")),
        q("rc-ja-meeting-room", Recall, "japanese email about booking a meeting room", XL, p("plant:tanaka-cjk")),
        q("rc-nordlys-contact", Recall, "who is our contact at nordlys?", QU, p("plant:soren")),
        q("rc-proofs", Recall, "what did julia think of the colour proofs?", QU, p("plant:colour-proofs")),
        q("rc-lamp-tracking", Recall, "what's the tracking number for the lamp?", QU, vec![(t("plant:lamp-ship"), 3), (t("plant:lamp-order"), 1)]),
        q("rc-madrid-hotel-cost", Recall, "how much was the hotel in madrid?", XLQ, p("plant:hotel-madrid")),
        q("rc-movers", Recall, "when are the movers coming?", QU, p("plant:movers")),
        q("rc-will", Recall, "where's the draft of my will?", QU, p("plant:kowalski")),
        q("rc-surprise", Recall, "the surprise party for sofia", KI, p("plant:party")),
        q("rc-jose-photos", Recall, "did josé send the photos from cádiz?", XLQ, p("plant:jose-cadiz")),
        q("rc-sample-address", Recall, "what's the delivery address for the sample cards?", XLQ, p("plant:muller")),
        q("rc-london-phone", Recall, "what number do I call if I arrive late at the london hotel?", QU, p("plant:london-hotel")),
        q("rc-version", Recall, "what version are we deploying tonight?", QU, p("plant:deploy-version")),
        q("rc-bag", Recall, "what's my case number for the lost bag?", QU, p("plant:baggage")),
        q("rc-blood-test", Recall, "what did the doctor say about my blood test?", QU, p("plant:evans")),
        q("rc-books-cancelled", Recall, "why was my book order cancelled?", QU, p("plant:uk-cancelled")),
        q("rc-xl-cadiz", Recall, "photos from the cadiz trip", XL, p("plant:jose-cadiz")),
        q("rc-xl-es-meeting", Recall, "reserva de sala de reuniones en tokio", XL, p("plant:tanaka-cjk")),
        q("rc-xl-fr-hotel", Recall, "réservation hôtel madrid", XL, vec![(t("plant:hotel-madrid"), 3), (all([t("kind:hotel"), t("city:madrid")]), 2)]),
        q("rc-xl-de-address", Recall, "delivery address in munich", XL, p("plant:muller")),
        q("rc-xl-es-flat", Recall, "cuánto cuesta el piso en ruzafa", XLQ, p("plant:valencia-flat")),
        q("rc-compost-day", Recall, "which day are compost bins collected now?", QU, p("plant:compost")),
        q("rc-kestrel", Recall, "is the kestrel cluster down?", QU, vec![(t("plant:kestrel-early"), 3), (t("plant:kestrel-late"), 2)]),
        q("rc-florist", Recall, "did I order flowers for the anniversary?", QU, p("plant:birch-bloom")),
    ]
}
