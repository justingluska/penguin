//! The labelled query set: ~200 queries over the synthetic mailbox, each
//! with one category, optional facets and graded judgment rules.
//!
//! Grades: 3 = the item / the answer, 2 = highly relevant, 1 = related (see
//! judge.rs). Plants are `plant:<id>` (corpus/planted.rs); everything else
//! is a rule over background facts, so judgments cover every conversation
//! that shares the concept.
//!
//! Adding a query: pick the category by what the query tests, not what it
//! looks like; write the rules from the user's need; run `search-eval check`
//! to confirm every query has something to find and every tag exists.

use crate::judge::Category::*;
use crate::judge::*;
use crate::window::Window::*;

#[rustfmt::skip]
fn q(id: &'static str, category: Category, text: &'static str, facets: &'static [&'static str], rules: Vec<(Cond, u8)>) -> QuerySpec {
    QuerySpec { id, text: text.to_string(), category, facets, rules }
}

const KI: &[&str] = &["known-item"];
const XL: &[&str] = &["cross-lingual"];
const XLQ: &[&str] = &["cross-lingual", "question"];
const OP: &[&str] = &["operator"];
const BROAD: &[&str] = &["broad"];
const NONE: &[&str] = &[];

/// Every judged query. `now` is the corpus's own "now" (`Corpus::now`):
/// the date-format queries spell out dates relative to it.
pub fn all_queries(now: i64) -> Vec<QuerySpec> {
    let mut v = Vec::new();
    v.extend(operator());
    v.extend(names());
    v.extend(identifiers());
    v.extend(natural());
    v.extend(paraphrase());
    v.extend(questions());
    v.extend(misspellings());
    v.extend(spanish());
    v.extend(time());
    v.extend(crate::edge_queries::edge_queries(now));
    v
}

#[rustfmt::skip]
fn operator() -> Vec<QuerySpec> {
    vec![
        q("op-mike-lease", Operator, "from:mike lease", KI, vec![(t("plant:lease"), 3), (t("plant:parking"), 1), (t("plant:plumber"), 1)]),
        q("op-lease-pdf", Operator, "lease has:pdf", KI, vec![(t("plant:lease"), 3)]),
        q("op-theo-pdf", Operator, "from:theo has:pdf", BROAD, vec![(t("pdf-from:theo@crestline.example"), 2)]),
        q("op-theo-pastdue", Operator, "from:theo \"past due\"", KI, vec![(t("plant:inv-20417"), 3)]),
        q("op-deskcraft", Operator, "from:deskcraft", KI, vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 1)]),
        q("op-kwame-atlas", Operator, "from:kwame atlas approved", KI, vec![(t("plant:atlas-budget"), 3), (all([t("person:kwame"), t("topic:budget"), t("project:atlas")]), 1)]),
        q("op-priya-launch", Operator, "from:priya launch", BROAD, vec![(all([t("from:priya.shah@northwind.example"), t("topic:launch")]), 2), (all([t("person:priya"), t("topic:launch")]), 1)]),
        q("op-invite-hannah", Operator, "has:invite from:hannah", BROAD, vec![(all([t("kind:invite"), t("person:hannah")]), 2)]),
        q("op-invite-design", Operator, "has:invite design review", BROAD, vec![(all([t("kind:invite"), t("topic:design")]), 2)]),
        q("op-newsletter-sourdough", Operator, "is:newsletter sourdough", KI, vec![(t("plant:sourdough"), 3)]),
        q("op-panpantry-pasta", Operator, "from:panpantry pasta", NONE, vec![(all([t("newsletter:panpantry"), subj("pasta")]), 2)]),
        q("op-filename-inv", Operator, "filename:INV-20417", KI, vec![(t("plant:inv-20417"), 3)]),
        q("op-filename-forecast", Operator, "filename:forecast", BROAD, vec![(file("forecast"), 2)]),
        q("op-account-studio-invoice", Operator, "account:studio invoice", BROAD, vec![(all([t("kind:invoice"), t("account:studio")]), 2)]),
        q("op-sent-fernhill-invoice", Operator, "in:sent invoice fernhill", BROAD, vec![(all([t("kind:invoice"), t("direction:out"), t("person:julia")]), 2)]),
        q("op-to-julia-logo", Operator, "to:julia logo", NONE, vec![(t("plant:fernhill-final"), 3), (all([t("person:julia"), t("topic:logo"), t("has:sent")]), 2), (all([t("person:julia"), t("topic:logo")]), 1)]),
        q("op-subject-retreat", Operator, "subject:retreat", KI, vec![(t("plant:retreat"), 3)]),
        q("op-ledgerly-code", Operator, "from:ledgerly code", &["recency"], vec![(t("plant:otp-bank"), 3), (all([t("kind:code"), t("merchant:ledgerly")]), 1)]),
        q("op-pdf-tax-return", Operator, "has:pdf tax return", KI, vec![(t("plant:tax"), 3)]),
        q("op-larger-board", Operator, "larger:5M board", KI, vec![(t("plant:board-deck"), 3)]),
        q("op-promo-gearloft", Operator, "category:promotions gearloft", BROAD, vec![(all([t("kind:promotion"), t("merchant:gearloft")]), 2)]),
        q("op-or-invoices", Operator, "(from:marco OR from:julia) invoice", BROAD, vec![(all([t("kind:invoice"), any([t("person:marco"), t("person:julia")])]), 2)]),
        q("op-invoice-not-crestline", Operator, "invoice -crestline", BROAD, vec![(all([t("kind:invoice"), not(t("person:theo"))]), 2)]),
        q("op-work-flight", Operator, "account:work flight", BROAD, vec![(all([t("kind:flight"), t("account:work")]), 2)]),
        q("op-has-pdf-msa", Operator, "has:pdf from:ana agreement", KI, vec![(t("plant:linden-msa"), 3)]),
        q("op-from-me-pdf", Operator, "from:me has:pdf fernhill", NONE, vec![(t("plant:fernhill-final"), 3), (all([t("pdf-from:hello@morenostudio.example"), t("person:julia")]), 2)]),
    ]
}

