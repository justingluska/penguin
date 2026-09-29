//! Question → [`AskQuery`], compositionally: instead of one template per
//! phrasing, the question's parts are read separately and combined.
//!
//! 1. Words are normalized as for the templates (`intent::normalize`:
//!    case, accents, contractions, Spanish date phrases).
//! 2. Comparisons ("in July or August", "Lisbon vs Madrid") and date
//!    phrases anywhere in the question are taken out and kept as written;
//!    `query::read_timeframe` reads them with the date grammar.
//! 3. Each remaining word is looked up in a small lexicon: verbs and nouns
//!    name the subject ("fly", "flew", "trip", "vuelo" → flights; "spend",
//!    "paid" → money), cue words name the operation ("how many" → count,
//!    "how much" → sum, "which month … most" → group by month + max,
//!    "did I ever" → exists, "first/last/next", "average", "cheapest").
//! 4. What follows a preposition ("to Lisbon", "at Swiftcab", "from
//!    Priya") is an entity: a place when the airport table knows it,
//!    otherwise a merchant (or, for messages, a person). The mailbox checks
//!    it when the query runs.
//!
//! A question is taken only when it names a subject and every content word
//! was understood; anything else stays with the templates and passages.

use chrono::NaiveDate;

use super::intent;
use super::query::*;
use crate::structured::airports_for_place;

/// How a word counts toward the subject.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    /// A noun naming the thing ("flights", "hotel", "pedidos").
    Noun(QuerySubject),
    /// A verb implying it ("flew", "stayed", "ordered").
    Verb(QuerySubject),
    /// Money ("spend", "paid", "cost").
    Money,
    /// Hotel nights.
    Nights,
    /// Trips (bookings of flights).
    Trips,
}

/// Word (or phrase) → role. Phrases are matched longest first.
const LEXICON: &[(&str, Role)] = &[
    // flights
    ("flights", Role::Noun(QuerySubject::Flights)),
    ("flight", Role::Noun(QuerySubject::Flights)),
    ("plane", Role::Noun(QuerySubject::Flights)),
    ("planes", Role::Noun(QuerySubject::Flights)),
    ("plane tickets", Role::Noun(QuerySubject::Flights)),
    ("airfare", Role::Noun(QuerySubject::Flights)),
    ("air travel", Role::Noun(QuerySubject::Flights)),
    ("vuelos", Role::Noun(QuerySubject::Flights)),
    ("vuelo", Role::Noun(QuerySubject::Flights)),
    ("fly", Role::Verb(QuerySubject::Flights)),
    ("flew", Role::Verb(QuerySubject::Flights)),
    ("flown", Role::Verb(QuerySubject::Flights)),
    ("flying", Role::Verb(QuerySubject::Flights)),
    ("flies", Role::Verb(QuerySubject::Flights)),
    ("volar", Role::Verb(QuerySubject::Flights)),
    ("vole", Role::Verb(QuerySubject::Flights)),
    ("volamos", Role::Verb(QuerySubject::Flights)),
    ("volado", Role::Verb(QuerySubject::Flights)),
    ("trips", Role::Trips),
    ("trip", Role::Trips),
    ("viajes", Role::Trips),
    ("viaje", Role::Trips),
    ("travel", Role::Verb(QuerySubject::Flights)),
    ("land", Role::Verb(QuerySubject::Flights)),
    ("lands", Role::Verb(QuerySubject::Flights)),
    ("landing", Role::Verb(QuerySubject::Flights)),
    ("aterrizo", Role::Verb(QuerySubject::Flights)),
    ("traveled", Role::Verb(QuerySubject::Flights)),
    ("travelled", Role::Verb(QuerySubject::Flights)),
    ("viajamos", Role::Trips),
    ("viajado", Role::Trips),
    // stays
    ("hotel stays", Role::Noun(QuerySubject::Stays)),
    ("hotels", Role::Noun(QuerySubject::Stays)),
    ("hotel", Role::Noun(QuerySubject::Stays)),
    ("stays", Role::Noun(QuerySubject::Stays)),
    ("airbnb", Role::Noun(QuerySubject::Stays)),
    ("airbnbs", Role::Noun(QuerySubject::Stays)),
    ("lodging", Role::Noun(QuerySubject::Stays)),
    ("accommodation", Role::Noun(QuerySubject::Stays)),
    ("accommodations", Role::Noun(QuerySubject::Stays)),
    ("hoteles", Role::Noun(QuerySubject::Stays)),
    ("alojamiento", Role::Noun(QuerySubject::Stays)),
    ("estancias", Role::Noun(QuerySubject::Stays)),
    ("stay", Role::Verb(QuerySubject::Stays)),
    ("stayed", Role::Verb(QuerySubject::Stays)),
    ("staying", Role::Verb(QuerySubject::Stays)),
    ("hospede", Role::Verb(QuerySubject::Stays)),
    ("hospedado", Role::Verb(QuerySubject::Stays)),
    ("alojado", Role::Verb(QuerySubject::Stays)),
    ("nights", Role::Nights),
    ("night", Role::Nights),
    ("noches", Role::Nights),
    // orders
    ("orders", Role::Noun(QuerySubject::Orders)),
    ("order", Role::Noun(QuerySubject::Orders)),
    ("purchases", Role::Noun(QuerySubject::Orders)),
    ("purchase", Role::Noun(QuerySubject::Orders)),
    ("rides", Role::Noun(QuerySubject::Orders)),
    ("ride", Role::Noun(QuerySubject::Orders)),
    ("receipts", Role::Noun(QuerySubject::Orders)),
    ("receipt", Role::Noun(QuerySubject::Orders)),
    ("pedidos", Role::Noun(QuerySubject::Orders)),
    ("pedido", Role::Noun(QuerySubject::Orders)),
    ("compras", Role::Noun(QuerySubject::Orders)),
    ("compra", Role::Noun(QuerySubject::Orders)),
    ("ordered", Role::Verb(QuerySubject::Orders)),
    ("buy", Role::Verb(QuerySubject::Orders)),
    ("bought", Role::Verb(QuerySubject::Orders)),
    ("purchased", Role::Verb(QuerySubject::Orders)),
    ("compre", Role::Verb(QuerySubject::Orders)),
    ("pedi", Role::Verb(QuerySubject::Orders)),
    ("comprado", Role::Verb(QuerySubject::Orders)),
    // parcels
    ("packages", Role::Noun(QuerySubject::Parcels)),
    ("package", Role::Noun(QuerySubject::Parcels)),
    ("parcels", Role::Noun(QuerySubject::Parcels)),
    ("parcel", Role::Noun(QuerySubject::Parcels)),
    ("deliveries", Role::Noun(QuerySubject::Parcels)),
    ("delivery", Role::Noun(QuerySubject::Parcels)),
    ("shipments", Role::Noun(QuerySubject::Parcels)),
    ("shipment", Role::Noun(QuerySubject::Parcels)),
    ("paquetes", Role::Noun(QuerySubject::Parcels)),
    ("paquete", Role::Noun(QuerySubject::Parcels)),
    ("envios", Role::Noun(QuerySubject::Parcels)),
    ("entregas", Role::Noun(QuerySubject::Parcels)),
    ("delivered", Role::Verb(QuerySubject::Parcels)),
    ("shipped", Role::Verb(QuerySubject::Parcels)),
    // bills
    ("bills", Role::Noun(QuerySubject::Bills)),
    ("bill", Role::Noun(QuerySubject::Bills)),
    ("invoices", Role::Noun(QuerySubject::Bills)),
    ("invoice", Role::Noun(QuerySubject::Bills)),
    ("statements", Role::Noun(QuerySubject::Bills)),
    ("statement", Role::Noun(QuerySubject::Bills)),
    ("utility bills", Role::Noun(QuerySubject::Bills)),
    ("facturas", Role::Noun(QuerySubject::Bills)),
    ("factura", Role::Noun(QuerySubject::Bills)),
    ("recibos", Role::Noun(QuerySubject::Bills)),
    // money paid, whatever the email calls it: "my Streamly charges",
    // "Tunewave payments", "my Linear subscription"
    ("charges", Role::Noun(QuerySubject::Spending)),
    ("payments", Role::Noun(QuerySubject::Spending)),
    ("payment", Role::Noun(QuerySubject::Spending)),
    ("subscriptions", Role::Noun(QuerySubject::Spending)),
    ("subscription", Role::Noun(QuerySubject::Spending)),
    ("memberships", Role::Noun(QuerySubject::Spending)),
    ("membership", Role::Noun(QuerySubject::Spending)),
    ("suscripcion", Role::Noun(QuerySubject::Spending)),
    ("pagos", Role::Noun(QuerySubject::Spending)),
    // bookings
    ("reservations", Role::Noun(QuerySubject::Bookings)),
    ("reservation", Role::Noun(QuerySubject::Bookings)),
    ("dinner reservations", Role::Noun(QuerySubject::Bookings)),
    ("restaurant reservations", Role::Noun(QuerySubject::Bookings)),
    ("tickets", Role::Noun(QuerySubject::Bookings)),
    ("concerts", Role::Noun(QuerySubject::Bookings)),
    ("concert", Role::Noun(QuerySubject::Bookings)),
    ("shows", Role::Noun(QuerySubject::Bookings)),
    ("events", Role::Noun(QuerySubject::Bookings)),
    ("reservas", Role::Noun(QuerySubject::Bookings)),
    ("entradas", Role::Noun(QuerySubject::Bookings)),
    // messages
    ("emails", Role::Noun(QuerySubject::Messages)),
    ("email", Role::Noun(QuerySubject::Messages)),
    ("e-mails", Role::Noun(QuerySubject::Messages)),
    ("messages", Role::Noun(QuerySubject::Messages)),
    ("message", Role::Noun(QuerySubject::Messages)),
    ("mails", Role::Noun(QuerySubject::Messages)),
    ("mail", Role::Noun(QuerySubject::Messages)),
    ("correos", Role::Noun(QuerySubject::Messages)),
    ("correo", Role::Noun(QuerySubject::Messages)),
    ("mensajes", Role::Noun(QuerySubject::Messages)),
    ("emailed", Role::Verb(QuerySubject::Messages)),
    ("emails me", Role::Verb(QuerySubject::Messages)),
    ("email me", Role::Verb(QuerySubject::Messages)),
    ("wrote", Role::Verb(QuerySubject::Messages)),
    ("write", Role::Verb(QuerySubject::Messages)),
    ("written", Role::Verb(QuerySubject::Messages)),
    ("writes", Role::Verb(QuerySubject::Messages)),
    ("messaged", Role::Verb(QuerySubject::Messages)),
    ("hear from", Role::Verb(QuerySubject::Messages)),
    ("heard from", Role::Verb(QuerySubject::Messages)),
    ("escribio", Role::Verb(QuerySubject::Messages)),
    ("escribi", Role::Verb(QuerySubject::Messages)),
    // money
    ("spend", Role::Money),
    ("spent", Role::Money),
    ("spending", Role::Money),
    ("pay", Role::Money),
    ("paid", Role::Money),
    ("cost", Role::Money),
    ("costs", Role::Money),
    ("money", Role::Money),
    ("expenses", Role::Money),
    ("charged", Role::Money),
    ("charge", Role::Money),
    ("gaste", Role::Money),
    ("gastado", Role::Money),
    ("gastamos", Role::Money),
    ("gasto", Role::Money),
    ("gastos", Role::Money),
    ("pague", Role::Money),
    ("pagado", Role::Money),
    ("pagamos", Role::Money),
    ("dinero", Role::Money),
    ("expensive", Role::Money),
    ("caro", Role::Money),
    ("cara", Role::Money),
    ("barato", Role::Money),
    ("cheapest", Role::Money),
];