#[rustfmt::skip]
fn names() -> Vec<QuerySpec> {
    vec![
        q("nm-priya", Name, "priya", BROAD, vec![(t("from:priya.shah@northwind.example"), 2), (t("person:priya"), 1)]),
        q("nm-priya-shah", Name, "Priya Shah", BROAD, vec![(t("from:priya.shah@northwind.example"), 2), (t("person:priya"), 1)]),
        q("nm-kwame", Name, "kwame mensah", BROAD, vec![(t("from:kwame.mensah@northwind.example"), 2), (t("person:kwame"), 1)]),
        q("nm-mike-delgado", Name, "mike delgado", NONE, vec![(t("from:mike.delgado@harborrealty.example"), 2), (t("person:mike-d"), 1)]),
        q("nm-mike", Name, "mike", BROAD, vec![(any([t("from:mike.delgado@harborrealty.example"), t("from:mike.chen@northwind.example")]), 2), (any([t("person:mike-d"), t("person:mike-w")]), 1)]),
        q("nm-theo", Name, "theo laurent", BROAD, vec![(t("from:theo@crestline.example"), 2), (t("person:theo"), 1)]),
        q("nm-crestline", Name, "crestline", BROAD, vec![(t("person:theo"), 2), (t("topic:vendor"), 1)]),
        q("nm-jess", Name, "jess", BROAD, vec![(t("from:jesspark@inbox.example"), 2), (t("person:jess"), 1)]),
        q("nm-carmen", Name, "carmen moreno", BROAD, vec![(t("from:carmen.moreno@correo.example"), 2), (t("person:carmen"), 1)]),
        q("nm-grace-liu", Name, "grace liu", KI, vec![(t("plant:tax"), 3)]),
        q("nm-dr-patel", Name, "dr patel", KI, vec![(t("plant:dental"), 3)]),
        q("nm-bianchi", Name, "bianchi wines", BROAD, vec![(t("person:marco"), 2)]),
        q("nm-irene", Name, "irene castillo", BROAD, vec![(t("person:irene"), 2)]),
        q("nm-linden", Name, "linden partners", BROAD, vec![(t("person:ana"), 2), (all([t("topic:customer"), subj("linden")]), 1)]),
        q("nm-address", Name, "mike.delgado@harborrealty.example", NONE, vec![(t("from:mike.delgado@harborrealty.example"), 2)]),
        q("nm-omar", Name, "omar haddad", BROAD, vec![(t("from:omar.haddad@northwind.example"), 2), (t("person:omar"), 1)]),
        q("nm-dana", Name, "dana", BROAD, vec![(t("from:dana.whitfield@inbox.example"), 2), (any([t("person:dana"), subj("dana")]), 1)]),
    ]
}

#[rustfmt::skip]
fn identifiers() -> Vec<QuerySpec> {
    vec![
        q("id-pnr", Identifier, "K7QX2M", KI, vec![(t("plant:lisbon-flight"), 3)]),
        q("id-pnr-lower", Identifier, "k7qx2m", KI, vec![(t("plant:lisbon-flight"), 3)]),
        q("id-order", Identifier, "DC-55120", KI, vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 2)]),
        q("id-order-digits", Identifier, "55120", KI, vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 2)]),
        q("id-tracking", Identifier, "SW4829105533", KI, vec![(t("plant:desk-ship"), 3)]),
        q("id-invoice", Identifier, "INV-20417", KI, vec![(t("plant:inv-20417"), 3)]),
        q("id-invoice-digits", Identifier, "20417", KI, vec![(t("plant:inv-20417"), 3)]),
        q("id-otp", Identifier, "604218", KI, vec![(t("plant:otp-bank"), 3)]),
        q("id-otp-es", Identifier, "771903", KI, vec![(t("plant:otp-es"), 3)]),
        q("id-claim", Identifier, "RI-CLM-30918", KI, vec![(t("plant:claim"), 3)]),
        q("id-phone", Identifier, "555-0138", KI, vec![(t("plant:dana-number"), 3)]),
        q("id-phone-full", Identifier, "(415) 555-0138", KI, vec![(t("plant:dana-number"), 3)]),
        q("id-amount", Identifier, "$649.00", KI, vec![(t("plant:desk-order"), 3)]),
        q("id-amount-comma", Identifier, "1,284", KI, vec![(t("plant:tax"), 3)]),
        q("id-door-code", Identifier, "4471", KI, vec![(t("plant:cabin"), 3)]),
        q("id-wifi-password", Identifier, "sunflower-42", KI, vec![(t("plant:cabin"), 3)]),
        q("id-hotel-ref", Identifier, "NA-883104", KI, vec![(t("plant:lisbon-stay"), 3)]),
        q("id-ticket", Identifier, "TK-661902", KI, vec![(t("plant:tickets"), 3)]),
        q("id-juror", Identifier, "7730214", KI, vec![(t("plant:jury"), 3)]),
        q("id-passport-file", Identifier, "59-2210-448", KI, vec![(t("plant:passport"), 3)]),
        q("id-studio-invoice", Identifier, "MS-2026-007", NONE, vec![(t("plant:bianchi-late"), 3), (t("invoice:ms-2026-007"), 2)]),
        q("id-flight-no", Identifier, "AV 118", KI, vec![(t("plant:parents-arrive"), 3)]),
        q("id-asset-tag", Identifier, "NW-8816", KI, vec![(t("plant:laptop"), 3)]),
    ]
}