/// Words that carry no content for the query (after the cues are read).
const GLUE: &[&str] = &[
    "i", "we", "me", "us", "my", "our", "you", "the", "a", "an", "of", "do", "did", "does",
    "done", "have", "has", "had", "is", "are", "was", "were", "am", "be", "been", "will",
    "would", "there", "any", "some", "that", "this", "these", "those", "it", "total", "in",
    "on", "at", "to", "from", "for", "with", "by", "and", "all", "every", "each", "times",
    "time", "take", "took", "taken", "taking", "make", "made", "get", "got", "gotten", "go",
    "went", "gone", "going", "place", "placed", "book", "booked", "receive", "received",
    "many", "much", "how", "what", "which", "when", "where", "who", "whom", "altogether",
    "overall", "so", "far", "up", "out", "back", "just", "ever", "yet", "still", "far",
    "list", "show", "tell", "give", "find", "count", "number", "amount", "sum", "much",
    "more", "most", "less", "least", "fewer", "fewest", "than", "or", "vs", "versus",
    "compared", "same", "did", "can", "could", "please", "'s", "about", "one", "ones",
    "things", "stuff", "coming", "upcoming", "next", "last", "latest", "first", "earliest",
    "recent", "most recent", "previous", "average", "typical", "typically", "usually",
    "per", "month", "months", "year", "years", "week", "weeks", "city", "cities",
    "country", "countries", "store", "stores", "shop", "shops", "merchant", "merchants",
    "company", "companies", "airline", "airlines", "person", "people", "destination",
    "destinations", "long", "since", "ago", "often", "biggest", "largest", "highest",
    "smallest", "lowest", "longest", "shortest", "priciest", "been", "lately",
    "recently", "whole", "entire", "together", "combined", "been", "yes", "no", "not",
    "into", "out", "around", "during", "across", "within", "over", "them", "they", "he",
    "she", "him", "her", "its", "their", "his", "hers", "your", "yours", "mine", "ours",
    "kind", "type", "sort", "day", "days", "date", "dates", "exactly", "roughly", "about",
    "visit", "visited", "visiting", "ido", "estuve",
    "add", "added", "totals", "subtotal",
    "anywhere", "anything", "something", "somewhere", "ever", "higher", "lower", "bigger",
    "smaller", "cheaper", "pricier", "until", "till", "left", "wait", "own", "used", "use",
    // Spanish
    "cuantos", "cuantas", "cuanto", "cuanta", "veces", "vez", "que", "cual", "cuales",
    "cuando", "donde", "quien", "quienes", "en", "de", "del", "a", "al", "el", "la", "los",
    "las", "un", "una", "unos", "unas", "mi", "mis", "nuestro", "nuestra", "yo", "me", "he",
    "hemos", "ha", "hay", "tuve", "tengo", "tenemos", "hice", "hicimos", "hecho", "fue",
    "fui", "fuimos", "es", "son", "con", "por", "para", "mas", "menos", "total", "todos",
    "todas", "mes", "meses", "ano", "anos", "ciudad", "tienda", "alguna", "algun", "o",
    "y", "se", "lo", "le", "les", "nos", "primer", "primera", "ultimo", "ultima",
    "proximo", "proxima", "promedio", "media", "cada", "hotel", "recibi", "recibido", "tome",
    "tomamos", "tomado", "hice", "hecho",
];