#[rustfmt::skip]
fn natural() -> Vec<QuerySpec> {
    vec![
        q("nl-pdf-mike-lease", Natural, "the pdf mike sent about the lease", KI, vec![(t("plant:lease"), 3), (t("plant:parking"), 1)]),
        q("nl-lease-renewal", Natural, "lease renewal", KI, vec![(t("plant:lease"), 3)]),
        q("nl-cabin-wifi", Natural, "cabin wifi password", KI, vec![(t("plant:cabin"), 3)]),
        q("nl-board-deck", Natural, "final board deck", KI, vec![(t("plant:board-deck"), 3)]),
        q("nl-signed-msa", Natural, "signed MSA from linden", KI, vec![(t("plant:linden-msa"), 3)]),
        q("nl-return-filed", Natural, "2025 return filed", KI, vec![(t("plant:tax"), 3)]),
        q("nl-deskcraft-order", Natural, "deskcraft order confirmation", KI, vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 1)]),
        q("nl-crestline-past-due", Natural, "crestline invoice past due", KI, vec![(t("plant:inv-20417"), 3), (all([t("kind:invoice"), t("person:theo")]), 1)]),
        q("nl-chicago-expenses", Natural, "chicago expense report", KI, vec![(t("plant:expense-returned"), 3), (all([t("topic:expense"), subj("chicago")]), 1)]),
        q("nl-fjordsoft-sunset", Natural, "fjordsoft v1 sunset", KI, vec![(t("plant:api-sunset"), 3)]),
        q("nl-plumber-thursday", Natural, "plumber thursday", KI, vec![(t("plant:plumber"), 3)]),
        q("nl-save-the-date", Natural, "ben and maya save the date", KI, vec![(t("plant:wedding"), 3)]),
        q("nl-neon-harbor", Natural, "neon harbor tickets", KI, vec![(t("plant:tickets"), 3)]),
        q("nl-bike-ready", Natural, "bike ready for pickup", KI, vec![(t("plant:bike"), 3)]),
        q("nl-phishing", Natural, "phishing emails", KI, vec![(t("plant:phishing"), 3)]),
        q("nl-staff-engineer", Natural, "promotion to staff engineer", KI, vec![(t("plant:promotion"), 3)]),
        q("nl-pine-lodge-rooms", Natural, "pine lodge rooms list", KI, vec![(t("plant:retreat"), 3)]),
        q("nl-pager-november", Natural, "pager rotation november", KI, vec![(t("plant:oncall"), 3)]),
        q("nl-devharbor-accepted", Natural, "devharbor proposal accepted", KI, vec![(t("plant:talk"), 3)]),
        q("nl-headphones-refund", Natural, "headphones refund", KI, vec![(t("plant:refund"), 3)]),
        q("nl-casa-do-rio", Natural, "casa do rio booking", KI, vec![(t("plant:lisbon-stay"), 3)]),
        q("nl-swiftcab-receipt", Natural, "swiftcab receipt", BROAD, vec![(all([t("kind:receipt"), t("merchant:swiftcab")]), 2)]),
        q("nl-rabies-booster", Natural, "biscuit rabies booster", KI, vec![(t("plant:vet"), 3)]),
        q("nl-atlas-forecast", Natural, "atlas forecast spreadsheet", NONE, vec![(all([t("project:atlas"), file("forecast")]), 3), (all([t("project:atlas"), t("topic:budget")]), 1)]),
    ]
}

#[rustfmt::skip]
fn paraphrase() -> Vec<QuerySpec> {
    vec![
        q("pa-water-heater", Paraphrase, "water heater replacement", KI, vec![(t("plant:plumber"), 3)]),
        q("pa-plane-portugal", Paraphrase, "plane tickets to portugal", NONE, vec![(t("plant:lisbon-flight"), 3), (all([t("kind:flight"), t("country:portugal")]), 2), (t("plant:lisbon-stay"), 1)]),
        q("pa-hotel-lisbon", Paraphrase, "hotel in lisbon", NONE, vec![(t("plant:lisbon-stay"), 3), (all([t("kind:hotel"), t("city:lisbon")]), 2), (t("plant:lisbon-flight"), 1)]),
        q("pa-standing-desk", Paraphrase, "standing desk receipt", KI, vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 1)]),
        q("pa-dog-shots", Paraphrase, "dog vaccinations", KI, vec![(t("plant:vet"), 3)]),
        q("pa-dentist-visit", Paraphrase, "dentist visit", KI, vec![(t("plant:dental"), 3), (t("plant:dental-bill"), 1)]),
        q("pa-tax-refund", Paraphrase, "tax refund from the accountant", KI, vec![(t("plant:tax"), 3)]),
        q("pa-company-offsite", Paraphrase, "company offsite", KI, vec![(t("plant:retreat"), 3)]),
        q("pa-budget-signoff", Paraphrase, "atlas budget sign-off", KI, vec![(t("plant:atlas-budget"), 3), (all([t("topic:budget"), t("project:atlas")]), 1)]),
        q("pa-scam", Paraphrase, "scam email warning", KI, vec![(t("plant:phishing"), 3)]),
        q("pa-vacation", Paraphrase, "vacation approved", KI, vec![(t("plant:pto"), 3)]),
        q("pa-new-computer", Paraphrase, "new computer pickup", KI, vec![(t("plant:laptop"), 3)]),
        q("pa-api-shutdown", Paraphrase, "fjordsoft api shutdown date", KI, vec![(t("plant:api-sunset"), 3)]),
        q("pa-linden-contract", Paraphrase, "linden contract signed", KI, vec![(t("plant:linden-msa"), 3)]),
        q("pa-late-payment", Paraphrase, "crestline late payment", KI, vec![(t("plant:inv-20417"), 3), (all([t("kind:invoice"), t("person:theo")]), 1)]),
        q("pa-slides", Paraphrase, "slides for the board meeting", KI, vec![(t("plant:board-deck"), 3)]),
        q("pa-db-migration", Paraphrase, "database migration schedule", KI, vec![(t("plant:cutover"), 3)]),
        q("pa-reimbursement", Paraphrase, "reimbursement rejected", KI, vec![(t("plant:expense-returned"), 3)]),
        q("pa-bluepeak-leaving", Paraphrase, "bluepeak cancelling", KI, vec![(t("plant:bluepeak-churn"), 3), (all([t("topic:customer"), t("person:ravi")]), 1)]),
        q("pa-japan-visa", Paraphrase, "japan visa letter", KI, vec![(t("plant:visa"), 3)]),
        q("pa-bicycle", Paraphrase, "bicycle repair", KI, vec![(t("plant:bike"), 3)]),
        q("pa-jury-duty", Paraphrase, "jury duty", KI, vec![(t("plant:jury"), 3)]),
        q("pa-money-back", Paraphrase, "money back for headphones", KI, vec![(t("plant:refund"), 3)]),
        q("pa-lake-pictures", Paraphrase, "pictures from the lake", KI, vec![(t("plant:tahoe"), 3)]),
        q("pa-no-knead", Paraphrase, "bread without kneading", KI, vec![(t("plant:sourdough"), 3)]),
        q("pa-marathon-rest", Paraphrase, "resting before a marathon", KI, vec![(t("plant:taper"), 3)]),
        q("pa-electricity", Paraphrase, "electricity bill", BROAD, vec![(all([t("kind:bill"), t("merchant:cedarvalley")]), 2)]),
        q("pa-cell-bill", Paraphrase, "cell phone bill", BROAD, vec![(all([t("kind:bill"), t("merchant:harbormobile")]), 2)]),
        q("pa-spain-flights", Paraphrase, "flights to spain", BROAD, vec![(all([t("kind:flight"), t("country:spain")]), 2), (all([t("kind:hotel"), t("country:spain")]), 1)]),
        q("pa-gym", Paraphrase, "gym cancellation", KI, vec![(t("plant:gym"), 3)]),
        q("pa-domain-lapse", Paraphrase, "website address about to lapse", KI, vec![(t("plant:domain"), 3)]),
        q("pa-landlord-repair", Paraphrase, "landlord repair visit", KI, vec![(t("plant:plumber"), 3), (all([t("person:mike-d"), t("topic:housing")]), 1)]),
        q("pa-water-damage", Paraphrase, "flooded kitchen insurance", KI, vec![(t("plant:claim"), 3)]),
    ]
}