/// Prepositions that introduce an entity, with the direction they imply.
const PREPS: &[(&str, Option<QueryDirection>)] = &[
    ("to", Some(QueryDirection::To)),
    ("into", Some(QueryDirection::To)),
    ("for", None),
    ("from", Some(QueryDirection::From)),
    ("at", None),
    ("in", None),
    ("on", None),
    ("with", None),
    ("by", Some(QueryDirection::From)),
    ("a", Some(QueryDirection::To)),
    ("hacia", Some(QueryDirection::To)),
    ("para", None),
    ("de", Some(QueryDirection::From)),
    ("del", Some(QueryDirection::From)),
    ("desde", Some(QueryDirection::From)),
    ("en", None),
    ("con", None),
];

/// Prepositions read only in a Spanish question ("a" is an article in
/// English).
const SPANISH_PREPS: &[&str] = &["a", "hacia", "para", "de", "del", "desde", "en", "con"];

/// Words that mark a question as Spanish.
const SPANISH_CUES: &[&str] = &[
    "cuantos", "cuantas", "cuanto", "cuanta", "veces", "que", "donde", "cuando", "mis", "mi",
    "gaste", "vole", "pedi", "compre", "tuve", "hice", "hospede", "vuelos", "pedidos", "noches",
    "hoteles", "paquetes", "facturas", "correos", "mensajes", "gastado", "volado", "alguna",
    "cual", "cuales", "el", "la", "los", "las", "este", "esta",
];

fn prep_of(w: &str, spanish: bool) -> Option<Option<QueryDirection>> {
    if !spanish && SPANISH_PREPS.contains(&w) {
        return None;
    }
    PREPS.iter().find(|(p, _)| *p == w).map(|(_, d)| *d)
}

/// Words that end an entity phrase.
fn stops_entity(w: &str) -> bool {
    PREPS.iter().any(|(p, _)| *p == w)
        || matches!(
            w,
            "or" | "vs" | "versus" | "than" | "and" | "o" | "y" | "did" | "do" | "does" | "have"
                | "has" | "was" | "were" | "is" | "are" | "the" | "most" | "more" | "least"
                | "less" | "per" | "each" | "every" | "by" | "compared" | "so" | "que"
                | "mas" | "menos" | "i" | "we" | "me" | "us"
        )
}

/// A parsed question and how the parse went.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Parsed {
    pub query: AskQuery,
    /// Words nothing explained (a non-empty list means the parse was
    /// rejected; kept for tests and the steps).
    pub unread: Vec<String>,
}

fn has(words: &[String], w: &str) -> bool {
    words.iter().any(|x| x == w)
}

fn has_seq(words: &[String], seq: &[&str]) -> bool {
    words
        .windows(seq.len())
        .any(|win| win.iter().zip(seq).all(|(a, b)| a == b))
}

fn is_date_start(w: &str) -> bool {
    matches!(
        w,
        "in" | "during" | "since" | "from" | "before" | "after" | "between" | "this" | "last"
            | "past" | "until" | "till" | "through" | "over" | "for" | "the" | "q1" | "q2"
            | "q3" | "q4" | "first" | "second" | "third" | "fourth" | "next" | "year"
            | "ytd" | "today" | "yesterday" | "of" | "early" | "mid" | "late" | "end"
            | "beginning"
    ) || crate::dates::year_num(w).is_some()
        || crate::dates::month_num(w).is_some()
        || w.contains('-')
        || w.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Date phrases in `words`: (start, end) spans, longest first at each
/// position. A lone month name needs a preposition before it ("in may"),
/// since "may" and "june" are also names.
fn date_spans(words: &[String], today: NaiveDate, tense: QueryTense) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if !is_date_start(&words[i]) {
            i += 1;
            continue;
        }
        let mut found = None;
        for end in (i + 1..=(i + 7).min(words.len())).rev() {
            let text = words[i..end].join(" ");
            let slice: Vec<&str> = words[i..end].iter().map(String::as_str).collect();
            // "the last flight", "first", "next" alone aren't dates.
            if slice.len() == 1
                && matches!(
                    slice[0],
                    "last" | "first" | "next" | "this" | "the" | "in" | "for" | "over" | "of"
                        | "since" | "past" | "second" | "end" | "early" | "late" | "mid"
                        | "year"
                )
            {
                continue;
            }
            if slice.len() == 1 && crate::dates::month_num(slice[0]).is_some() {
                let prev = i.checked_sub(1).map(|p| words[p].as_str()).unwrap_or("");
                if !matches!(prev, "in" | "during" | "since" | "for" | "of" | "en" | "de" | "before" | "after" | "until" | "through") {
                    continue;
                }
            }
            if read_timeframe(&text, today, tense).is_some() {
                found = Some(end);
                break;
            }
        }
        match found {
            Some(end) => {
                out.push((i, end));
                i = end;
            }
            None => i += 1,
        }
    }
    out
}

/// Tense from the verbs.
fn tense_of(words: &[String]) -> QueryTense {
    const PAST: &[&str] = &[
        "did", "was", "were", "had", "flew", "flown", "went", "bought", "ordered", "spent",
        "paid", "got", "received", "took", "stayed", "traveled", "travelled", "been",
        "purchased", "emailed", "wrote", "vole", "volamos", "gaste", "gastamos", "pague",
        "compre", "pedi", "tuve", "fui", "hice", "hospede", "viaje", "volado", "gastado",
        "comprado", "recibi", "delivered", "shipped", "used", "last", "previous", "tome",
        "tomamos", "flown",
    ];
    const FUTURE: &[&str] = &[
        "will", "upcoming", "next", "coming", "planned", "scheduled", "proximo", "proxima",
        "voy", "vamos", "tengo", "pending",
    ];
    let past = words.iter().any(|w| PAST.contains(&w.as_str())) || has_seq(words, &["have", "i"]) || has_seq(words, &["have", "we"]);
    let future = words.iter().any(|w| FUTURE.contains(&w.as_str()))
        || has_seq(words, &["am", "i"])
        || has_seq(words, &["are", "we"])
        || has_seq(words, &["going", "to"])
        // "How many flights do I have in October": what is booked.
        || has_seq(words, &["do", "i", "have"])
        || has_seq(words, &["do", "we", "have"])
        || has_seq(words, &["have", "i", "got"])
        || has_seq(words, &["tengo"]);
    match (past, future) {
        (true, false) => QueryTense::Past,
        (false, true) => QueryTense::Future,
        _ => QueryTense::Any,
    }
}