#[rustfmt::skip]
fn questions() -> Vec<QuerySpec> {
    let qq: &[&str] = &["question"];
    vec![
        q("qu-lisbon-leave", Question, "when does my flight to lisbon leave?", qq, vec![(t("plant:lisbon-flight"), 3), (all([t("kind:flight"), t("city:lisbon")]), 1)]),
        q("qu-rent", Question, "how much is the new rent?", qq, vec![(t("plant:lease"), 3)]),
        q("qu-cabin-wifi", Question, "what's the wifi password at the cabin?", qq, vec![(t("plant:cabin"), 3)]),
        q("qu-desk-cost", Question, "how much did I pay for the standing desk?", qq, vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 1)]),
        q("qu-desk-arrive", Question, "when is the desk arriving?", qq, vec![(t("plant:desk-ship"), 3), (t("plant:desk-order"), 1)]),
        q("qu-wedding", Question, "when is ben's wedding?", qq, vec![(t("plant:wedding"), 3)]),
        q("qu-lisbon-stay", Question, "where am I staying in lisbon?", qq, vec![(t("plant:lisbon-stay"), 3), (t("plant:lisbon-flight"), 1)]),
        q("qu-oncall", Question, "when am I on call?", qq, vec![(t("plant:oncall"), 3)]),
        q("qu-atlas-approved", Question, "did finance approve the atlas budget?", qq, vec![(t("plant:atlas-budget"), 3), (all([t("topic:budget"), t("project:atlas")]), 1)]),
        q("qu-migration", Question, "when is the database migration?", qq, vec![(t("plant:cutover"), 3)]),
        q("qu-expense", Question, "why was my expense report sent back?", qq, vec![(t("plant:expense-returned"), 3)]),
        q("qu-bianchi-pay", Question, "when will bianchi pay?", qq, vec![(t("plant:bianchi-late"), 3), (all([t("kind:invoice"), t("person:marco")]), 1)]),
        q("qu-dana-number", Question, "what is dana's new number?", qq, vec![(t("plant:dana-number"), 3)]),
        q("qu-vet", Question, "when is biscuit due at the vet?", qq, vec![(t("plant:vet"), 3)]),
        q("qu-tax-refund", Question, "how big is my tax refund?", qq, vec![(t("plant:tax"), 3)]),
        q("qu-seats", Question, "which seats did we get for neon harbor?", qq, vec![(t("plant:tickets"), 3)]),
        q("qu-jury", Question, "when do I report for jury duty?", qq, vec![(t("plant:jury"), 3)]),
        q("qu-retreat", Question, "where is the team retreat?", qq, vec![(t("plant:retreat"), 3)]),
        q("qu-winter-vacation", Question, "when is my winter vacation?", qq, vec![(t("plant:pto"), 3)]),
        q("qu-api-off", Question, "when is fjordsoft turning off the old api?", qq, vec![(t("plant:api-sunset"), 3)]),
        q("qu-talk", Question, "did my devharbor talk get accepted?", qq, vec![(t("plant:talk"), 3)]),
        q("qu-bluepeak", Question, "is bluepeak renewing?", qq, vec![(t("plant:bluepeak-churn"), 3), (all([t("topic:customer"), t("person:ravi")]), 1)]),
        q("qu-self-review", Question, "when is my self review due?", qq, vec![(t("plant:self-review"), 3)]),
        q("qu-bike-cost", Question, "how much was the bike repair?", qq, vec![(t("plant:bike"), 3)]),
        q("qu-plumber", Question, "what time is the plumber coming?", qq, vec![(t("plant:plumber"), 3)]),
        q("qu-adjuster", Question, "who is the adjuster on my insurance claim?", qq, vec![(t("plant:claim"), 3)]),
        q("qu-parking", Question, "which parking spot is mine?", qq, vec![(t("plant:parking"), 3)]),
        q("qu-passport", Question, "how long will my passport renewal take?", qq, vec![(t("plant:passport"), 3)]),
    ]
}