/// Parse a question into a query, or None when it isn't one the grammar
/// can read completely.
pub(crate) fn parse(question: &str, today: NaiveDate) -> Option<Parsed> {
    let mut words = intent::normalize(question);
    // Leading requests: "tell me", "show me", "can you tell me", "list".
    let mut list_cue = false;
    loop {
        let w0 = words.first().cloned().unwrap_or_default();
        let w1 = words.get(1).cloned().unwrap_or_default();
        let (w0, w1) = (w0.as_str(), w1.as_str());
        let n = match (w0, w1) {
            ("show" | "tell" | "give" | "find" | "get", "me") => 2,
            ("can" | "could", "you") => 2,
            ("i", "want") | ("i", "need") => 2,
            ("to", "know") => 2,
            ("list" | "show" | "find" | "get", _) => {
                list_cue = true;
                1
            }
            ("dime" | "muestrame" | "lista", _) => {
                list_cue = true;
                1
            }
            _ => 0,
        };
        if n == 0 || words.len() <= n {
            break;
        }
        words.drain(..n);
        if matches!(w0, "list" | "show" | "lista" | "muestrame") {
            list_cue = true;
        }
    }
    if words.len() < 2 {
        return None;
    }
    let mut tense = tense_of(&words);
    let spanish = words.iter().any(|w| SPANISH_CUES.contains(&w.as_str()));

    // ---- comparisons: "<A> or <B>", "<A> than <B>", "<A> vs <B>"
    let mut compare: Vec<String> = Vec::new();
    let mut compare_is_time = false;
    let conj = words
        .iter()
        .position(|w| matches!(w.as_str(), "or" | "vs" | "versus" | "than" | "o"))
        .or_else(|| {
            words
                .windows(2)
                .position(|w| w[0] == "compared" && matches!(w[1].as_str(), "to" | "with"))
        });
    let comparative = words.iter().any(|w| {
        matches!(
            w.as_str(),
            "more" | "less" | "fewer" | "mas" | "menos" | "compared" | "vs" | "versus" | "than"
                | "higher" | "lower" | "bigger" | "smaller" | "cheaper" | "pricier"
        )
    });
    if let (Some(k), true) = (conj, comparative) {
        let skip = if words[k] == "compared" { 2 } else { 1 };
        let right_start = k + skip;
        // Timeframes on both sides.
        let left_t = (k.saturating_sub(5)..k).find(|&s| {
            read_timeframe(&words[s..k].join(" "), today, tense).is_some()
                && (s == 0 || !is_date_start(&words[s - 1]) || words[s - 1] == "in" || words[s - 1] == "en")
        });
        let mut right_end = None;
        if right_start < words.len() {
            for e in (right_start + 1..=(right_start + 6).min(words.len())).rev() {
                if read_timeframe(&words[right_start..e].join(" "), today, tense).is_some() {
                    right_end = Some(e);
                    break;
                }
            }
        }
        if let (Some(s), Some(e)) = (left_t, right_end) {
            // Include a preposition before the left side ("in july").
            let s0 = if s > 0 && matches!(words[s - 1].as_str(), "in" | "en" | "during") { s - 1 } else { s };
            let bare = |from: usize, to: usize| {
                let mut f = from;
                if to - f > 1 && matches!(words[f].as_str(), "in" | "en" | "during") {
                    f += 1;
                }
                words[f..to].join(" ")
            };
            compare = vec![bare(s, k), bare(right_start, e)];
            compare_is_time = true;
            words.drain(s0..e);
        } else {
            // Entities: the words after the last preposition before the
            // conjunction, and the words after it.
            let lp = (0..k).rev().find(|&i| prep_of(&words[i], spanish).is_some());
            if let Some(lp) = lp {
                let left: Vec<String> = words[lp + 1..k].to_vec();
                let mut re = right_start;
                // Skip a repeated preposition ("to lisbon or to madrid").
                if re < words.len() && prep_of(&words[re], spanish).is_some() {
                    re += 1;
                }
                let mut e = re;
                while e < words.len() && !stops_entity(&words[e]) && !is_date_start(&words[e]) {
                    e += 1;
                }
                let right: Vec<String> = words[re..e].to_vec();
                if !left.is_empty() && !right.is_empty() && left.len() <= 4 && right.len() <= 4 {
                    compare = vec![left.join(" "), right.join(" ")];
                    // Keep the preposition, drop the sides.
                    words.drain(lp + 1..e);
                }
            }
        }
    }

    // ---- timeframe
    let spans = date_spans(&words, today, tense);
    let mut timeframe: Option<String> = None;
    for (s, e) in spans.iter().rev() {
        let text = words[*s..*e].join(" ");
        timeframe = Some(match timeframe {
            // Two phrases ("in 2025 … in march"): join them; the reader
            // takes the pair as written when it can.
            Some(t) if read_timeframe(&format!("{text} {t}"), today, tense).is_some() => format!("{text} {t}"),
            Some(t) => t,
            None => text,
        });
        let mut s0 = *s;
        while s0 > 0 && matches!(words[s0 - 1].as_str(), "in" | "during" | "for" | "over" | "within" | "en" | "durante") {
            s0 -= 1;
        }
        // "flights booked for October", "reservations for December": a
        // month someone plans for is the coming one.
        if s0 > 0 && words[s0 - 1] == "for" || words[*s] == "for" {
            if words[*s..*e].iter().all(|w| crate::dates::month_num(w).is_some() || w == "for") {
                tense = QueryTense::Future;
            }
        }
        words.drain(s0..*e);
    }
    if timeframe.is_none() {
        // A month named right after its preposition was kept above; a
        // future question about it looks ahead.
        if words.iter().any(|w| matches!(w.as_str(), "next" | "upcoming")) {
            tense = QueryTense::Future;
        }
    }

    // ---- subject
    let mut subject: Option<QuerySubject> = None;
    let mut noun_seen = false;
    let mut money = false;
    let mut nights = false;
    let mut trips = false;
    let mut used = vec![false; words.len()];
    let mut named_plan = false;
    let mut i = 0;
    while i < words.len() {
        let mut matched = 0;
        for len in (1..=3).rev() {
            if i + len > words.len() {
                continue;
            }
            let phrase = words[i..i + len].join(" ");
            // "the reservation code for my flight": a field's name, not the
            // subject.
            let names_field = len == 1
                && matches!(phrase.as_str(), "reservation" | "booking")
                && words
                    .get(i + 1)
                    .is_some_and(|w| matches!(w.as_str(), "code" | "number" | "reference"));
            if names_field {
                matched = 1;
                break;
            }
            if let Some((_, role)) = LEXICON.iter().find(|(w, _)| *w == phrase) {
                match *role {
                    Role::Noun(s) => {
                        // "subscriptions" names a kind of charge the query
                        // can't filter on; only one merchant's ("my Streamly
                        // subscription") reads as a query.
                        if matches!(phrase.as_str(), "subscriptions" | "subscription" | "memberships" | "membership" | "suscripcion") {
                            named_plan = true;
                        }
                        if !noun_seen || subject == Some(QuerySubject::Messages) {
                            subject = Some(s);
                        }
                        noun_seen = true;
                    }
                    Role::Verb(s) => {
                        if !noun_seen && subject.is_none_or(|x| x == QuerySubject::Messages) {
                            subject = Some(s);
                        }
                    }
                    Role::Money => money = true,
                    Role::Nights => nights = true,
                    Role::Trips => {
                        trips = true;
                        if !noun_seen {
                            subject = Some(QuerySubject::Flights);
                        }
                    }
                }
                matched = len;
                break;
            }
        }
        if matched > 0 {
            for u in used.iter_mut().skip(i).take(matched) {
                *u = true;
            }
            i += matched;
        } else {
            i += 1;
        }
    }
    // "Streamly total this year", "sum of my Tunewave payments", "add up my
    // receipts": a total is of money unless it counts ("total number of").
    let sum_cue = words.iter().any(|w| matches!(w.as_str(), "total" | "totals" | "sum" | "subtotal"))
        || has_seq(&words, &["add", "up"])
        || has_seq(&words, &["added", "up"]);
    let counting = has_seq(&words, &["how", "many"]) || has(&words, "number") || has(&words, "count");
    if sum_cue && !counting && subject.is_none() {
        subject = Some(QuerySubject::Spending);
        money = true;
    }
    if nights && subject.is_none() {
        subject = Some(QuerySubject::Stays);
    }
    if money && subject.is_none() {
        subject = Some(QuerySubject::Spending);
    }
    // "email" as a verb about a fact ("how many emails about flights") is
    // a topic count, not this.
    // "When did I last go to London", "the last time I went to Seattle":
    // going somewhere the airport table knows is a trip.
    if subject.is_none() {
        const TRAVEL: &[&str] = &["go", "went", "gone", "been", "visit", "visited", "visiting", "fui", "fuimos", "ido", "estuve"];
        let travel = words.iter().any(|w| TRAVEL.contains(&w.as_str()));
        let place_named = (0..words.len()).any(|i| {
            matches!(words[i].as_str(), "to" | "in" | "a" | "en") && (1..=3).any(|n| {
                i + 1 + n <= words.len() && {
                    let p = words[i + 1..i + 1 + n].join(" ");
                    !airports_for_place(&p).is_empty() || country(&p).is_some()
                }
            })
        });
        if travel && place_named {
            subject = Some(QuerySubject::Flights);
        }
    }
    let subject = subject?;

    // ---- operation
    let first = words.first().map(String::as_str).unwrap_or("");
    let second = words.get(1).map(String::as_str).unwrap_or("");
    let group_word = |w: &str| -> Option<QueryGroup> {
        Some(match w {
            "month" | "mes" => QueryGroup::Month,
            "year" | "ano" => QueryGroup::Year,
            "city" | "cities" | "country" | "countries" | "destination" | "destinations" | "place"
            | "places" | "ciudad" | "pais" | "where" | "donde" => QueryGroup::Place,
            "store" | "stores" | "shop" | "shops" | "merchant" | "merchants" | "company"
            | "companies" | "airline" | "airlines" | "tienda" | "brand" | "vendor" => {
                QueryGroup::Merchant
            }
            "who" | "person" | "people" | "quien" | "sender" | "senders" => QueryGroup::Person,
            _ => return None,
        })
    };
    let most = words.iter().any(|w| matches!(w.as_str(), "most" | "more" | "mas"));
    let least = words
        .iter()
        .any(|w| matches!(w.as_str(), "least" | "fewest" | "less" | "fewer" | "menos"));
    let mut group_by: Option<QueryGroup> = None;
    // "what did I spend the most on", "where did I spend the most".
    if money && (most || least) && matches!(words.last().map(String::as_str), Some("on" | "at" | "with")) {
        group_by = Some(QueryGroup::Merchant);
    }
    // "per month", "each month", "by month", "every month", "monthly".
    for (k, w) in words.iter().enumerate() {
        let prev = k.checked_sub(1).map(|p| words[p].as_str()).unwrap_or("");
        let next = words.get(k + 1).map(String::as_str).unwrap_or("");
        let g = match w.as_str() {
            "monthly" => Some(QueryGroup::Month),
            "yearly" | "annually" => Some(QueryGroup::Year),
            _ if matches!(prev, "per" | "each" | "by" | "every" | "a" | "cada" | "por" | "al") => {
                group_word(w).filter(|g| *g != QueryGroup::Person || prev == "by")
            }
            // "which month", "what year", "which city", "en que mes".
            _ if matches!(prev, "which" | "what" | "que" | "cual" | "cuales") => group_word(w),
            // "where do I fly the most", "who emails me most"; for money,
            // "where do I spend the most" is the store.
            "where" | "donde" if (most || least) && (money || matches!(subject, QuerySubject::Orders | QuerySubject::Bills | QuerySubject::Spending)) => Some(QueryGroup::Merchant),
            "where" | "donde" if most || least => Some(QueryGroup::Place),
            "who" | "quien" if subject == QuerySubject::Messages && (most || least) => Some(QueryGroup::Person),
            "where" if k == 0 && matches!(subject, QuerySubject::Flights | QuerySubject::Stays) && timeframe.is_some() && !matches!(next, "is" | "am" | "are") => {
                Some(QueryGroup::Place)
            }
            _ => None,
        };
        if g.is_some() && group_by.is_none() {
            group_by = g;
            break;
        }
    }
    let starts_yes_no = matches!(first, "did" | "have" | "has" | "do" | "does" | "was" | "were" | "is" | "are" | "am")
        && matches!(second, "i" | "we" | "you" | "there" | "my" | "any")
        || (first == "alguna" && second == "vez")
        || matches!(first, "he" | "hemos" | "hay");
    let how_many = has_seq(&words, &["how", "many"])
        || has_seq(&words, &["how", "often"])
        || words.windows(2).enumerate().any(|(i, w)| w[0] == "number" && w[1] == "of" && (i == 0 || !matches!(words[i - 1].as_str(), "order" | "flight" | "tracking" | "confirmation" | "booking" | "reservation" | "phone" | "account" | "invoice")))
        || matches!(first, "cuantos" | "cuantas" | "count");
    // A total of receipts, bills or payments is money ("total cost of my
    // Streamly receipts", "sum of my Nimbus invoices"); "what did I spend on
    // X" asks the same as "how much".
    let spend_verb = words.iter().any(|w| matches!(w.as_str(), "spend" | "spent" | "pay" | "paid" | "gaste" | "gastado" | "pague" | "pagado"));
    let what_spent = matches!(first, "what" | "que") && spend_verb && !(most || least) && group_by.is_none();
    let how_much = has_seq(&words, &["how", "much"])
        || matches!(first, "cuanto" | "cuanta")
        || has(&words, "total") && money
        || what_spent
        || sum_cue && !counting && (money || matches!(subject, QuerySubject::Orders | QuerySubject::Bills | QuerySubject::Spending));
    let average = words.iter().any(|w| matches!(w.as_str(), "average" | "avg" | "typical" | "typically" | "usually" | "promedio" | "media" | "mean"));
    let longest = words.iter().any(|w| matches!(w.as_str(), "longest" | "shortest"));
    if longest && subject == QuerySubject::Stays {
        nights = true;
    }
    let max_cue = words.iter().any(|w| {
        matches!(
            w.as_str(),
            "biggest" | "largest" | "highest" | "priciest" | "longest" | "expensive" | "caro" | "cara" | "mayor"
        )
    }) || (most && group_by.is_none() && !how_many && !how_much && compare.is_empty());
    let min_cue = words.iter().any(|w| {
        matches!(
            w.as_str(),
            "cheapest" | "smallest" | "lowest" | "shortest" | "barato" | "barata" | "menor"
        )
    }) || has_seq(&words, &["least", "expensive"]);
    let first_cue = words.iter().any(|w| matches!(w.as_str(), "first" | "earliest" | "primer" | "primera"));
    // Lookups: "the confirmation code for my flight to Lisbon", "tracking
    // number for my package", "how much was my last Swiftcab ride".
    let field = if has_seq(&words, &["tracking", "number"]) || has(&words, "tracking") {
        Some(QueryField::Tracking)
    } else if has_seq(&words, &["flight", "number"]) {
        Some(QueryField::FlightNumber)
    } else if has_seq(&words, &["order", "number"]) || has_seq(&words, &["order", "#"]) {
        Some(QueryField::OrderNumber)
    } else if ["confirmation", "reservation code", "reservation number", "booking reference", "booking code", "booking number", "record locator", "localizador"]
        .iter()
        .any(|f| has_seq(&words, &f.split(' ').collect::<Vec<_>>()))
    {
        Some(QueryField::Confirmation)
    } else {
        None
    };
    let last_cue = words.iter().any(|w| matches!(w.as_str(), "last" | "latest" | "previous" | "ultimo" | "ultima"))
        || has_seq(&words, &["most", "recent"]);
    let next_cue = words.iter().any(|w| matches!(w.as_str(), "next" | "proximo" | "proxima"));
    let until = has_seq(&words, &["how", "long", "until"])
        || has_seq(&words, &["how", "long", "till"])
        || has_seq(&words, &["how", "much", "time", "until"])
        || has_seq(&words, &["how", "many", "days", "until"]);
    let when = until
        || matches!(first, "when" | "cuando")
        || has_seq(&words, &["how", "long", "since"])
        || has_seq(&words, &["how", "long", "ago"])
        || has_seq(&words, &["what", "date"]);
    let how_many = how_many && !until;
    let how_much = how_much && !until;
    let plural_noun = words.iter().enumerate().any(|(k, w)| {
        used.get(k).copied().unwrap_or(false) && w.ends_with('s') && !matches!(w.as_str(), "was" | "is")
    });
    // "Where am I staying", "where's my order": one thing, the templates'
    // job; "where did I fly in 2025" is grouped above.
    let which_what = matches!(first, "what" | "which" | "que" | "cuales");

    // "How much is my Streamly subscription", "how much was the Nimbus
    // bill": one charge, the latest, unless dates or "per month" ask for more.
    let how_much_is = (has_seq(&words, &["how", "much", "is"]) || has_seq(&words, &["how", "much", "was"]))
        && timeframe.is_none()
        && group_by.is_none()
        && !plural_noun
        && !sum_cue;
    let op = if !compare.is_empty() {
        if money || how_much {
            QueryOp::Sum
        } else {
            QueryOp::Count
        }
    } else if average {
        QueryOp::Average
    } else if group_by.is_some() && (most || least || max_cue || min_cue) {
        if least || min_cue {
            QueryOp::Min
        } else {
            QueryOp::Max
        }
    } else if group_by.is_some() && (how_many || how_much) {
        if how_much || money {
            QueryOp::Sum
        } else {
            QueryOp::Count
        }
    } else if how_many {
        QueryOp::Count
    } else if how_much && (first_cue || last_cue || next_cue || how_much_is) {
        // "how much was my last Swiftcab ride": that one's amount.
        if first_cue {
            QueryOp::First
        } else if next_cue {
            QueryOp::Next
        } else {
            QueryOp::Last
        }
    } else if field.is_some() {
        let event = matches!(subject, QuerySubject::Flights | QuerySubject::Stays | QuerySubject::Bookings);
        if first_cue {
            QueryOp::First
        } else if last_cue || (tense == QueryTense::Past && !next_cue) || !event {
            QueryOp::Last
        } else {
            QueryOp::Next
        }
    } else if how_much {
        QueryOp::Sum
    } else if min_cue {
        QueryOp::Min
    } else if max_cue {
        QueryOp::Max
    } else if when || ((first_cue || last_cue || next_cue) && !plural_noun) {
        if first_cue {
            QueryOp::First
        } else if next_cue || (tense == QueryTense::Future && !last_cue) {
            QueryOp::Next
        } else if last_cue || tense == QueryTense::Past {
            QueryOp::Last
        } else {
            QueryOp::Next
        }
    } else if starts_yes_no {
        QueryOp::Exists
    } else if group_by.is_some() || list_cue || which_what || plural_noun {
        QueryOp::List
    } else if money {
        // "my Streamly subscription cost this year": what it came to.
        QueryOp::Sum
    } else {
        return None;
    };
    // "How much do I pay for Streamly a month": what it usually costs, the
    // average over the last twelve whole months (shown, and editable, as the
    // timeframe). "How much did I spend per month in 2025" lists the months.
    let op = if op == QueryOp::Sum
        && group_by == Some(QueryGroup::Month)
        && how_much
        && timeframe.is_none()
        && compare.is_empty()
        && tense != QueryTense::Past
        && !has(&words, "did")
    {
        timeframe = Some(HABITUAL_MONTHS.into());
        QueryOp::Average
    } else {
        op
    };
    // A plain "when" question about travel ("when do I fly to Lisbon") and
    // lookups are the templates' job; this layer adds what they lack.
    let measure = if nights && subject == QuerySubject::Stays {
        QueryMeasure::Nights
    } else if trips && subject == QuerySubject::Flights && matches!(op, QueryOp::Count | QueryOp::Max | QueryOp::Min) && group_by.is_none() && !money {
        QueryMeasure::Trips
    } else if money
        || (how_much && !matches!(subject, QuerySubject::Parcels | QuerySubject::Messages))
        || matches!(op, QueryOp::Max | QueryOp::Min | QueryOp::Average) && group_by.is_none() && !matches!(subject, QuerySubject::Parcels | QuerySubject::Messages | QuerySubject::Flights) && !how_many
    {
        QueryMeasure::Money
    } else {
        QueryMeasure::Items
    };
    if matches!(subject, QuerySubject::Parcels | QuerySubject::Messages) && measure == QueryMeasure::Money {
        return None;
    }
    // Max/min over messages or flights without a group needs a measure.
    if matches!(op, QueryOp::Max | QueryOp::Min) && group_by.is_none() && measure == QueryMeasure::Items {
        if subject == QuerySubject::Messages {
            return None;
        }
        // "my longest trip" = most nights; flights have no size otherwise.
        return None;
    }
    let op = if op == QueryOp::Sum && measure == QueryMeasure::Items {
        QueryOp::Count
    } else {
        op
    };

    // ---- entities: what follows a preposition
    let mut place: Option<String> = None;
    let mut merchant: Option<String> = None;
    let mut person: Option<String> = None;
    let mut direction: Option<QueryDirection> = None;
    let mut unread: Vec<String> = Vec::new();
    let mut k = 0;
    while k < words.len() {
        if used[k] {
            k += 1;
            continue;
        }
        let w = words[k].as_str();
        let prep = prep_of(w, spanish);
        if let Some(dir) = prep {
            let dir = &dir;
            // Leading articles: "at the gearloft store".
            let mut s = k + 1;
            while s < words.len() && !used[s] && matches!(words[s].as_str(), "the" | "my" | "our" | "el" | "la" | "los" | "las" | "mi" | "mis") {
                s += 1;
            }
            let mut e = s;
            while e < words.len()
                && !used[e]
                && !stops_entity(&words[e])
                && !GLUE_SKIP.contains(&words[e].as_str())
                && (!GLUE.contains(&words[e].as_str()) || words[e] == "city")
            {
                e += 1;
            }
            let mut phrase: Vec<&str> = words[s..e].iter().map(String::as_str).collect();
            // "at gearloft store" → gearloft.
            while phrase.len() > 1 && matches!(*phrase.last().unwrap_or(&""), "store" | "shop" | "restaurant" | "tienda" | "hotel" | "hotels") {
                phrase.pop();
            }
            if phrase.is_empty() {
                k += 1;
                continue;
            }
            let text = phrase.join(" ");
            let is_place = !airports_for_place(&text).is_empty() || country(&text).is_some();
            match subject {
                QuerySubject::Messages => {
                    if person.is_none() {
                        person = Some(text);
                        direction = match (w, dir) {
                            ("with" | "con", _) => None,
                            (_, d) => *d,
                        };
                    } else {
                        unread.push(text);
                    }
                }
                QuerySubject::Flights | QuerySubject::Stays if is_place => {
                    if place.is_none() {
                        place = Some(text);
                        if subject == QuerySubject::Flights {
                            direction = *dir;
                        }
                    } else {
                        unread.push(text);
                    }
                }
                _ if is_place && matches!(subject, QuerySubject::Spending | QuerySubject::Bookings) => {
                    place = Some(text);
                }
                _ => {
                    if merchant.is_none() {
                        merchant = Some(text);
                    } else {
                        unread.push(text);
                    }
                }
            }
            for u in used.iter_mut().take(e).skip(k) {
                *u = true;
            }
            k = e;
            continue;
        }
        k += 1;
    }
    // Compared names are places or merchants, checked like the others.
    if !compare.is_empty() && !compare_is_time {
        let places = compare
            .iter()
            .all(|c| !airports_for_place(c).is_empty() || country(c).is_some());
        if subject == QuerySubject::Messages || (!places && matches!(subject, QuerySubject::Flights | QuerySubject::Stays)) {
            if subject != QuerySubject::Messages {
                return None;
            }
        }
    }
    for (k, w) in words.iter().enumerate() {
        let field_word = field.is_some() && matches!(w.as_str(), "confirmation" | "code" | "number" | "tracking" | "reference" | "locator" | "record" | "reservation" | "booking" | "localizador" | "#");
        if used[k] || field_word || GLUE.contains(&w.as_str()) || GLUE_SKIP.contains(&w.as_str()) {
            continue;
        }
        // "sofia" in "how many emails did sofia send me": the person.
        if subject == QuerySubject::Messages && person.is_none() && w.chars().all(char::is_alphabetic) && w.len() > 1 {
            person = Some(w.clone());
            // "emails I sent Priya" are to her; "emails Priya sent" from her.
            let i_sent = words.windows(2).any(|p| {
                matches!(p[0].as_str(), "i" | "we")
                    && matches!(p[1].as_str(), "sent" | "send" | "wrote" | "write" | "emailed" | "email" | "messaged")
            });
            direction = Some(if i_sent { QueryDirection::To } else { QueryDirection::From });
            continue;
        }
        unread.push(w.clone());
    }
    // A name with no preposition ("my gearloft orders", "the average
    // swiftcab ride"): one run of words, as the merchant or place.
    if !unread.is_empty() && subject != QuerySubject::Messages && unread.len() <= 3 {
        let run: Vec<usize> = words
            .iter()
            .enumerate()
            .filter(|(k, w)| unread.contains(w) && !used[*k])
            .map(|(k, _)| k)
            .collect();
        let contiguous = run.windows(2).all(|p| p[1] == p[0] + 1) && run.len() == unread.len();
        if contiguous {
            let text = unread.join(" ");
            let is_place = !airports_for_place(&text).is_empty() || country(&text).is_some();
            if is_place && place.is_none() && subject != QuerySubject::Orders && subject != QuerySubject::Bills {
                place = Some(text);
                unread.clear();
            } else if !is_place && merchant.is_none() {
                merchant = Some(text);
                unread.clear();
            }
        }
    }
    let field = if matches!(op, QueryOp::First | QueryOp::Last | QueryOp::Next) {
        if how_much {
            Some(QueryField::Amount)
        } else {
            field
        }
    } else {
        None
    };
    if field.is_some() {
        unread.retain(|w| !matches!(w.as_str(), "confirmation" | "code" | "number" | "tracking" | "reference" | "locator" | "record" | "reservation" | "booking" | "localizador" | "#"));
    }
    let mut query = AskQuery::new(subject, op);
    query.measure = measure;
    query.group_by = group_by;
    query.timeframe = timeframe;
    query.compare = compare;
    query.place = place;
    query.merchant = merchant;
    query.person = person;
    query.direction = direction;
    query.tense = tense;
    query.field = field;
    if subject == QuerySubject::Messages
        && matches!(query.person.as_deref(), Some("me" | "us" | "i" | "we" | "you"))
    {
        query.person = None;
    }
    // "how many emails did I send" / "emails I wrote": to others.
    if subject == QuerySubject::Messages && query.direction.is_none() {
        if has(&words, "send") || has(&words, "sent") || (has(&words, "i") && (has(&words, "wrote") || has(&words, "emailed"))) {
            query.direction = Some(QueryDirection::To);
        } else if has(&words, "get") || has(&words, "got") || has(&words, "receive") || has(&words, "received") || has_seq(&words, &["emails", "me"]) || has_seq(&words, &["emailed", "me"]) || has_seq(&words, &["email", "me"]) {
            query.direction = Some(QueryDirection::From);
        }
    }
    if check(&query, today).is_err() || (named_plan && query.merchant.is_none()) {
        return None;
    }
    Some(Parsed { query, unread })
}

/// The timeframe a habitual "a month" question is averaged over.
const HABITUAL_MONTHS: &str = "the last 12 full months";

/// Words skipped inside and around entities: verbs and fillers that don't
/// name anything.
const GLUE_SKIP: &[&str] = &[
    "send", "sent", "sends", "receive", "received", "get", "got", "spend", "spent", "pay",
    "paid", "fly", "flew", "flown", "order", "ordered", "stay", "stayed", "buy", "bought",
    "travel", "traveled", "go", "went", "emails", "email", "emailed", "me", "us", "the",
    "most", "total", "overall", "altogether", "so", "far", "this", "past",
];

/// Countries by name (English and Spanish) → airport codes in the table.
pub(crate) fn country(name: &str) -> Option<&'static [&'static str]> {
    const COUNTRIES: &[(&[&str], &[&str])] = &[
        (&["portugal"], &["LIS", "OPO", "FAO"]),
        (&["spain", "espana"], &["MAD", "BCN", "VLC", "AGP", "SVQ", "PMI", "BIO"]),
        (&["japan", "japon"], &["HND", "NRT", "KIX"]),
        (&["mexico"], &["MEX", "CUN", "GDL", "MTY", "SJD", "PVR"]),
        (&["canada"], &["YYZ", "YVR", "YUL", "YYC"]),
        (&["uk", "united kingdom", "england", "reino unido", "inglaterra"], &["LHR", "LGW", "STN", "MAN", "EDI"]),
        (&["france", "francia"], &["CDG", "ORY", "NCE"]),
        (&["italy", "italia"], &["FCO", "MXP", "VCE"]),
        (&["germany", "alemania"], &["FRA", "MUC", "BER"]),
        (&["netherlands", "holland", "holanda"], &["AMS"]),
        (&["ireland", "irlanda"], &["DUB"]),
        (&["usa", "us", "united states", "estados unidos", "america"], &[
            "SFO", "LAX", "JFK", "EWR", "LGA", "ORD", "DEN", "SEA", "AUS", "BOS", "ATL", "DFW",
            "MIA", "IAH", "PHX", "LAS", "SAN", "PDX", "MSP", "DTW", "PHL", "CLT", "IAD", "DCA",
            "SLC", "HNL", "MCO", "OAK", "SJC", "BNA", "MSY", "RDU",
        ]),
    ];
    let n = crate::text::tokens(name)
        .into_iter()
        .map(|t| t.2)
        .collect::<Vec<_>>()
        .join(" ");
    COUNTRIES
        .iter()
        .find(|(names, _)| names.contains(&n.as_str()))
        .map(|(_, codes)| *codes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 27).unwrap()
    }

    fn q(s: &str) -> AskQuery {
        match parse(s, today()) {
            Some(p) if p.unread.is_empty() => p.query,
            other => panic!("{s:?} → {other:?}"),
        }
    }

    fn none(s: &str) {
        if let Some(p) = parse(s, today()) {
            assert!(!p.unread.is_empty(), "{s:?} should not parse: {:?}", p.query);
        }
    }

    #[test]
    fn counts_with_time() {
        let x = q("how many times did i fly in august");
        assert_eq!((x.subject, x.op), (QuerySubject::Flights, QueryOp::Count));
        assert_eq!(x.timeframe.as_deref(), Some("in august"));
        let x = q("how many flights did I take in 2025?");
        assert_eq!((x.subject, x.op, x.timeframe.as_deref()), (QuerySubject::Flights, QueryOp::Count, Some("in 2025")));
        let x = q("¿Cuántas veces volé en agosto?");
        assert_eq!((x.subject, x.op), (QuerySubject::Flights, QueryOp::Count));
        assert!(x.timeframe.is_some());
        let x = q("how many nights did I stay in hotels this year");
        assert_eq!((x.subject, x.op, x.measure), (QuerySubject::Stays, QueryOp::Count, QueryMeasure::Nights));
        let x = q("how many trips did I take last year");
        assert_eq!(x.measure, QueryMeasure::Trips);
    }

    #[test]
    fn places_and_merchants() {
        let x = q("how many times have I flown to Lisbon");
        assert_eq!(x.place.as_deref(), Some("lisbon"));
        assert_eq!(x.direction, Some(QueryDirection::To));
        let x = q("how many orders did I place with Gearloft in 2025");
        assert_eq!((x.subject, x.merchant.as_deref()), (QuerySubject::Orders, Some("gearloft")));
        let x = q("how much did I spend in august");
        assert_eq!((x.subject, x.op, x.measure), (QuerySubject::Spending, QueryOp::Sum, QueryMeasure::Money));
    }

    #[test]
    fn groups_and_extremes() {
        let x = q("which month did I spend the most");
        assert_eq!((x.subject, x.op, x.group_by), (QuerySubject::Spending, QueryOp::Max, Some(QueryGroup::Month)));
        let x = q("where do I fly the most");
        assert_eq!((x.op, x.group_by), (QueryOp::Max, Some(QueryGroup::Place)));
        let x = q("what was my most expensive order");
        assert_eq!((x.subject, x.op, x.measure), (QuerySubject::Orders, QueryOp::Max, QueryMeasure::Money));
        // Habitual: what it usually costs, over the last twelve whole months.
        let x = q("how much do I spend per month on dishdash");
        assert_eq!((x.op, x.group_by, x.merchant.as_deref()), (QueryOp::Average, Some(QueryGroup::Month), Some("dishdash")));
        assert_eq!(x.timeframe.as_deref(), Some(HABITUAL_MONTHS));
        // Past: the months, one by one.
        let x = q("how much did I spend per month on dishdash this year");
        assert_eq!((x.op, x.group_by), (QueryOp::Sum, Some(QueryGroup::Month)));
        let x = q("what's the average swiftcab ride cost");
        assert_eq!(x.op, QueryOp::Average);
    }

    #[test]
    fn totals_by_merchant() {
        // A merchant as a noun modifier, with any word for the receipts.
        for (text, subject) in [
            ("total cost of my linear receipts this year", QuerySubject::Orders),
            ("sum of my Linear invoices", QuerySubject::Bills),
            ("sum of my Linear payments in 2025", QuerySubject::Spending),
            ("total of my linear charges this year", QuerySubject::Spending),
            ("Linear total this year", QuerySubject::Spending),
            ("add up my linear receipts", QuerySubject::Orders),
            ("how much have I paid Linear in 2026", QuerySubject::Spending),
            ("what did I spend on Linear last month", QuerySubject::Spending),
            ("my linear subscription cost this year", QuerySubject::Spending),
        ] {
            let x = q(text);
            assert_eq!((x.subject, x.op, x.measure), (subject, QueryOp::Sum, QueryMeasure::Money), "{text}");
            assert_eq!(x.merchant.as_deref(), Some("linear"), "{text}");
        }
        let x = q("how much is my Linear subscription");
        assert_eq!((x.op, x.field, x.merchant.as_deref()), (QueryOp::Last, Some(QueryField::Amount), Some("linear")));
        let x = q("how much do I pay for Linear a month");
        assert_eq!((x.op, x.group_by, x.timeframe.as_deref()), (QueryOp::Average, Some(QueryGroup::Month), Some(HABITUAL_MONTHS)));
        // Counting stays counting.
        let x = q("total number of flights this year");
        assert_eq!((x.subject, x.op), (QuerySubject::Flights, QueryOp::Count));
        // Subscriptions in general are the subscriptions answer's, not a
        // filter this layer has.
        assert!(parse("how much did I spend on subscriptions this year", today()).is_none());
        let x = q("first email I sent priya");
        assert_eq!((x.person.as_deref(), x.direction), (Some("priya"), Some(QueryDirection::To)));
    }

    #[test]
    fn comparisons() {
        let x = q("did I spend more in july or august");
        assert_eq!(x.compare, vec!["july".to_string(), "august".to_string()]);
        assert_eq!(x.op, QueryOp::Sum);
        let x = q("did I fly more to lisbon or madrid");
        assert_eq!(x.compare, vec!["lisbon".to_string(), "madrid".to_string()]);
    }

    #[test]
    fn existence_and_time() {
        let x = q("did I ever fly to Tokyo");
        assert_eq!((x.op, x.place.as_deref()), (QueryOp::Exists, Some("tokyo")));
        let x = q("when was my first order from pagebound");
        assert_eq!((x.op, x.merchant.as_deref()), (QueryOp::First, Some("pagebound")));
        let x = q("list all my flights in 2025");
        assert_eq!(x.op, QueryOp::List);
    }

    #[test]
    fn lookups() {
        let x = q("what's the order number of my last Pagebound order");
        assert_eq!((x.subject, x.op, x.field), (QuerySubject::Orders, QueryOp::Last, Some(QueryField::OrderNumber)));
        assert_eq!(x.merchant.as_deref(), Some("pagebound"));
        let x = q("what's the tracking number for my package?");
        assert_eq!((x.subject, x.op, x.field), (QuerySubject::Parcels, QueryOp::Last, Some(QueryField::Tracking)));
        let x = q("what's the reservation code for my Skyward Air flight to Lisbon");
        assert_eq!((x.subject, x.op, x.field), (QuerySubject::Flights, QueryOp::Next, Some(QueryField::Confirmation)));
        assert_eq!(x.merchant.as_deref(), Some("skyward air"));
        let x = q("how much was my last Swiftcab ride");
        assert_eq!((x.op, x.field), (QueryOp::Last, Some(QueryField::Amount)));
    }

    #[test]
    fn travel_verbs() {
        let x = q("when did I last go to London");
        assert_eq!((x.subject, x.op, x.place.as_deref()), (QuerySubject::Flights, QueryOp::Last, Some("london")));
        let x = q("when's the last time I went to Seattle");
        assert_eq!((x.subject, x.op), (QuerySubject::Flights, QueryOp::Last));
        assert!(parse("when did I go to the dentist", today()).is_none_or(|p| !p.unread.is_empty()));
    }

    #[test]
    fn not_queries() {
        none("what is the wifi password");
        none("when is ben's wedding");
        none("what did priya say about pricing");
        assert!(parse("why is the sky blue", today()).is_none());
    }
}