#[rustfmt::skip]
fn misspellings() -> Vec<QuerySpec> {
    vec![
        q("mi-lisbon", Misspelling, "lisbn flight", NONE, vec![(t("plant:lisbon-flight"), 3), (all([t("kind:flight"), t("city:lisbon")]), 2)]),
        q("mi-receipt", Misspelling, "recipt deskcraft", KI, vec![(t("plant:desk-order"), 3), (t("plant:desk-ship"), 1)]),
        q("mi-invoice", Misspelling, "invioce crestline", BROAD, vec![(all([t("kind:invoice"), t("person:theo")]), 2)]),
        q("mi-dental", Misspelling, "dentel appointment", KI, vec![(t("plant:dental"), 3)]),
        q("mi-paella", Misspelling, "paela recipe", NONE, vec![(t("plant:paella"), 3), (all([t("topic:receta"), subj("paella")]), 2), (t("topic:receta"), 1)]),
        q("mi-wifi", Misspelling, "wifi pasword", KI, vec![(t("plant:cabin"), 3)]),
        q("mi-wedding", Misspelling, "ben okafor weding", KI, vec![(t("plant:wedding"), 3)]),
        q("mi-jury", Misspelling, "juror sumons", KI, vec![(t("plant:jury"), 3)]),
        q("mi-biscuit", Misspelling, "biscut booster", KI, vec![(t("plant:vet"), 3)]),
        q("mi-retreat", Misspelling, "team retreet", KI, vec![(t("plant:retreat"), 3)]),
        q("mi-kwame", Misspelling, "kwame mensa", BROAD, vec![(t("from:kwame.mensah@northwind.example"), 2), (t("person:kwame"), 1)]),
        q("mi-priya-budget", Misspelling, "pryia budget", BROAD, vec![(all([t("person:priya"), t("topic:budget")]), 2)]),
        q("mi-phishing", Misspelling, "fishing emails", KI, vec![(t("plant:phishing"), 3)]),
        q("mi-passport", Misspelling, "pasport renewal", KI, vec![(t("plant:passport"), 3)]),
        q("mi-sourdough", Misspelling, "sour dough", KI, vec![(t("plant:sourdough"), 3)]),
        q("mi-lease", Misspelling, "leese renewal", KI, vec![(t("plant:lease"), 3)]),
        q("mi-swiftcab", Misspelling, "swift cab receipts", BROAD, vec![(all([t("kind:receipt"), t("merchant:swiftcab")]), 2)]),
    ]
}

#[rustfmt::skip]
fn spanish() -> Vec<QuerySpec> {
    vec![
        q("es-paella", Spanish, "receta paella abuela", NONE, vec![(t("plant:paella"), 3), (all([t("topic:receta"), subj("paella")]), 2), (t("topic:receta"), 1)]),
        q("es-paella-en", Spanish, "grandma's paella recipe", XL, vec![(t("plant:paella"), 3), (all([t("topic:receta"), subj("paella")]), 2), (t("topic:receta"), 1)]),
        q("es-mama-llega", Spanish, "¿a qué hora llega mamá?", &["question"], vec![(t("plant:parents-arrive"), 3)]),
        q("es-parents-land", Spanish, "when do my parents land?", XLQ, vec![(t("plant:parents-arrive"), 3)]),
        q("es-piso-valencia", Spanish, "alquiler piso valencia", KI, vec![(t("plant:valencia-flat"), 3)]),
        q("es-valencia-apartment", Spanish, "apartment in valencia for july", XL, vec![(t("plant:valencia-flat"), 3)]),
        q("es-presupuesto", Spanish, "presupuesto web casa castillo", KI, vec![(t("plant:castillo-quote"), 3), (all([t("person:irene"), t("topic:website")]), 1)]),
        q("es-castillo-quote", Spanish, "casa castillo website quote", XL, vec![(t("plant:castillo-quote"), 3), (all([t("person:irene"), t("topic:website")]), 1)]),
        q("es-codigo", Spanish, "código banco solaris", &["recency"], vec![(t("plant:otp-es"), 3), (all([t("kind:code"), t("merchant:solaris")]), 1)]),
        q("es-bank-code-en", Spanish, "spanish bank verification code", XL, vec![(t("plant:otp-es"), 3), (all([t("kind:code"), t("merchant:solaris")]), 2)]),
        q("es-vuelo-lisboa", Spanish, "vuelo a lisboa", XL, vec![(t("plant:lisbon-flight"), 3), (all([t("kind:flight"), t("city:lisbon")]), 2)]),
        q("es-reserva-madrid", Spanish, "reserva vuelo madrid", BROAD, vec![(all([t("kind:flight"), t("city:madrid")]), 2), (all([t("kind:hotel"), t("city:madrid")]), 1)]),
        q("es-pedido", Spanish, "pedido mercado sol", BROAD, vec![(t("merchant:mercadosol"), 2)]),
        q("es-seguimiento", Spanish, "seguimiento del paquete", BROAD, vec![(all([t("kind:shipping"), t("lang:es")]), 2), (t("kind:shipping"), 1)]),
        q("es-cumple-abuela", Spanish, "cumpleaños de la abuela", BROAD, vec![(t("topic:cumple"), 2)]),
        q("es-nochebuena", Spanish, "cena de nochebuena", BROAD, vec![(t("topic:navidad"), 2)]),
        q("es-factura-luz", Spanish, "factura de la luz", XL, vec![(all([t("kind:bill"), t("merchant:cedarvalley")]), 2)]),
        q("es-boletin", Spanish, "boletín el pulso", BROAD, vec![(t("newsletter:elpulso"), 2)]),
    ]
}

#[rustfmt::skip]
fn time() -> Vec<QuerySpec> {
    vec![
        q("ti-dental-op", Time, "dental date:\"last spring\"", OP, vec![(t("plant:dental"), 3)]),
        q("ti-dentist-words", Time, "dentist last spring", KI, vec![(t("plant:dental"), 3)]),
        q("ti-lease-words", Time, "lease from last spring", KI, vec![(t("plant:lease"), 3)]),
        q("ti-swiftcab-words", Time, "swiftcab rides last month", BROAD, vec![(all([t("merchant:swiftcab"), t("kind:receipt"), within(LastMonth)]), 2)]),
        q("ti-swiftcab-op", Time, "from:swiftcab date:\"last month\"", OP, vec![(all([t("merchant:swiftcab"), within(LastMonth)]), 2)]),
        q("ti-electric-march", Time, "electricity bill from march", NONE, vec![(all([t("merchant:cedarvalley"), t("kind:bill"), within(Month(3))]), 3), (all([t("merchant:cedarvalley"), t("kind:bill")]), 1)]),
        q("ti-electric-march-op", Time, "from:\"cedar valley\" date:march", OP, vec![(all([t("merchant:cedarvalley"), t("kind:bill"), within(Month(3))]), 3)]),
        q("ti-summer-photos", Time, "photos from last summer", NONE, vec![(t("plant:tahoe"), 3), (all([any([t("topic:photos"), t("topic:fotos")]), within(LastSummer)]), 2)]),
        q("ti-accountant-spring", Time, "what did the accountant send last spring", KI, vec![(t("plant:tax"), 3)]),
        q("ti-invoices-last-year", Time, "invoices I sent last year", BROAD, vec![(all([t("kind:invoice"), t("direction:out"), within(LastYear)]), 2)]),
        q("ti-invoices-last-year-op", Time, "in:sent invoice date:\"last year\"", OP, vec![(all([t("kind:invoice"), t("direction:out"), within(LastYear)]), 2)]),
        q("ti-flights-last-summer", Time, "flights last summer", BROAD, vec![(all([t("kind:flight"), touches(LastSummer)]), 2)]),
        q("ti-gearloft-march", Time, "gearloft order in march", BROAD, vec![(all([t("merchant:gearloft"), t("kind:receipt"), within(Month(3))]), 2), (all([t("merchant:gearloft"), t("kind:shipping"), within(Month(3))]), 1)]),
        q("ti-latest-bank-code", Time, "latest bank verification code", &["recency"], vec![(t("plant:otp-bank"), 3), (all([t("kind:code"), t("merchant:ledgerly")]), 1)]),
    ]
}
