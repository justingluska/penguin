//! Question understanding for Ask, without a model: normalize the words,
//! peel off a date scope ("this year", `date:2025`), then match a small
//! table of templates. A template is a word pattern:
//!
//! - `word` matches that word;
//! - `(a|b c|)` matches one alternative (an empty one makes it optional);
//! - `<name:a|b c>` is the same, and records which alternative matched;
//! - `{name}` captures one or more words (shortest first).
//!
//! `%CONTACT%`-style macros expand to shared word lists before parsing.
//! Groups don't nest and can't hold a slot (write two templates instead);
//! within a group the longest alternative is tried first. First matching
//! template wins, so specific templates come before general ones.

use std::sync::OnceLock;

use chrono::NaiveDate;

use crate::dates::{self, DateRange};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dir {
    /// Either way.
    Any,
    /// They wrote to you.
    FromThem,
    /// You wrote to them.
    ToThem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Span,
    End,
}

/// What a first-contact question asks for: the first email itself, when a
/// working relationship began ("when did I hire Julia": the mail can't say,
/// so the answer is the earliest email and says so), or how long you've
/// known someone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum First {
    Email,
    Start,
    Known,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Any,
    Invoice,
    Receipt,
    Contract,
    Pdf,
    Attachment,
    Spreadsheet,
    Image,
    Doc,
}

/// Which occurrence a travel question means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Which {
    /// The next one ("when do I fly", "my next flight").
    Next,
    /// The latest past one ("when did I fly to Lisbon").
    Last,
    /// Upcoming if there is one, else the latest.
    Any,
}

/// Kinds of extracted facts a question can count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FactKind {
    Flight,
    Stay,
    Order,
    Shipment,
    Bill,
    Booking,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Intent {
    LastContact {
        who: String,
        dir: Dir,
    },
    FirstContact {
        who: String,
        dir: Dir,
        first: First,
    },
    Relationship {
        who: String,
        focus: Focus,
    },
    LatestItem {
        who: Option<String>,
        kind: Kind,
    },
    Count {
        who: String,
        dir: Dir,
        kind: Kind,
    },
    CountTopic {
        topic: String,
    },
    Spend {
        merchant: String,
    },
    WhoAbout {
        topic: String,
    },
    TopSenders,
    WhoIs {
        who: String,
    },
    WaitingOn {
        who: Option<String>,
    },
    OweReplies {
        who: Option<String>,
    },
    When {
        topic: String,
    },
    /// `field`: "confirmation", "number" or "time" when the question asks
    /// for one thing.
    Flight {
        place: Option<String>,
        which: Which,
        field: Option<&'static str>,
    },
    Stay {
        place: Option<String>,
        which: Which,
    },
    Package {
        what: Option<String>,
    },
    Orders {
        merchant: Option<String>,
    },
    Bills {
        what: Option<String>,
    },
    Booking {
        what: Option<String>,
    },
    Code {
        service: Option<String>,
    },
    /// Recurring charges ("what subscriptions do I pay for").
    Subscriptions,
    /// `field`: "email", "phone", "address", or "contact" (all of them).
    ContactInfo {
        who: String,
        field: &'static str,
    },
    Said {
        who: Option<String>,
        topic: String,
    },
    DidReply {
        who: String,
        topic: String,
    },
    Find {
        who: Option<String>,
        topic: String,
    },
    Passage {
        topic: String,
    },
    CountFacts {
        kind: FactKind,
        who: Option<String>,
    },
    Unknown,
}

/// A parsed question.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Question {
    pub intent: Intent,
    /// Local-date range the question is limited to.
    pub range: Option<DateRange>,
    /// How the range was written ("this year", "2025"), for headlines.
    pub range_text: Option<String>,
    /// The normalized words the templates saw (without the date scope).
    pub text: String,
}

// ----------------------------------------------------------------- words

const CONTRACTIONS: &[(&str, &str)] = &[
    ("what's", "what is"),
    ("whats", "what is"),
    ("who's", "who is"),
    ("whos", "who is"),
    ("when's", "when is"),
    ("where's", "where is"),
    ("how's", "how is"),
    ("i've", "i have"),
    ("we've", "we have"),
    ("i'm", "i am"),
    ("didn't", "did not"),
    ("hasn't", "has not"),
    ("haven't", "have not"),
    ("hadn't", "had not"),
    ("isn't", "is not"),
    ("wasn't", "was not"),
    ("don't", "do not"),
    ("doesn't", "does not"),
    ("i'd", "i would"),
    ("we're", "we are"),
    ("they're", "they are"),
    ("let's", "let us"),
    // Typed without the apostrophe, as people do in a search box. Only
    // forms that aren't also ordinary words ("lets", "were", "its" stay).
    ("whens", "when is"),
    ("wheres", "where is"),
    ("hows", "how is"),
    ("ive", "i have"),
    ("im", "i am"),
    ("didnt", "did not"),
    ("hasnt", "has not"),
    ("havent", "have not"),
    ("hadnt", "had not"),
    ("isnt", "is not"),
    ("wasnt", "was not"),
    ("dont", "do not"),
    ("doesnt", "does not"),
    ("theyre", "they are"),
];

/// Words and phrases that carry no meaning for the templates.
const FILLER: &[&[&str]] = &[
    &["from", "when", "to", "now"],
    &["from", "then", "to", "now"],
    &["from", "start", "to", "finish"],
    &["up", "to", "now"],
    &["until", "now"],
    &["till", "now"],
    &["to", "date"],
    &["so", "far"],
    &["can", "you", "tell", "me"],
    &["do", "you", "know"],
    &["i", "wonder"],
    &["please"],
    &["por", "favor"],
    &["exactly"],
    &["roughly"],
    &["approximately"],
    &["ever"],
    &["actually"],
    &["again"],
];
const LEAD_FILLER: &[&str] = &[
    "hey", "ok", "okay", "so", "um", "penguin", "and", "also", "quick", "question", "oye", "y",
    "hola",
];

/// Lowercase, expand contractions, split possessives, drop punctuation and
/// filler. Emails and operators (`date:2025`) survive intact.
pub(crate) fn normalize(q: &str) -> Vec<String> {
    let q = q
        .trim()
        .trim_start_matches(['?', '/', '>', '\u{bf}', '\u{a1}'])
        .replace(['\u{2019}', '\u{2018}', '`'], "'")
        .replace(['\u{bf}', '\u{a1}'], " ")
        .to_lowercase();
    // Accents fold ("cuándo" = "cuando"); Spanish templates are written
    // without them. Names fold the same way people are indexed.
    let q: String = q.chars().map(crate::text::fold_char).collect();
    let mut words: Vec<String> = Vec::new();
    let raws: Vec<&str> = q.split_whitespace().collect();
    let mut r = 0;
    while r < raws.len() {
        let raw = raws[r];
        r += 1;
        // date:"last spring" → one word "date:last spring".
        if let Some(op) = ["date:", "after:", "before:"]
            .into_iter()
            .find(|op| raw.starts_with(op))
        {
            let mut value = raw[op.len()..].to_string();
            if value.starts_with('"') {
                while !(value.len() > 1 && value.ends_with('"')) && r < raws.len() {
                    value.push(' ');
                    value.push_str(raws[r]);
                    r += 1;
                }
            }
            let value = value.trim_matches(|c: char| matches!(c, '"' | '?' | '!' | ',' | '.'));
            words.push(format!("{op}{value}"));
            continue;
        }
        // Keep "a/b" alternatives as three words; dates like 9/24 stay one.
        let parts: Vec<&str> =
            if raw.contains('/') && raw.chars().any(|c| c.is_alphabetic()) && !raw.contains(':') {
                let mut v = Vec::new();
                for (i, p) in raw.split('/').enumerate() {
                    if i > 0 {
                        v.push("/");
                    }
                    v.push(p);
                }
                v
            } else {
                vec![raw]
            };
        for p in parts {
            let mut w = p
                .trim_matches(|c: char| {
                    matches!(c, '?' | '!' | ',' | ';' | '"' | '(' | ')' | '[' | ']')
                })
                .to_string();
            while w.ends_with('.')
                || (w.ends_with(':') && !matches!(w.as_str(), "date:" | "after:" | "before:"))
            {
                w.pop();
            }
            if w.is_empty() {
                continue;
            }
            if let Some((_, full)) = CONTRACTIONS.iter().find(|(c, _)| *c == w) {
                words.extend(full.split(' ').map(String::from));
                continue;
            }
            if let Some(base) = w
                .strip_suffix("'s")
                .or_else(|| w.strip_suffix("s'"))
                .filter(|b| !b.is_empty() && !b.contains('@'))
            {
                words.push(base.to_string());
                words.push("'s".into());
                continue;
            }
            if w == "&" {
                words.push("and".into());
                continue;
            }
            words.push(w);
        }
    }
    // Filler phrases anywhere, then leading interjections.
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut i = 0;
    'outer: while i < words.len() {
        for f in FILLER {
            if words.len() - i >= f.len() && f.iter().zip(&words[i..]).all(|(a, b)| a == b) {
                i += f.len();
                continue 'outer;
            }
        }
        out.push(words[i].clone());
        i += 1;
    }
    while out
        .first()
        .is_some_and(|w| LEAD_FILLER.contains(&w.as_str()))
        && out.len() > 1
    {
        out.remove(0);
    }
    spanish_dates(&mut out);
    out
}

/// Spanish date phrases → the English ones `dates::parse` reads
/// ("el año pasado" → "last year", "en marzo" → "in march").
fn spanish_dates(words: &mut Vec<String>) {
    const PHRASES: &[(&[&str], &[&str])] = &[
        (&["el", "ano", "pasado"], &["last", "year"]),
        (&["este", "ano"], &["this", "year"]),
        (&["el", "mes", "pasado"], &["last", "month"]),
        (&["este", "mes"], &["this", "month"]),
        (&["la", "semana", "pasada"], &["last", "week"]),
        (&["esta", "semana"], &["this", "week"]),
        (&["hoy"], &["today"]),
        (&["ayer"], &["yesterday"]),
        (&["desde"], &["since"]),
    ];
    const MONTHS: &[(&str, &str)] = &[
        ("enero", "january"),
        ("febrero", "february"),
        ("marzo", "march"),
        ("abril", "april"),
        ("mayo", "may"),
        ("junio", "june"),
        ("julio", "july"),
        ("agosto", "august"),
        ("septiembre", "september"),
        ("setiembre", "september"),
        ("octubre", "october"),
        ("noviembre", "november"),
        ("diciembre", "december"),
    ];
    let mut i = 0;
    while i < words.len() {
        if let Some((from, to)) = PHRASES.iter().find(|(from, _)| {
            words.len() - i >= from.len() && from.iter().zip(&words[i..]).all(|(a, b)| a == b)
        }) {
            words.splice(i..i + from.len(), to.iter().map(|s| s.to_string()));
            i += to.len();
            continue;
        }
        // "en 2025", "en marzo", "since marzo", "marzo de 2025".
        let prev = if i > 0 {
            words[i - 1].clone()
        } else {
            String::new()
        };
        let prev = prev.as_str();
        if let Some((_, en)) = MONTHS.iter().find(|(es, _)| *es == words[i]) {
            if matches!(prev, "en" | "since" | "in" | "de")
                || words.get(i + 1).is_some_and(|w| w == "de")
            {
                words[i] = en.to_string();
                if prev == "en" {
                    words[i - 1] = "in".into();
                }
                if words.get(i + 1).is_some_and(|w| w == "de")
                    && words
                        .get(i + 2)
                        .is_some_and(|w| w.len() == 4 && w.chars().all(|c| c.is_ascii_digit()))
                {
                    words.remove(i + 1);
                }
            }
        } else if words[i] == "en"
            && words
                .get(i + 1)
                .is_some_and(|w| w.len() == 4 && w.chars().all(|c| c.is_ascii_digit()))
        {
            words[i] = "in".into();
        }
        i += 1;
    }
}

// ------------------------------------------------------------ date scope

/// Pull `date:`/`after:`/`before:` operators out of `words`, then a
/// trailing natural date phrase ("this year", "in 2025", "since may").
fn take_range(words: &mut Vec<String>, today: NaiveDate) -> (Option<DateRange>, Option<String>) {
    let mut range: Option<DateRange> = None;
    let mut texts: Vec<String> = Vec::new();
    let mut intersect = |r: DateRange| {
        let cur = range.unwrap_or(DateRange {
            from: None,
            to: None,
        });
        range = Some(DateRange {
            from: match (cur.from, r.from) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            },
            to: match (cur.to, r.to) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
        });
    };
    let mut i = 0;
    while i < words.len() {
        let w = words[i].clone();
        let op = ["date:", "after:", "before:"]
            .into_iter()
            .find(|p| w.starts_with(p));
        let Some(op) = op else {
            i += 1;
            continue;
        };
        let value = w[op.len()..].to_string();
        let parts: Vec<&str> = value.split_whitespace().collect();
        let parsed = dates::parse(&parts, today).filter(|(n, _)| *n == parts.len());
        if let Some((_, r)) = parsed {
            match op {
                // Same as search: after: includes that day, before: excludes it.
                "after:" => intersect(DateRange {
                    from: r.from,
                    to: None,
                }),
                "before:" => intersect(DateRange {
                    from: None,
                    to: r.from,
                }),
                _ => intersect(r),
            }
            texts.push(match op {
                "after:" => format!("after {value}"),
                "before:" => format!("before {value}"),
                _ => value.clone(),
            });
            words.remove(i);
            continue;
        }
        i += 1;
    }
    // Trailing phrase: the longest suffix that parses entirely as a date.
    if range.is_none() && words.len() >= 2 {
        for start in 1..words.len() {
            let tail: Vec<&str> = words[start..].iter().map(String::as_str).collect();
            // A lone month name could be a person ("may", "june").
            if tail.len() == 1 && dates::month_num(tail[0]).is_some() {
                continue;
            }
            let Some((n, r)) = dates::parse(&tail, today) else {
                continue;
            };
            if n != tail.len() {
                continue;
            }
            let mut cut = start;
            // "during 2025", "for this year", "over the last 3 months",
            // "in the last 3 months"
            while cut > 1
                && matches!(
                    words[cut - 1].as_str(),
                    "during" | "for" | "over" | "the" | "within" | "in"
                )
            {
                cut -= 1;
            }
            texts.push(tail.join(" "));
            range = Some(r);
            words.truncate(cut);
            break;
        }
    }
    (range, (!texts.is_empty()).then(|| texts.join(", ")))
}

// ------------------------------------------------------------- templates

#[derive(Debug)]
enum Elem {
    Alt(Option<&'static str>, Vec<Vec<String>>),
    Slot(&'static str),
}

const MACROS: &[(&str, &str)] = &[
    (
        "%CONTACT%",
        "<verb:email|e-mail|emailed|mail|message|messaged|text|write to|wrote to|write|wrote|reply to|replied to|respond to|responded to|send|sent|send anything to|sent anything to|talk to|talked to|talk with|talked with|speak to|spoke to|speak with|spoke with|chat with|chatted with|hear from|heard from|get an email from|got an email from|get anything from|got anything from|contact|contacted|reach out to|reached out to|meet|met|meet with|met with|see|saw|correspond with|corresponded with|exchange emails with|exchanged emails with|have contact with|had contact with|get in touch with|got in touch with>",
    ),
    ("%ME%", "(i|we)"),
    ("%MSG%", "(email|emails|e-mail|message|messages|mail|mails|note|reply|response|conversation|contact|exchange|thread|correspondence|interaction|touchpoint|communication)"),
    ("%WORKWITH%", "(work with|work for|worked with|worked for|working with|working for|do business with|did business with|do work for|did work for)"),
    ("%HIRE%", "(start working with|start working for|started working with|started working for|begin working with|begin working for|began working with|began working for|start with|started with|start at|started at|get hired by|got hired by|hire|hired|engage|engaged|partner with|partnered with|sign with|signed with|sign up with|signed up with|onboard|onboarded|onboard with|onboarded with|join|joined|retain|retained|take on|took on|bring on|brought on)"),
    ("%STOP%", "(stop working with|stopped working with|stop working for|stopped working for|part ways with|parted ways with|finish with|finished with|finish working with|finished working with|end with|ended with|end things with|ended things with|leave|left|lose|lost|wrap up with|wrapped up with|stop doing work for|stopped doing work for|offboard from|offboarded from)"),
    ("%LETGO%", "(let us go|let me go|fire us|fire me|fired us|fired me|drop us|drop me|dropped us|dropped me|cut us loose|cut me loose|end the contract|ended the contract|end our contract|ended our contract|end the engagement|ended the engagement|end our engagement|ended our engagement|end the relationship|ended the relationship|cancel|cancelled|canceled|cancel on us|cancel on me|stop working with us|stop working with me|stopped working with us|stopped working with me|part ways|parted ways|terminate us|terminate me|terminated us|terminated me|terminate the contract|terminated the contract|churn|churned|leave us|leave me|left us|left me|pull the plug|pulled the plug|move on|moved on|take it in-house|took it in-house|bring it in-house|brought it in-house)"),
    ("%KIND%", "<kind:invoice|invoices|bill|bills|receipt|receipts|contract|contracts|agreement|agreements|proposal|proposals|pdf|pdfs|attachment|attachments|file|files|document|documents|doc|docs|spreadsheet|spreadsheets|sheet|sheets|photo|photos|image|images|picture|pictures|email|emails|message|messages|mail|reply|replies|response|note|thing|update|statement|statements>"),
    ("%LATEST%", "(latest|last|most recent|newest|recent|final)"),
    ("%FIRST%", "(first|oldest|earliest|very first)"),
    ("%WHEN%", "(when|what date|what day|what time|which day|which date|cuando|que dia|que fecha|a que hora)"),
    ("%WHICH%", "<which:next|upcoming|return|outbound|last|previous|>"),
    ("%FLIGHTFIELD%", "<field:number|flight number|confirmation|confirmation code|confirmation number|confirmation #|booking reference|booking code|booking number|reservation code|reservation number|record locator|pnr|details|info|information|time|times|itinerary|status>"),
    ("%BILL%", "<bill:bill|bills|invoice|invoices|payment|rent|statement|premium|tuition|subscription|renewal|fee>"),
    ("%CODEKIND%", "(verification|login|log in|sign-in|sign in|signin|security|2fa|two-factor|one-time|one time|otp|access|authentication|)"),
    ("%CONTACTFIELD%", "<field:phone|phone number|number|cell|cell number|cell phone|mobile|mobile number|telephone|address|mailing address|office address|home address|street address|postal address|work address|email|email address|e-mail|e-mail address|emails|email addresses|contact info|contact information|contact details|details>"),
    ("%FACTS%", "<facts:orders|purchases|packages|deliveries|parcels|shipments|flights|hotel stays|stays|hotel nights|hotels|reservations|bookings|pedidos|compras|paquetes|envios|vuelos|hoteles|estancias|reservas>"),
];

type Build = fn(&Caps) -> Option<Intent>;

struct Caps(Vec<(&'static str, String)>);

impl Caps {
    fn get(&self, k: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.as_str())
    }
    fn slot(&self, k: &str) -> String {
        self.get(k).unwrap_or("").to_string()
    }
}

fn verb_dir(c: &Caps) -> Dir {
    match c.get("verb").unwrap_or("") {
        v if v.starts_with("hear")
            || v.starts_with("heard")
            || v.starts_with("get")
            || v.starts_with("got") =>
        {
            Dir::FromThem
        }
        v if [
            "email",
            "e-mail",
            "emailed",
            "mail",
            "message",
            "messaged",
            "text",
            "write",
            "wrote",
            "reply",
            "replied",
            "respond",
            "responded",
            "send",
            "sent",
        ]
        .iter()
        .any(|p| v.starts_with(p)) =>
        {
            Dir::ToThem
        }
        _ => Dir::Any,
    }
}

fn kind_of(c: &Caps) -> Kind {
    match c.get("kind").unwrap_or("") {
        "invoice" | "invoices" | "bill" | "bills" | "statement" | "statements" => Kind::Invoice,
        "receipt" | "receipts" => Kind::Receipt,
        "contract" | "contracts" | "agreement" | "agreements" | "proposal" | "proposals" => {
            Kind::Contract
        }
        "pdf" | "pdfs" => Kind::Pdf,
        "attachment" | "attachments" | "file" | "files" => Kind::Attachment,
        "document" | "documents" | "doc" | "docs" => Kind::Doc,
        "spreadsheet" | "spreadsheets" | "sheet" | "sheets" => Kind::Spreadsheet,
        "photo" | "photos" | "image" | "images" | "picture" | "pictures" => Kind::Image,
        _ => Kind::Any,
    }
}

fn some_who(c: &Caps) -> Option<String> {
    c.get("p").map(String::from)
}

const TEMPLATES: &[(&str, Build)] = &[
    // ---- relationship end ("when did he let us go")
    ("%WHEN% did {p} %LETGO%", |c| Some(Intent::Relationship { who: c.slot("p"), focus: Focus::End })),
    ("%WHEN% (was|were) (i|we) (let go|fired|dropped|terminated) by {p}", |c| Some(Intent::Relationship { who: c.slot("p"), focus: Focus::End })),
    ("%WHEN% did %ME% %STOP% {p}", |c| Some(Intent::Relationship { who: c.slot("p"), focus: Focus::End })),
    ("%WHEN% did (the|our|my) (work|relationship|engagement|contract|project|retainer) (with|for) {p} (end|stop|finish|wrap up)", |c| {
        Some(Intent::Relationship { who: c.slot("p"), focus: Focus::End })
    }),
    ("(did|has|have) {p} %LETGO%", |c| Some(Intent::Relationship { who: c.slot("p"), focus: Focus::End })),
    ("(are|is) %ME% still (working with|working for|doing work for|emailing|talking to|in touch with) {p}", |c| {
        Some(Intent::Relationship { who: c.slot("p"), focus: Focus::End })
    }),
    // ---- relationship start: when it began is the first email with them
    // (the mail can't show a hire date; the answer says so)
    ("%WHEN% did %ME% (start|begin|first start) (working|work|doing work) (with|for) {p}", |c| {
        Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Start })
    }),
    ("%WHEN% did (the|our|my) (work|relationship|engagement|contract|project|retainer) (with|for) {p} (start|begin|kick off)", |c| {
        Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Start })
    }),
    ("%WHEN% did {p} (start working with|hire|sign|onboard|engage|retain|take on|bring on) (us|me)", |c| Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Start })),
    ("%WHEN% did %ME% %HIRE% {p}", |c| Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Start })),
    // ---- relationship span
    ("%WHEN% did %ME% %WORKWITH% {p}", |c| Some(Intent::Relationship { who: c.slot("p"), focus: Focus::Span })),
    ("how long (have|had) %ME% (known|know|been emailing|been emailing with|been talking to|been writing to|been in touch with|been corresponding with) {p}", |c| {
        Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Known })
    }),
    ("how long (have|did|had) %ME% (worked with|worked for|work with|work for|been working with|been working for|been doing business with|been dealing with|dealt with) {p}", |c| {
        Some(Intent::Relationship { who: c.slot("p"), focus: Focus::Span })
    }),
    ("how long (was|were) %ME% (working with|working for|with) {p}", |c| Some(Intent::Relationship { who: c.slot("p"), focus: Focus::Span })),
    ("(what is|show|show me|give me) (my|our|the) (history|relationship|timeline) with {p}", |c| {
        Some(Intent::Relationship { who: c.slot("p"), focus: Focus::Span })
    }),
    ("(my|our) (history|relationship|timeline) with {p}", |c| Some(Intent::Relationship { who: c.slot("p"), focus: Focus::Span })),
    ("(timeline|history) (of|with|for) {p}", |c| Some(Intent::Relationship { who: c.slot("p"), focus: Focus::Span })),
    ("(for) how long (have|did) %ME% %WORKWITH% {p}", |c| Some(Intent::Relationship { who: c.slot("p"), focus: Focus::Span })),
    // ---- first contact
    ("%WHEN% did %ME% first %CONTACT% {p}", |c| Some(Intent::FirstContact { who: c.slot("p"), dir: verb_dir(c), first: First::Email })),
    ("%WHEN% did {p} first (email|e-mail|message|write to|contact|reach out to|reply to|get in touch with) (me|us)", |c| {
        Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::FromThem, first: First::Email })
    }),
    ("%WHEN% was (the|my|our) %FIRST% %MSG% <prep:from|with|to|between me and|between us and> {p}", |c| Some(Intent::FirstContact { who: c.slot("p"), dir: prep_dir(c), first: First::Email })),
    // "the first email I sent Priya", "the first email Priya sent me".
    ("(when was|what date was|what was|) (the|my|our|) %FIRST% %MSG% (i|we) (sent|wrote|emailed|wrote to|sent to) {p}", |c| {
        Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::ToThem, first: First::Email })
    }),
    ("(when was|what date was|what was|) (the|my|our|) %FIRST% %MSG% {p} (sent|wrote|emailed) (me|us)", |c| {
        Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::FromThem, first: First::Email })
    }),
    ("(what was|show me|show|find|open|) (the|my|our|) %FIRST% %MSG% <prep:from|with|to|between me and|between us and> {p}", |c| {
        Some(Intent::FirstContact { who: c.slot("p"), dir: prep_dir(c), first: First::Email })
    }),
    ("%WHEN% did %ME% (meet|get introduced to|get to know|first hear about|start talking to|start emailing|start emailing with) {p}", |c| {
        Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Email })
    }),
    ("how did %ME% (meet|get introduced to|get connected with) {p}", |c| Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Email })),
    // ---- waiting on / owe replies (before the generic "did X reply")
    ("(what|which|what emails|which emails|what threads|which threads|who) (am|are) %ME% (still|) waiting (on|for) (a reply|replies|an answer|a response|) from {p}", |c| {
        Some(Intent::WaitingOn { who: some_who(c) })
    }),
    ("(what|which|what emails|which emails|what threads|which threads|who) (am|are) %ME% (still|) waiting (on|for) (a reply|replies|an answer|a response|)", |_| {
        Some(Intent::WaitingOn { who: None })
    }),
    ("(am|are) %ME% (still|) waiting (on|for) {p}", |c| Some(Intent::WaitingOn { who: some_who(c) })),
    ("(did|has|have) {p} (replied|reply|responded|respond|answered|answer|gotten back to me|got back to me|get back to me|gotten back to us|got back to us|get back to us|written back|write back)", |c| {
        Some(Intent::WaitingOn { who: some_who(c) })
    }),
    ("(what|who) (has not|have not|did not) (replied|reply|responded|respond|gotten back to me|got back to me|get back to me|answered|answer) (to me|to my emails|yet|)", |c| {
        Some(Intent::WaitingOn { who: some_who(c) })
    }),
    ("(unanswered|unreplied) (sent|outgoing) (emails|messages|mail)", |_| Some(Intent::WaitingOn { who: None })),
    ("(what|which) (emails|messages|threads) (are|am) (i|we) (still|) waiting on", |_| Some(Intent::WaitingOn { who: None })),
    ("(what|who) do (i|we) owe (a reply|replies|a response|responses|an answer|answers|emails) to {p}", |c| Some(Intent::OweReplies { who: some_who(c) })),
    ("(what|who) do (i|we) owe (a reply|replies|a response|responses|an answer|answers|emails) (to|)", |_| Some(Intent::OweReplies { who: None })),
    ("(do|did) (i|we) owe {p} (a reply|a response|an answer|an email|anything)", |c| Some(Intent::OweReplies { who: some_who(c) })),
    ("(who|what) (do|should|must) (i|we) (need to|have to|still|) (reply to|respond to|answer|get back to|write back to)", |_| Some(Intent::OweReplies { who: None })),
    ("(what|which) (emails|messages|threads|mail) (do|should) (i|we) (need to|have to|still|) (reply to|respond to|answer|get back to)", |_| {
        Some(Intent::OweReplies { who: None })
    }),
    ("(what|which) (emails|messages|threads|mail|) (needs|need) (a reply|replies|a response|responses|an answer|my reply|my attention)", |_| {
        Some(Intent::OweReplies { who: None })
    }),
    ("(what|who) have (i|we) not (replied to|responded to|answered|gotten back to)", |_| Some(Intent::OweReplies { who: None })),
    ("(unanswered|unreplied|unresponded) (emails|messages|mail|threads)", |_| Some(Intent::OweReplies { who: None })),
    ("who am i (ignoring|not replying to|leaving hanging)", |_| Some(Intent::OweReplies { who: None })),
    ("(did|have) (i|we) (reply|replied|respond|responded|answer|answered|get back|gotten back|got back) to {p}", |c| {
        Some(Intent::OweReplies { who: some_who(c) })
    }),
    // ---- last contact
    ("%WHEN% (did|have|had) %ME% last %CONTACT% {p}", |c| Some(Intent::LastContact { who: c.slot("p"), dir: verb_dir(c) })),
    ("%WHEN% (did|have) %ME% %CONTACT% {p} last", |c| Some(Intent::LastContact { who: c.slot("p"), dir: verb_dir(c) })),
    ("%WHEN% (was|is) the last time (that|) %ME% %CONTACT% {p}", |c| Some(Intent::LastContact { who: c.slot("p"), dir: verb_dir(c) })),
    ("%WHEN% (was|is) the last time {p} (emailed|e-mailed|messaged|wrote to|contacted|replied to|reached out to|got back to) (me|us)", |c| {
        Some(Intent::LastContact { who: c.slot("p"), dir: Dir::FromThem })
    }),
    ("%WHEN% did {p} last (email|e-mail|message|write to|write|contact|reply to|respond to|reach out to|get back to) (me|us|)", |c| {
        Some(Intent::LastContact { who: c.slot("p"), dir: Dir::FromThem })
    }),
    ("%WHEN% (was|is) (my|our|the) (last|latest|most recent) %MSG% <prep:with|from|to> {p}", |c| {
        let dir = match c.get("prep") {
            Some("from") => Dir::FromThem,
            Some("to") => Dir::ToThem,
            _ => Dir::Any,
        };
        Some(Intent::LastContact { who: c.slot("p"), dir })
    }),
    ("(the|my|our|) last time %ME% %CONTACT% {p}", |c| Some(Intent::LastContact { who: c.slot("p"), dir: verb_dir(c) })),
    ("(the|my|our|) (last|latest|most recent) (contact|conversation|exchange|interaction|touchpoint|correspondence) with {p}", |c| {
        Some(Intent::LastContact { who: c.slot("p"), dir: Dir::Any })
    }),
    ("(have|did) %ME% %CONTACT% {p} (recently|lately|this week|this month)", |c| Some(Intent::LastContact { who: c.slot("p"), dir: verb_dir(c) })),
    ("(have|did) %ME% %CONTACT% {p}", |c| Some(Intent::LastContact { who: c.slot("p"), dir: verb_dir(c) })),
    ("how long (has it been|since) (i|we) (last|) %CONTACT% {p}", |c| Some(Intent::LastContact { who: c.slot("p"), dir: verb_dir(c) })),
    ("(is|was) {p} (in touch|emailing me|writing to me) (recently|lately|)", |c| Some(Intent::LastContact { who: c.slot("p"), dir: Dir::FromThem })),
    // ---- spend
    ("how much (did|have|do|had) %ME% (spend|spent|pay|paid|spending) (on|at|to|for|with|in) {m}", |c| Some(Intent::Spend { merchant: c.slot("m") })),
    ("how much (did|have|do) %ME% (spend|spent|pay|paid) {m}", |c| Some(Intent::Spend { merchant: c.slot("m") })),
    ("how much (did|has|have) {m} (charge|charged|bill|billed|cost) (me|us)", |c| Some(Intent::Spend { merchant: c.slot("m") })),
    ("how much (money|) (went to|did i give|have i given) {m}", |c| Some(Intent::Spend { merchant: c.slot("m") })),
    ("(what|how much) (is|was|are|were) (my|our|the) (total|) {m} (spend|spending|total|bill|bills|charges|costs|expenses)", |c| {
        Some(Intent::Spend { merchant: c.slot("m") })
    }),
    ("(total|sum|sum of|add up) (my|our|the|) (spend|spending|spent|charges|costs|expenses|receipts|payments) (on|at|for|from|to) {m}", |c| {
        Some(Intent::Spend { merchant: c.slot("m") })
    }),
    ("(total|add up|sum) (my|our|the|) {m} (receipts|charges|spend|spending|expenses|bills|invoices)", |c| Some(Intent::Spend { merchant: c.slot("m") })),
    ("{m} (spend|spending|expenses|charges) (total|)", |c| Some(Intent::Spend { merchant: c.slot("m") })),
    // ---- counts
    ("how many (emails|messages|mails|e-mails|times) (did|has|have) {p} (sent|send|emailed|email|written|write|wrote|messaged) (me|us|)", |c| {
        Some(Intent::Count { who: c.slot("p"), dir: Dir::FromThem, kind: Kind::Any })
    }),
    ("how many (emails|messages|mails|e-mails|times) (did|have) %ME% (sent|send|emailed|email|written|write|wrote|messaged) (to|) {p}", |c| {
        Some(Intent::Count { who: c.slot("p"), dir: Dir::ToThem, kind: Kind::Any })
    }),
    ("how many (emails|messages|mails|e-mails|threads|conversations) (have|did|do) %ME% (had|have|exchanged|exchange) with {p}", |c| {
        Some(Intent::Count { who: c.slot("p"), dir: Dir::Any, kind: Kind::Any })
    }),
    ("how many (emails|messages|mails|e-mails) <prep:from|by|to|with|between me and> {p}", |c| {
        let dir = match c.get("prep") {
            Some("from" | "by") => Dir::FromThem,
            Some("to") => Dir::ToThem,
            _ => Dir::Any,
        };
        Some(Intent::Count { who: c.slot("p"), dir, kind: Kind::Any })
    }),
    ("how many %KIND% (did|has|have) {p} (sent|send) (me|us|)", |c| Some(Intent::Count { who: c.slot("p"), dir: Dir::FromThem, kind: kind_of(c) })),
    ("how many %KIND% (from|by) {p}", |c| Some(Intent::Count { who: c.slot("p"), dir: Dir::FromThem, kind: kind_of(c) })),
    ("how often (do|does|did) {p} (email|e-mail|write to|message|contact) (me|us)", |c| {
        Some(Intent::Count { who: c.slot("p"), dir: Dir::FromThem, kind: Kind::Any })
    }),
    ("how often (do|did) (i|we) (email|e-mail|write to|message|talk to|hear from) {p}", |c| {
        Some(Intent::Count { who: c.slot("p"), dir: Dir::Any, kind: Kind::Any })
    }),
    ("(count|number of) (emails|messages|mails) <prep:from|to|with> {p}", |c| {
        let dir = match c.get("prep") {
            Some("from") => Dir::FromThem,
            Some("to") => Dir::ToThem,
            _ => Dir::Any,
        };
        Some(Intent::Count { who: c.slot("p"), dir, kind: Kind::Any })
    }),
    ("how many (emails|messages|mails|threads) (about|mentioning|mention|regarding|re|on|with the word|containing) {t}", |c| {
        Some(Intent::CountTopic { topic: c.slot("t") })
    }),
    // ---- top senders
    ("who (emails|emailed|messages|messaged|writes to|wrote to|sends|sent) (me|us) the most", |_| Some(Intent::TopSenders)),
    ("who (sends|sent|emails|emailed|writes|wrote) (me|us) the most (emails|email|mail|messages)", |_| Some(Intent::TopSenders)),
    ("who (do|did) (i|we) (get|receive|hear from) the most (email|emails|mail|messages|from)", |_| Some(Intent::TopSenders)),
    ("(top|most frequent|biggest) (senders|emailers|correspondents|contacts)", |_| Some(Intent::TopSenders)),
    ("who are my (top|most frequent) (senders|contacts|correspondents)", |_| Some(Intent::TopSenders)),
    // ---- who emailed about
    ("who (emailed|e-mailed|messaged|wrote|wrote to|contacted|sent|emails|e-mails|messages|writes|writes to|contacts|sends) (me|us|) (about|regarding|re|concerning|on) {t}", |c| Some(Intent::WhoAbout { topic: c.slot("t") })),
    ("who (sent|shared|forwarded) (me|us|) (the|a|an|that|) {t}", |c| Some(Intent::WhoAbout { topic: c.slot("t") })),
    ("who (mentioned|talked about|asked about|brought up|wrote about|emailed about|was talking about|is talking about) {t}", |c| Some(Intent::WhoAbout { topic: c.slot("t") })),
    ("who (was|is|were) (involved in|on|in) (the|) {t} (thread|email|emails|conversation|discussion)", |c| Some(Intent::WhoAbout { topic: c.slot("t") })),
    ("who (knows|knew) about {t}", |c| Some(Intent::WhoAbout { topic: c.slot("t") })),
    // ---- who is
    ("who (is|was|are) {p}", |c| Some(Intent::WhoIs { who: c.slot("p") })),
    ("(tell me about|what do i know about|what do we know about|info on|details on|look up|lookup) {p}", |c| Some(Intent::WhoIs { who: c.slot("p") })),
    ("(what is|what are) {p} (email|email address|address|contact info|domain)", |c| Some(Intent::WhoIs { who: c.slot("p") })),
    ("(what is|what was) {p} 's (email|email address|address|contact info)", |c| Some(Intent::WhoIs { who: c.slot("p") })),
    // ---- latest item
    ("(what|where|which) (is|was|were|are) (the|my|our|) %LATEST% %KIND% (from|by|sent by|that) {p} (sent|sent me|sent us|)", |c| {
        Some(Intent::LatestItem { who: some_who(c), kind: kind_of(c) })
    }),
    ("(what|where|which) (is|was|were|are) (the|my|our|) %LATEST% %KIND%", |c| Some(Intent::LatestItem { who: None, kind: kind_of(c) })),
    ("(show|show me|find|get|open|pull up|grab|give me) (the|my|our|) %LATEST% %KIND% (from|by|sent by) {p}", |c| {
        Some(Intent::LatestItem { who: some_who(c), kind: kind_of(c) })
    }),
    ("(show|show me|find|get|open|pull up|grab|give me) (the|my|our|) %LATEST% %KIND%", |c| Some(Intent::LatestItem { who: None, kind: kind_of(c) })),
    ("(the|my|our|) %LATEST% %KIND% (from|by|sent by) {p}", |c| Some(Intent::LatestItem { who: some_who(c), kind: kind_of(c) })),
    ("{p} 's %LATEST% %KIND%", |c| Some(Intent::LatestItem { who: some_who(c), kind: kind_of(c) })),
    ("(what is|what was|what's) (the|) (last|latest|most recent) thing {p} (sent|sent me|sent us|emailed me|emailed us|said|wrote)", |c| {
        Some(Intent::LatestItem { who: some_who(c), kind: Kind::Any })
    }),
    ("(the|) (last|latest|most recent) thing {p} (sent|sent me|sent us|emailed me|emailed us|said|wrote)", |c| {
        Some(Intent::LatestItem { who: some_who(c), kind: Kind::Any })
    }),
    ("what did {p} (last|) (send|sent|email|say|write) (me|us|) (last|)", |c| Some(Intent::LatestItem { who: some_who(c), kind: Kind::Any })),
    ("(the|my|our|) %LATEST% %KIND%", |c| Some(Intent::LatestItem { who: None, kind: kind_of(c) })),
    ("(did|has) {p} (send|sent) (me|us) (an|a|the|any) %KIND%", |c| Some(Intent::LatestItem { who: some_who(c), kind: kind_of(c) })),
    // ---- dates in text
    ("%WHEN% (is|was) {t} due", |c| Some(Intent::When { topic: c.slot("t") })),
    ("%WHEN% (is|was|are|were|will be) {t}", |c| Some(Intent::When { topic: c.slot("t") })),
    ("%WHEN% (does|do|did) {t} (start|begin|end|happen|expire|renew|close|open|launch|ship|arrive|take place|kick off|go live|end up happening|finish|land|come)", |c| {
        Some(Intent::When { topic: c.slot("t") })
    }),
    ("(what is|what's) the date (of|for) {t}", |c| Some(Intent::When { topic: c.slot("t") })),
    ("(date|deadline|due date) (of|for) {t}", |c| Some(Intent::When { topic: c.slot("t") })),
];

/// Templates tried before [`TEMPLATES`]: the questions about extracted
/// facts (travel, orders, bills, bookings, codes, contact details) and
/// person + topic questions, which the general templates would otherwise
/// read too loosely. Spanish templates are written without accents
/// (`normalize` folds them).
const TEMPLATES_FIRST: &[(&str, Build)] = &[
    // ---- flights
    ("%WHEN% <aux:is|was|does|did> (my|our|the) %WHICH% (flight|flights|plane) (to|for|into|from) {place} (leave|depart|take off|land|arrive|board|boarding|)", |c| Some(flight(c, true))),
    ("%WHEN% <aux:is|was|does|did> (my|our|the) %WHICH% (flight|plane) (leave|depart|take off|land|arrive|board|boarding|)", |c| Some(flight(c, false))),
    ("%WHEN% <aux:do|did|am|are|will> (i|we) (fly|flying|fly out|flying out|fly back|flying back|leave|leaving|depart|departing|head|heading) (to|for|out to|back to|home to) {place}", |c| Some(flight(c, true))),
    ("%WHEN% <aux:do|did|am|are|will> (i|we) (fly|flying|fly out|flying out|fly back|flying back|fly home|flying home)", |c| Some(flight(c, false))),
    ("<verb:what is|what's|what was|show me|show|give me|get|find> (my|our|the) %WHICH% flight %FLIGHTFIELD% (to|for|into|from) {place}", |c| Some(flight(c, true))),
    ("<verb:what is|what's|what was|show me|show|give me|get|find> (my|our|the) %WHICH% flight %FLIGHTFIELD%", |c| Some(flight(c, false))),
    ("<verb:what is|what's|what was|show me|give me|get|find|> (my|our|the|) %FLIGHTFIELD% (for|of|on) (my|our|the) %WHICH% flight (to|for|into|from) {place}", |c| Some(flight(c, true))),
    ("<verb:what is|what's|what was|show me|give me|get|find|> (my|our|the|) %FLIGHTFIELD% (for|of|on) (my|our|the) %WHICH% flight", |c| Some(flight(c, false))),
    // "the reservation code for my Skyward Air flight to Lisbon": the
    // airline adds nothing the booking doesn't say.
    ("<verb:what is|what's|what was|show me|give me|get|find|> (my|our|the|) %FLIGHTFIELD% (for|of|on) (my|our|the) %WHICH% {airline} flight (to|for|into|from) {place}", |c| Some(flight(c, true))),
    ("<verb:what is|what's|what was|show me|give me|get|find|> (my|our|the|) %FLIGHTFIELD% (for|of|on) (my|our|the) %WHICH% {airline} flight", |c| Some(flight(c, false))),
    ("(my|our|the|) %WHICH% (flight|flights) (to|for|into|from) {place}", |c| Some(flight(c, true))),
    ("(my|our|the|) <which:next|upcoming|last|previous|recent> (flight|flights)", |c| Some(flight(c, false))),
    ("(my|our) (flight|flights)", |c| Some(flight(c, false))),
    ("(what|which) flights (do|did|have) (i|we) (have|got|booked|book|taken|take|) (coming up|booked|lately|recently|)", |c| Some(flight(c, false))),
    ("(show|show me|list) (my|our|the|) <which:upcoming|next|recent|last|> flights", |c| Some(flight(c, false))),
    ("(what|which) airline (am|are|was|were) (i|we) (flying|taking|on) (to|for|into) {place}", |c| Some(flight(c, true))),
    // ---- stays
    ("where <aux:am|are|was|were|will> (i|we) (be|) staying (in|at|for|during) {place}", |c| Some(stay(c, true))),
    ("where <aux:am|are|was|were|will> (i|we) (be|) staying", |c| Some(stay(c, false))),
    ("(what is|what's|what was|which is) (my|our|the) (hotel|airbnb|accommodation|accommodations|lodging|rental|place) (in|at|for) {place}", |c| Some(stay(c, true))),
    ("(my|our|the) (hotel|airbnb|accommodation|accommodations|lodging|stay|hotel reservation|hotel booking|rental) (in|at|for) {place}", |c| Some(stay(c, true))),
    ("(my|our) <which:next|upcoming|last|> (hotel|airbnb|accommodation|lodging|stay|hotel reservation|hotel booking|hotels|stays)", |c| Some(stay(c, false))),
    ("where <aux:did|do|will> (i|we) stay (in|at|during) {place}", |c| Some(stay(c, true))),
    ("where <aux:did|do|will> (i|we) stay", |c| Some(stay(c, false))),
    ("%WHEN% (do|can|did|should|must) (i|we) check (in|out) (at|to|in|into|of|from) {place}", |c| Some(stay(c, true))),
    ("%WHEN% (do|can|did|should|must) (i|we) check (in|out)", |c| Some(stay(c, false))),
    // ---- parcels
    ("(where is|where are|what happened to) (my|our|the) {what} (order|package|parcel|delivery|shipment|stuff)", |c| Some(Intent::Package { what: thing(c, "what") })),
    ("(where is|where are|what happened to) (my|our|the) (order|orders|package|packages|parcel|parcels|delivery|deliveries|shipment|shipments|stuff)", |_| Some(Intent::Package { what: None })),
    ("(where is|where are) (my|our|the) (order|package|parcel|delivery|shipment) from {what}", |c| Some(Intent::Package { what: thing(c, "what") })),
    ("(track|tracking|tracking number|tracking info|tracking status|status) (for|of|on|) (my|our|the) {what} (order|package|parcel|delivery|shipment)", |c| Some(Intent::Package { what: thing(c, "what") })),
    ("(track|tracking|tracking number|tracking info|tracking status|status) (for|of|on|) (my|our|the) (order|package|parcel|delivery|shipment) from {what}", |c| Some(Intent::Package { what: thing(c, "what") })),
    ("(track|tracking|tracking number|tracking info|tracking status) (for|of|on|) (my|our|the) (order|orders|package|packages|parcel|delivery|deliveries|shipment|shipments)", |_| Some(Intent::Package { what: None })),
    ("(what is|what was|give me|find|get|show me) (the|my|our|) (tracking number|tracking info|tracking status|tracking link|tracking) (for|of|on) (my|our|the) {what} (order|package|parcel|delivery|shipment)", |c| Some(Intent::Package { what: thing(c, "what") })),
    ("(what is|what was|give me|find|get|show me) (the|my|our|) (tracking number|tracking info|tracking status|tracking link|tracking) (for|of|on) (my|our|the) (order|package|parcel|delivery|shipment)", |_| Some(Intent::Package { what: None })),
    ("%WHEN% (will|does|is|should|did) (my|our|the) {what} (arrive|arriving|get here|be delivered|be here|come|show up|ship|deliver|land)", |c| Some(Intent::Package { what: thing(c, "what") })),
    ("(has|did|is|have) (my|our|the) {what} (shipped|ship|arrived|arrive|been delivered|delivered|come|out for delivery|on its way|on the way|been shipped)", |c| Some(Intent::Package { what: thing(c, "what") })),
    ("(what|which) (packages|deliveries|shipments|orders) (are|is) (coming|arriving|on the way|out for delivery|in transit|pending)", |_| Some(Intent::Package { what: None })),
    ("(what|which) (packages|deliveries|shipments|orders) (am|are) (i|we) (expecting|waiting for|waiting on)", |_| Some(Intent::Package { what: None })),
    ("(incoming|upcoming|pending) (packages|deliveries|shipments)", |_| Some(Intent::Package { what: None })),
    // ---- orders
    ("what (did|have) (i|we) (order|ordered|buy|bought|purchase|purchased|get|got) (from|at|on) {m}", |c| Some(Intent::Orders { merchant: thing(c, "m") })),
    ("what (did|have) (i|we) (order|ordered|buy|bought|purchase|purchased) (recently|lately|)", |_| Some(Intent::Orders { merchant: None })),
    ("(my|our|the) (last|latest|most recent|recent|previous) (order|orders|purchase|purchases) (from|at|on|with) {m}", |c| Some(Intent::Orders { merchant: thing(c, "m") })),
    ("(my|our|the) (last|latest|most recent|recent|previous) {m} (order|orders|purchase|purchases)", |c| Some(Intent::Orders { merchant: thing(c, "m") })),
    ("(my|our) (recent|last|latest) (orders|purchases|order|purchase)", |_| Some(Intent::Orders { merchant: None })),
    ("(what is|what was) (my|the|our) (order number|order #|order id) (for|of|from|on) (my|the|our|) {m} (order|purchase|)", |c| Some(Intent::Orders { merchant: thing(c, "m") })),
    ("(order number|order #|order id) (for|of|from) (my|the|our|) {m} (order|purchase|)", |c| Some(Intent::Orders { merchant: thing(c, "m") })),
    // ---- bills
    ("%WHEN% (is|was|are|were) (my|the|our|this|next) {what} %BILL% due", |c| Some(Intent::Bills { what: bill_what(c) })),
    ("%WHEN% (is|was|are) (my|the|our) (next|) %BILL% (from|for|to|with) {what} due", |c| Some(Intent::Bills { what: bill_what(c) })),
    ("%WHEN% (is|was|are) (my|the|our|this) (next|) %BILL% due", |c| Some(Intent::Bills { what: bill_what(c) })),
    ("%WHEN% (do|should|must) (i|we) (pay|have to pay|need to pay) (the|my|our|) {what} (bill|invoice|statement|)", |c| Some(Intent::Bills { what: thing(c, "what") })),
    ("(what|which) (bills|invoices|payments) (are|is|do i have|do we have) (due|coming up|upcoming|outstanding|unpaid|open|to pay)", |_| Some(Intent::Bills { what: None })),
    ("(what|which) (bills|invoices|payments) (do|should) (i|we) (have to pay|need to pay|pay|owe)", |_| Some(Intent::Bills { what: None })),
    ("(upcoming|unpaid|outstanding|open|due|my) (bills|invoices|payments)", |_| Some(Intent::Bills { what: None })),
    ("(bills|invoices|payments) (due|coming up|to pay)", |_| Some(Intent::Bills { what: None })),
    ("how much (is|was) (my|the|our) (latest|last|next|current|) {what} %BILL%", |c| Some(Intent::Bills { what: bill_what(c) })),
    ("(what is|what was) (my|the|our) (latest|last|current|most recent|newest) {what} (bill|invoice|statement)", |c| Some(Intent::Bills { what: thing(c, "what") })),
    ("how much (is|was) (my|the|our) (latest|last|next|current|new|) %BILL%", |c| Some(Intent::Bills { what: bill_what(c) })),
    ("how much do (i|we) owe {what}", |c| Some(Intent::Bills { what: thing(c, "what") })),
    // ---- bookings
    ("%WHEN% (is|was) (my|our|the) (dinner|lunch|brunch|breakfast|restaurant|table) (reservation|booking) (at|for|with) {what}", |c| Some(Intent::Booking { what: thing(c, "what") })),
    ("%WHEN% (is|was) (my|our|the) <meal:dinner|lunch|brunch|breakfast|restaurant|table> (reservation|booking)", |c| Some(Intent::Booking { what: c.get("meal").map(String::from) })),
    ("%WHEN% (is|was) (my|our|the) (reservation|booking) (at|for|with) {what}", |c| Some(Intent::Booking { what: thing(c, "what") })),
    ("(my|our|the) (dinner|lunch|brunch|restaurant|table) (reservation|reservations|booking) (at|for) {what}", |c| Some(Intent::Booking { what: thing(c, "what") })),
    ("(my|our) (reservation|booking) (at|for) {what}", |c| Some(Intent::Booking { what: thing(c, "what") })),
    ("%WHEN% (is|was|are|were) (my|our|the) (tickets|ticket|show|concert|game) (for|to) {what}", |c| Some(Intent::Booking { what: thing(c, "what") })),
    ("(my|our|the) (tickets|ticket) (for|to) {what}", |c| Some(Intent::Booking { what: thing(c, "what") })),
    ("(what|which) (reservations|bookings|tickets|events|shows|concerts) (do|did) (i|we) have (coming up|booked|)", |_| Some(Intent::Booking { what: None })),
    ("(upcoming|my|our) (reservations|bookings|tickets)", |_| Some(Intent::Booking { what: None })),
    // ---- subscriptions: recurring charges (the smart view's rule)
    ("(what|which) (subscriptions|memberships|recurring charges|recurring payments) (do|am|are|have) (i|we) (have|pay for|paying for|pay|paying|subscribed to|signed up for|got|) (right now|now|currently|)", |_| Some(Intent::Subscriptions)),
    ("(what|which) (subscriptions|memberships|recurring charges|recurring payments) (do|am|are|have) (i|we) (currently|still|) (have|pay for|paying for|pay|paying|got)", |_| Some(Intent::Subscriptions)),
    ("(what|which) (am|are) (i|we) (paying for|subscribed to|charged for) (every month|monthly|each month|every year|yearly|)", |_| Some(Intent::Subscriptions)),
    ("(what|which) do (i|we) pay for (every month|monthly|each month|every year|yearly)", |_| Some(Intent::Subscriptions)),
    ("(list|show|show me|find) (my|our|all my|all our|all) (subscriptions|memberships|recurring charges|recurring payments)", |_| Some(Intent::Subscriptions)),
    ("(my|our|active) (subscriptions|memberships|recurring charges|recurring payments)", |_| Some(Intent::Subscriptions)),
    ("how many (subscriptions|memberships|recurring charges) (do|have) (i|we) (have|pay for|got|)", |_| Some(Intent::Subscriptions)),
    ("how much (do|am|are|did) (i|we) (spend|spending|pay|paying) (on|for) (my|our|all my|all|) (subscriptions|memberships|recurring charges) (a month|per month|each month|every month|monthly|a year|per year|each year|every year|yearly|in total|total|altogether|)", |_| Some(Intent::Subscriptions)),
    ("how much (do|does) (my|our|all my) (subscriptions|memberships) cost (me|us|) (a month|per month|each month|every month|monthly|a year|per year|yearly|in total|total|altogether|)", |_| Some(Intent::Subscriptions)),
    ("(que|cuales) (suscripciones|membresias) (tengo|pago|tenemos|pagamos)", |_| Some(Intent::Subscriptions)),
    ("(mis|nuestras) (suscripciones|membresias)", |_| Some(Intent::Subscriptions)),
    // ---- verification codes
    ("(what is|what was|show me|show|get|copy|give me) (my|the|our) (latest|last|newest|most recent|recent|) %CODEKIND% (code|passcode|pin|otp) (from|for) {service}", |c| Some(Intent::Code { service: thing(c, "service") })),
    ("(what is|what was|show me|show|get|copy|give me) (my|the|our) (latest|last|newest|most recent|recent|) %CODEKIND% (code|passcode|pin|otp)", |_| Some(Intent::Code { service: None })),
    ("(what is|what was|show me|show|get|copy|give me) (my|the|our) {service} %CODEKIND% (code|passcode|pin|otp)", |c| Some(Intent::Code { service: thing(c, "service") })),
    ("(latest|last|newest|recent|my) %CODEKIND% (code|codes|passcode) (from|for) {service}", |c| Some(Intent::Code { service: thing(c, "service") })),
    ("(latest|last|newest|recent|my) %CODEKIND% (code|codes|passcode)", |_| Some(Intent::Code { service: None })),
    ("(verification|login|sign-in|security|2fa|otp|one-time|access) (code|codes) (from|for) {service}", |c| Some(Intent::Code { service: thing(c, "service") })),
    // ---- contact details
    // "their email", "his phone number": the person from the previous answer.
    ("<pro:his|her|their> %CONTACTFIELD%", |c| Some(Intent::ContactInfo { who: c.slot("pro"), field: contact_field(c) })),
    ("(what is|what was|give me|find|get|tell me) <pro:his|her|their> %CONTACTFIELD%", |c| Some(Intent::ContactInfo { who: c.slot("pro"), field: contact_field(c) })),
    ("(what is|what was|give me|find|get|tell me) {p} 's %CONTACTFIELD%", |c| Some(Intent::ContactInfo { who: c.slot("p"), field: contact_field(c) })),
    ("{p} 's %CONTACTFIELD%", |c| Some(Intent::ContactInfo { who: c.slot("p"), field: contact_field(c) })),
    ("(what is|find|get) the %CONTACTFIELD% (for|of) {p}", |c| Some(Intent::ContactInfo { who: c.slot("p"), field: contact_field(c) })),
    ("%CONTACTFIELD% (for|of) {p}", |c| Some(Intent::ContactInfo { who: c.slot("p"), field: contact_field(c) })),
    ("how (do|can|should) (i|we) (call|phone|ring|text) {p}", |c| Some(Intent::ContactInfo { who: c.slot("p"), field: "phone" })),
    ("how (do|can|should) (i|we) (reach|contact|get in touch with|get hold of|get a hold of|email|e-mail|write to) {p}", |c| Some(Intent::ContactInfo { who: c.slot("p"), field: "contact" })),
    ("where (is|are) {p} (located|based|headquartered)", |c| Some(Intent::ContactInfo { who: c.slot("p"), field: "address" })),
    // ---- did they reply about X
    ("(did|has|have) {p} (replied|reply|responded|respond|answered|answer|gotten back to me|got back to me|get back to me|gotten back to us|got back to us|get back to us|written back|write back) (about|regarding|re|on) {t}", |c| Some(Intent::DidReply { who: c.slot("p"), topic: c.slot("t") })),
    ("(did|has|have) {p} (replied|reply|responded|respond|answered|answer|gotten back to me|got back to me|get back to me|written back|write back) to (my|our|the) (email|message|note|question|proposal) (about|regarding|re|on) {t}", |c| Some(Intent::DidReply { who: c.slot("p"), topic: c.slot("t") })),
    ("(did|have) (i|we) (heard|hear) back from {p} (about|regarding|re|on) {t}", |c| Some(Intent::DidReply { who: c.slot("p"), topic: c.slot("t") })),
    // ---- what X said about Y
    ("what did {p} (say|write|mention|tell me|tell us|think|decide|conclude|propose|suggest|ask|want|need|send) (about|regarding|re|on|of|for|concerning) {t}", |c| Some(Intent::Said { who: some_who(c), topic: c.slot("t") })),
    ("what (does|did) {p} (think|say|feel) (about|of|on|regarding) {t}", |c| Some(Intent::Said { who: some_who(c), topic: c.slot("t") })),
    ("(did|has|have) {p} (say|said|mention|mentioned|write|wrote|tell me|told me|tell us|told us|ask|asked|bring up|brought up) (anything|something|) (about|regarding|on|re) {t}", |c| Some(Intent::Said { who: some_who(c), topic: c.slot("t") })),
    ("what (is|was) {p} 's (take|opinion|view|feedback|position|answer|response|stance|thinking|thoughts) (on|about|regarding) {t}", |c| Some(Intent::Said { who: some_who(c), topic: c.slot("t") })),
    ("what (did|have) (we|i) (decide|decided|agree|agreed|conclude|concluded|settle on|settled on) (about|on|regarding|for|with) {t}", |c| Some(Intent::Said { who: None, topic: c.slot("t") })),
    ("what (was|is) the (final|) (decision|conclusion|outcome|plan|status|update|verdict|result|consensus) (on|about|for|regarding|with) {t}", |c| Some(Intent::Said { who: None, topic: c.slot("t") })),
    // ---- counts of extracted things
    ("how many %FACTS% (did|have|do) (i|we) (place|placed|get|got|receive|received|take|taken|took|have|had|make|made|book|booked|order|ordered|) (from|with|at|on) {m}", |c| Some(Intent::CountFacts { kind: fact_kind(c)?, who: thing(c, "m") })),
    ("how many %FACTS% (did|have|do) (i|we) (place|placed|get|got|receive|received|take|taken|took|have|had|make|made|book|booked|order|ordered|)", |c| Some(Intent::CountFacts { kind: fact_kind(c)?, who: None })),
    ("how many %FACTS% (from|with|at|on) {m}", |c| Some(Intent::CountFacts { kind: fact_kind(c)?, who: thing(c, "m") })),
    // ================================================== Spanish
    ("cuando fue la ultima vez que (le|les|) (escribi|conteste|respondi|mande un correo|envie un correo|mande un mail|envie un mail|mande un mensaje) (a|) {p}", |c| Some(Intent::LastContact { who: c.slot("p"), dir: Dir::ToThem })),
    ("cuando fue la ultima vez que (hable|me comunique|hablamos) con {p}", |c| Some(Intent::LastContact { who: c.slot("p"), dir: Dir::Any })),
    ("cuando fue la ultima vez que {p} (me|nos) (escribio|contesto|respondio)", |c| Some(Intent::LastContact { who: c.slot("p"), dir: Dir::FromThem })),
    ("cuando (le|les|) (escribi|conteste|respondi) (por ultima vez|) a {p} (por ultima vez|)", |c| Some(Intent::LastContact { who: c.slot("p"), dir: Dir::ToThem })),
    ("cuando (me|nos) (escribio|contesto|respondio) {p} (por ultima vez|)", |c| Some(Intent::LastContact { who: c.slot("p"), dir: Dir::FromThem })),
    ("cuando (fue|es) (mi|el|nuestro) (ultimo|mas reciente) (correo|email|mensaje|contacto) (con|de|a) {p}", |c| Some(Intent::LastContact { who: c.slot("p"), dir: Dir::Any })),
    ("cuando (fue|es) (mi|el|nuestro) primer (correo|email|mensaje|contacto) (con|de|a) {p}", |c| Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Email })),
    ("cuando (empece|empezamos|comence|comenzamos) a trabajar con {p}", |c| Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Start })),
    ("cuando (contrate|contratamos) a {p}", |c| Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Start })),
    ("cuanto tiempo (llevo|llevamos|hace que) (conozco a|conocemos a) {p}", |c| Some(Intent::FirstContact { who: c.slot("p"), dir: Dir::Any, first: First::Known })),
    ("cuanto tiempo (llevo|llevamos|hace que) (trabajando con|trabajo con|trabajamos con) {p}", |c| Some(Intent::Relationship { who: c.slot("p"), focus: Focus::Span })),
    ("cuanto (gaste|he gastado|gastamos|hemos gastado|pague|he pagado|pagamos|hemos pagado) (en|a|con|por) {m}", |c| Some(Intent::Spend { merchant: c.slot("m") })),
    ("cuanto (dinero|) (me|nos) (cobro|ha cobrado|cobraron|han cobrado) {m}", |c| Some(Intent::Spend { merchant: c.slot("m") })),
    ("cuantos (correos|emails|mensajes|mails) (me|nos) (envio|mando|escribio|ha enviado|ha mandado) {p}", |c| Some(Intent::Count { who: c.slot("p"), dir: Dir::FromThem, kind: Kind::Any })),
    ("cuantos (correos|emails|mensajes|mails) <prep:de|con|a> {p}", |c| {
        let dir = match c.get("prep") {
            Some("de") => Dir::FromThem,
            Some("a") => Dir::ToThem,
            _ => Dir::Any,
        };
        Some(Intent::Count { who: c.slot("p"), dir, kind: Kind::Any })
    }),
    ("cuantos %FACTS% (hice|he hecho|tuve|he tenido|tome|hicimos|tuvimos|reserve|pedi|he pedido|) (de|en|con|a) {m}", |c| Some(Intent::CountFacts { kind: fact_kind(c)?, who: thing(c, "m") })),
    ("cuantos %FACTS% (hice|he hecho|tuve|he tenido|tome|hicimos|tuvimos|reserve|pedi|he pedido|)", |c| Some(Intent::CountFacts { kind: fact_kind(c)?, who: None })),
    ("(donde esta|donde estan|que paso con) (mi|mis|el|los|nuestro) (paquete|paquetes|pedido|pedidos|envio|envios) (de|del) {what}", |c| Some(Intent::Package { what: thing(c, "what") })),
    ("(donde esta|donde estan|que paso con) (mi|mis|el|los|nuestro) (paquete|paquetes|pedido|pedidos|envio|envios)", |_| Some(Intent::Package { what: None })),
    ("cuando (llega|llegara|va a llegar|llegan|llegaran) (mi|el|mis|nuestro) (paquete|pedido|envio|paquetes|pedidos) (de|del) {what}", |c| Some(Intent::Package { what: thing(c, "what") })),
    ("cuando (llega|llegara|va a llegar|llegan|llegaran) (mi|el|mis|nuestro) (paquete|pedido|envio|paquetes|pedidos)", |_| Some(Intent::Package { what: None })),
    ("(seguimiento|numero de seguimiento|rastreo) (de|del|para) (mi|el) (pedido|paquete|envio) (de|del) {what}", |c| Some(Intent::Package { what: thing(c, "what") })),
    ("(seguimiento|numero de seguimiento|rastreo) (de|del|para) (mi|el) (pedido|paquete|envio)", |_| Some(Intent::Package { what: None })),
    ("que (compre|pedi|he comprado|he pedido|compramos|pedimos) (en|a|de) {m}", |c| Some(Intent::Orders { merchant: thing(c, "m") })),
    ("cuando <aux:es|sale|fue|salio> (mi|el|nuestro) <which:proximo|ultimo|> vuelo (a|para|hacia|de|desde) {place}", |c| Some(flight(c, true))),
    ("cuando <aux:es|sale|fue|salio> (mi|el|nuestro) <which:proximo|ultimo|> vuelo", |c| Some(flight(c, false))),
    ("(mi|el|nuestro) <which:proximo|ultimo|> vuelo (a|para|hacia|de|desde) {place}", |c| Some(flight(c, true))),
    ("cuando (vuelo|viajo|volamos|viajamos|salgo|salimos) (a|para|hacia) {place}", |c| Some(Intent::Flight { place: thing(c, "place"), which: Which::Next, field: None })),
    ("(cual es|dame|) (el|mi) <field:codigo de reserva|localizador|numero de vuelo|numero de reserva> (de|del|para) (mi|el) vuelo (a|para|hacia) {place}", |c| Some(flight(c, true))),
    ("(cual es|dame|) (el|mi) <field:codigo de reserva|localizador|numero de vuelo|numero de reserva> (de|del|para) (mi|el) vuelo", |c| Some(flight(c, false))),
    ("(donde|en que hotel) (me hospedo|me quedo|nos hospedamos|nos quedamos|me alojo|nos alojamos) (en|) {place}", |c| Some(stay(c, true))),
    ("(mi|el|nuestro) (hotel|alojamiento|airbnb) (en|de|para) {place}", |c| Some(stay(c, true))),
    ("cuando vence (la|mi|el|nuestra) (factura|recibo|pago|cuota) (de|del) {what}", |c| Some(Intent::Bills { what: thing(c, "what") })),
    ("cuando (hay que|tengo que|debo) pagar (la|el|mi) (factura|recibo|cuota) (de|del) {what}", |c| Some(Intent::Bills { what: thing(c, "what") })),
    ("cuando vence (la|mi|el) (factura|recibo|pago)", |_| Some(Intent::Bills { what: None })),
    ("que (facturas|recibos|pagos) (tengo|hay) (pendientes|por pagar)", |_| Some(Intent::Bills { what: None })),
    ("(facturas|recibos|pagos) pendientes", |_| Some(Intent::Bills { what: None })),
    ("cuando es (mi|la|nuestra) (reserva|mesa|cena) (en|para|de) {what}", |c| Some(Intent::Booking { what: thing(c, "what") })),
    ("(mi|el|nuestro) (ultimo|) codigo (de verificacion|de acceso|de seguridad|) (de|para) {service}", |c| Some(Intent::Code { service: thing(c, "service") })),
    ("(cual es|dame) (el|mi) (ultimo|) codigo (de verificacion|de acceso|de seguridad|) (de|para) {service}", |c| Some(Intent::Code { service: thing(c, "service") })),
    ("(cual es|dame|) (el|mi) (ultimo|) codigo (de verificacion|de acceso|de seguridad)", |_| Some(Intent::Code { service: None })),
    ("(codigo|codigos) (de verificacion|de acceso|de seguridad) (de|para) {service}", |c| Some(Intent::Code { service: thing(c, "service") })),
    ("quien (me|nos) (escribio|hablo|mando un correo|envio un correo|dijo algo) (sobre|acerca de|de|respecto a) {t}", |c| Some(Intent::WhoAbout { topic: c.slot("t") })),
    ("quien es {p}", |c| Some(Intent::WhoIs { who: c.slot("p") })),
    ("que (dijo|opina|penso|piensa|comento|escribio) {p} (sobre|acerca de|de|respecto a|respecto de) {t}", |c| Some(Intent::Said { who: some_who(c), topic: c.slot("t") })),
    ("que (decidimos|acordamos) (sobre|acerca de|con|respecto a) {t}", |c| Some(Intent::Said { who: None, topic: c.slot("t") })),
    ("(me|nos) (contesto|respondio) {p} (sobre|acerca de|respecto a) {t}", |c| Some(Intent::DidReply { who: c.slot("p"), topic: c.slot("t") })),
    ("(ya|) (me|nos) (contesto|respondio) {p}", |c| Some(Intent::WaitingOn { who: some_who(c) })),
    ("(estoy|estamos) esperando (respuesta|una respuesta|) de {p}", |c| Some(Intent::WaitingOn { who: some_who(c) })),
    ("que (respuestas|correos) (estoy|estamos) esperando", |_| Some(Intent::WaitingOn { who: None })),
    ("a quien (le debo|le tengo que|tengo que|debo) (una respuesta|responder|contestar)", |_| Some(Intent::OweReplies { who: None })),
    ("(cual es|dame) el <field:telefono|numero|numero de telefono|celular|movil|direccion|correo|correo electronico|email> de {p}", |c| Some(Intent::ContactInfo { who: c.slot("p"), field: contact_field(c) })),
    ("(el|) <field:telefono|numero de telefono|celular|movil|direccion|correo|correo electronico|email> de {p}", |c| Some(Intent::ContactInfo { who: c.slot("p"), field: contact_field(c) })),
    ("cuando (es|fue|sera) {t}", |c| Some(Intent::When { topic: c.slot("t") })),
];

/// Templates tried after [`TEMPLATES`]: finding a thing, and open topic
/// questions answered with quoted sentences.
const TEMPLATES_LAST: &[(&str, Build)] = &[
    ("(find|show|show me|where is|pull up|get|open|grab|look for|look up|search for) (the|my|our|that|a|an) {t} (from|by|sent by) {p} (sent|sent me|sent us|)", |c| Some(find(c, true))),
    ("(find|show me|pull up|look for) (the|my|our|that) {t} (email|emails|message|thread|attachment|file|document|doc|pdf)", |c| Some(find(c, false))),
    ("(find|pull up|look for) (the|my|our|that) {t}", |c| Some(find(c, false))),
    ("(what|which) (is|was|are|were) (the|my|our) {t}", |c| Some(Intent::Passage { topic: c.slot("t") })),
    ("how (do|does|did|can|should) (i|we) {t}", |c| Some(Intent::Passage { topic: c.slot("t") })),
    ("why (did|does|is|was|were|do|are) {t}", |c| Some(Intent::Passage { topic: c.slot("t") })),
    ("where (is|was|are|were|do|did) (the|my|our|we|i) {t}", |c| Some(Intent::Passage { topic: c.slot("t") })),
    ("(que|cual) (es|fue|era) (el|la|mi|nuestro|nuestra) {t}", |c| Some(Intent::Passage { topic: c.slot("t") })),
];

fn which_of(c: &Caps) -> Which {
    // "What was my flight number to Austin" asks about the past one.
    if c.get("verb") == Some("what was") && c.get("which").is_none_or(str::is_empty) {
        return Which::Last;
    }
    match c.get("which").filter(|w| !w.is_empty()).or(c.get("aux")) {
        Some(
            "next" | "upcoming" | "return" | "proximo" | "is" | "does" | "do" | "am" | "are"
            | "will" | "es" | "sale",
        ) => Which::Next,
        Some(
            "last" | "previous" | "recent" | "ultimo" | "was" | "did" | "were" | "fue" | "salio",
        ) => Which::Last,
        _ => Which::Any,
    }
}

fn flight_field(c: &Caps) -> Option<&'static str> {
    let f = c.get("field")?;
    Some(match f {
        "number" | "flight number" | "numero de vuelo" => "number",
        "time" | "times" | "status" => "time",
        "details" | "info" | "information" | "itinerary" => return None,
        _ => "confirmation",
    })
}

fn flight(c: &Caps, with_place: bool) -> Intent {
    Intent::Flight {
        place: if with_place { thing(c, "place") } else { None },
        which: which_of(c),
        field: flight_field(c),
    }
}

fn stay(c: &Caps, with_place: bool) -> Intent {
    // "my hotel in Lisbon", "the airbnb at Porto": the place after the
    // lodging word.
    let place = if with_place { thing(c, "place") } else { None }.map(|p| {
        let words: Vec<&str> = p.split_whitespace().collect();
        match words.iter().rposition(|w| matches!(*w, "in" | "at" | "en")) {
            Some(i) if i + 1 < words.len()
                && words[..i].iter().all(|w| {
                    matches!(*w, "hotel" | "airbnb" | "rental" | "accommodation" | "lodging" | "stay" | "place" | "room")
                }) =>
            {
                words[i + 1..].join(" ")
            }
            _ => p,
        }
    });
    Intent::Stay {
        place,
        which: which_of(c),
    }
}

/// A bill question's subject: the named biller, else a specific kind of
/// bill ("rent", "tuition"); None for plain "bill"/"invoice".
fn bill_what(c: &Caps) -> Option<String> {
    thing(c, "what").or_else(|| {
        c.get("bill")
            .filter(|b| {
                !matches!(
                    *b,
                    "bill" | "bills" | "invoice" | "invoices" | "payment" | "statement"
                )
            })
            .map(String::from)
    })
}

fn contact_field(c: &Caps) -> &'static str {
    match c.get("field").unwrap_or("") {
        f if f.starts_with("email") || f.starts_with("e-mail") || f.contains("correo") => "email",
        f if f.starts_with("contact") || f == "details" || f == "datos de contacto" => "contact",
        f if f.contains("address") || f.contains("direccion") => "address",
        _ => "phone",
    }
}

/// Direction from "the first email from/to/with X".
fn prep_dir(c: &Caps) -> Dir {
    match c.get("prep") {
        Some("from") => Dir::FromThem,
        Some("to") => Dir::ToThem,
        _ => Dir::Any,
    }
}

fn fact_kind(c: &Caps) -> Option<FactKind> {
    Some(match c.get("facts")? {
        "orders" | "purchases" | "pedidos" | "compras" => FactKind::Order,
        "packages" | "deliveries" | "parcels" | "shipments" | "paquetes" | "envios" => {
            FactKind::Shipment
        }
        "flights" | "vuelos" => FactKind::Flight,
        "hotel stays" | "stays" | "hotel nights" | "hotels" | "hoteles" | "estancias" => {
            FactKind::Stay
        }
        "reservations" | "bookings" | "reservas" => FactKind::Booking,
        "bills" | "invoices" | "facturas" => FactKind::Bill,
        _ => return None,
    })
}

/// A slot naming a thing, without words that only name the kind ("my
/// order" → nothing; "my paperleaf order" → "paperleaf").
fn thing(c: &Caps, k: &str) -> Option<String> {
    const GENERIC: &[&str] = &[
        "order",
        "orders",
        "package",
        "packages",
        "parcel",
        "delivery",
        "shipment",
        "stuff",
        "bill",
        "bills",
        "invoice",
        "payment",
        "the",
        "my",
        "our",
        "a",
        "an",
        "it",
        "that",
        "this",
        "next",
        "latest",
        "last",
        "new",
        "recent",
        "reservation",
        "booking",
        "tickets",
        "ticket",
        "pedido",
        "paquete",
        "envio",
        "factura",
        "recibo",
        "el",
        "la",
        "mi",
    ];
    let v = c.get(k)?;
    let words: Vec<&str> = v
        .split_whitespace()
        .filter(|w| !GENERIC.contains(w))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

fn find(c: &Caps, with_who: bool) -> Intent {
    let who = if with_who { some_who(c) } else { None };
    let topic = c.slot("t");
    // "find the contract from Priya" is the latest-item question.
    let k = kind_of(&Caps(vec![("kind", topic.clone())]));
    if k != Kind::Any || matches!(topic.as_str(), "email" | "emails" | "message" | "messages") {
        return Intent::LatestItem { who, kind: k };
    }
    Intent::Find { who, topic }
}

fn expand(p: &str) -> String {
    let mut s = p.to_string();
    for (k, v) in MACROS {
        s = s.replace(k, v);
    }
    s
}

fn parse_pattern(p: &str) -> Vec<Elem> {
    // Patterns and names are 'static; leak once (the table is built once).
    let p: &'static str = Box::leak(expand(p).into_boxed_str());
    let mut out = Vec::new();
    let b = p.as_bytes();
    let mut i = 0;
    // Longest alternative first, so "meet with dana" never leaves "with"
    // in the slot; the empty (optional) one naturally goes last.
    let alts = |body: &str| -> Vec<Vec<String>> {
        let mut v: Vec<Vec<String>> = body
            .split('|')
            .map(|a| a.split_whitespace().map(String::from).collect())
            .collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.len()));
        v
    };
    while i < b.len() {
        match b[i] {
            b' ' => i += 1,
            b'(' => {
                let e = i + p[i..].find(')').expect("unclosed (");
                out.push(Elem::Alt(None, alts(&p[i + 1..e])));
                i = e + 1;
            }
            b'<' => {
                let e = i + p[i..].find('>').expect("unclosed <");
                let (name, body) = p[i + 1..e].split_once(':').expect("<name:...>");
                out.push(Elem::Alt(Some(name), alts(body)));
                i = e + 1;
            }
            b'{' => {
                let e = i + p[i..].find('}').expect("unclosed {");
                out.push(Elem::Slot(&p[i + 1..e]));
                i = e + 1;
            }
            _ => {
                let e = p[i..].find(' ').map_or(p.len(), |n| i + n);
                out.push(Elem::Alt(None, vec![vec![p[i..e].to_string()]]));
                i = e;
            }
        }
    }
    out
}

fn table() -> &'static [(Vec<Elem>, Build)] {
    static T: OnceLock<Vec<(Vec<Elem>, Build)>> = OnceLock::new();
    T.get_or_init(|| {
        let mut v = Vec::new();
        for (p, build) in TEMPLATES_FIRST
            .iter()
            .chain(TEMPLATES)
            .chain(TEMPLATES_LAST)
        {
            v.push((parse_pattern(p), *build));
        }
        v
    })
}

fn matches(pat: &[Elem], words: &[&str], caps: &mut Vec<(&'static str, String)>) -> bool {
    let Some(first) = pat.first() else {
        return words.is_empty();
    };
    match first {
        Elem::Alt(name, alts) => {
            for alt in alts {
                if alt.len() <= words.len() && alt.iter().zip(words).all(|(a, w)| a == w) {
                    let mark = caps.len();
                    if let Some(n) = name {
                        caps.push((n, alt.join(" ")));
                    }
                    if matches(&pat[1..], &words[alt.len()..], caps) {
                        return true;
                    }
                    caps.truncate(mark);
                }
            }
            false
        }
        Elem::Slot(name) => {
            for k in 1..=words.len() {
                let mark = caps.len();
                caps.push((name, words[..k].join(" ")));
                if matches(&pat[1..], &words[k..], caps) {
                    return true;
                }
                caps.truncate(mark);
            }
            false
        }
    }
}

/// Parse a question. `today` is the user's local date.
pub(crate) fn parse(question: &str, today: NaiveDate) -> Question {
    let mut words = normalize(question);
    let (range, range_text) = take_range(&mut words, today);
    let refs: Vec<&str> = words.iter().map(String::as_str).collect();
    let text = refs.join(" ");
    for (pat, build) in table() {
        let mut caps = Vec::new();
        if matches(pat, &refs, &mut caps) {
            if let Some(intent) = build(&Caps(caps)).map(tidy).filter(valid) {
                return Question {
                    intent,
                    range,
                    range_text,
                    text,
                };
            }
        }
    }
    Question {
        intent: Intent::Unknown,
        range,
        range_text,
        text,
    }
}

/// Tidy a capture: a merchant is its name, without "my" in front or
/// "rides" / "orders" / "receipts" after it.
fn tidy(i: Intent) -> Intent {
    match i {
        Intent::Spend { merchant } => {
            let mut w: Vec<&str> = merchant.split_whitespace().collect();
            while w.len() > 1 && matches!(w[0], "my" | "the" | "our" | "all" | "mis" | "el" | "la" | "of") {
                w.remove(0);
            }
            while w.len() > 1
                && matches!(
                    *w.last().unwrap_or(&""),
                    "ride" | "rides" | "order" | "orders" | "receipt" | "receipts" | "purchase"
                        | "purchases" | "subscription" | "subscriptions" | "trips" | "trip"
                )
            {
                w.pop();
            }
            Intent::Spend {
                merchant: w.join(" "),
            }
        }
        other => other,
    }
}

/// Reject captures that are only glue words ("who is it", "when is the").
fn valid(i: &Intent) -> bool {
    let meaningful = |s: &str| {
        s.split_whitespace().any(|w| {
            !matches!(
                w,
                "the" | "a" | "an" | "my" | "our" | "it" | "that" | "this" | "there" | "'s"
            )
        })
    };
    match i {
        Intent::LastContact { who, .. }
        | Intent::FirstContact { who, .. }
        | Intent::Relationship { who, .. } => meaningful(who),
        // "how many emails did I send": the sender is you, not someone
        // named "I".
        Intent::Count { who, .. } => meaningful(who) && !matches!(who.as_str(), "i" | "we" | "me" | "us" | "you"),
        Intent::WhoIs { who } => meaningful(who) && !who.ends_with("'s"),
        Intent::LatestItem { who, .. } | Intent::WaitingOn { who } | Intent::OweReplies { who } => {
            who.as_deref().is_none_or(meaningful)
        }
        // A merchant is a name, not the rest of a question ("how much did
        // i", "average cedar valley power", "per month on swiftcab").
        Intent::Spend { merchant } => {
            meaningful(merchant)
                && !merchant.split_whitespace().any(|w| {
                    matches!(
                        w,
                        "how" | "much" | "did" | "do" | "i" | "we" | "what" | "when" | "which"
                            | "average" | "per" | "most" | "least" | "biggest" | "largest"
                            | "highest" | "lowest" | "cheapest" | "last" | "latest" | "first"
                            | "next" | "total" | "month" | "monthly" | "each" | "every" | "more"
                            | "or" | "than" | "vs" | "cost" | "costs" | "sum"
                    )
                })
        }
        Intent::WhoAbout { topic } => {
            meaningful(topic) && !topic.starts_with("most ") && !topic.starts_with("the most")
        }
        Intent::CountTopic { topic }
        | Intent::When { topic }
        | Intent::Passage { topic } => meaningful(topic),
        Intent::ContactInfo { who, .. } => meaningful(who) && !who.contains("'s"),
        // "what did we decide" is the group's decision, not a person's.
        Intent::Said { who, topic } | Intent::Find { who, topic } => {
            meaningful(topic)
                && who
                    .as_deref()
                    .is_none_or(|w| meaningful(w) && !matches!(w, "we" | "i" | "you" | "us"))
        }
        Intent::DidReply { who, topic } => meaningful(who) && meaningful(topic),
        Intent::Flight { place, .. } | Intent::Stay { place, .. } => {
            place.as_deref().is_none_or(meaningful)
        }
        Intent::Package { what: x }
        | Intent::Orders { merchant: x }
        | Intent::Bills { what: x }
        | Intent::Booking { what: x }
        | Intent::Code { service: x }
        | Intent::CountFacts { who: x, .. } => x.as_deref().is_none_or(meaningful),
        Intent::TopSenders | Intent::Subscriptions | Intent::Unknown => true,
    }
}

/// Words that mean "the person we were just talking about".
pub(crate) fn is_pronoun(who: &str) -> bool {
    matches!(
        who,
        "he" | "she"
            | "they"
            | "him"
            | "her"
            | "them"
            | "his"
            | "their"
            | "this person"
            | "that person"
            | "this guy"
            | "that guy"
            | "this company"
            | "that company"
            | "this client"
            | "that client"
            | "the client"
            | "this one"
            | "that one"
    )
}

/// Does the input read like a question rather than a search? Used by
/// callers that route between search and Ask.
pub fn looks_like_question(input: &str) -> bool {
    let t = input.trim_start();
    if t.starts_with('?') {
        return true;
    }
    let words = normalize(t);
    let first = words.first().map(String::as_str).unwrap_or("");
    if words.len() < 2 {
        return false;
    }
    let second = words.get(1).map(String::as_str).unwrap_or("");
    matches!(
        first,
        "when"
            | "who"
            | "whom"
            | "how"
            | "what"
            | "which"
            | "did"
            | "have"
            | "has"
            | "am"
            | "are"
            | "is"
            | "was"
            | "do"
            | "does"
            | "where"
            | "why"
            | "track"
            | "tracking"
            | "cuando"
            | "cuanto"
            | "cuantos"
            | "cuantas"
            | "donde"
            | "quien"
            | "que"
            | "cual"
    ) || (first == "my"
        && matches!(
            second,
            "flight"
                | "flights"
                | "next"
                | "upcoming"
                | "hotel"
                | "order"
                | "orders"
                | "package"
                | "packages"
                | "bills"
                | "reservation"
                | "reservations"
                | "tickets"
        ))
        // "total cost of my Linear receipts", "sum of my invoices", "add up…"
        || matches!(
            (first, second),
            ("total" | "sum" | "add", "cost" | "of" | "spent" | "spend" | "paid" | "for" | "my" | "up")
        )
        // "Linear total this year"
        || (words.len() >= 3 && second == "total")
        // "first email with Priya", "oldest email from Dana"
        || (matches!(first, "first" | "oldest" | "earliest")
            && matches!(second, "email" | "emails" | "message" | "messages" | "mail"))
        // "their email", "his phone number"
        || (matches!(first, "his" | "her" | "their")
            && matches!(second, "email" | "phone" | "number" | "address" | "contact"))
        // "Priya's email", "Dana Whitfield's phone number"
        || (words.len() <= 6
            && words.iter().position(|w| w == "'s").is_some_and(|i| {
                i >= 1
                    && words.get(i + 1).is_some_and(|f| {
                        matches!(f.as_str(), "email" | "phone" | "number" | "address" | "contact" | "cell" | "mobile")
                    })
            }))
        || (t.trim_end().ends_with('?') && words.len() >= 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 24).unwrap()
    }

    fn p(q: &str) -> Intent {
        parse(q, today()).intent
    }

    fn lc(who: &str, dir: Dir) -> Intent {
        Intent::LastContact {
            who: who.into(),
            dir,
        }
    }
    fn fc(who: &str, dir: Dir) -> Intent {
        fcf(who, dir, First::Email)
    }
    fn fcf(who: &str, dir: Dir, first: First) -> Intent {
        Intent::FirstContact {
            who: who.into(),
            dir,
            first,
        }
    }
    fn contact(who: &str, field: &'static str) -> Intent {
        Intent::ContactInfo {
            who: who.into(),
            field,
        }
    }
    fn rel(who: &str, focus: Focus) -> Intent {
        Intent::Relationship {
            who: who.into(),
            focus,
        }
    }
    fn latest(who: Option<&str>, kind: Kind) -> Intent {
        Intent::LatestItem {
            who: who.map(String::from),
            kind,
        }
    }
    fn count(who: &str, dir: Dir, kind: Kind) -> Intent {
        Intent::Count {
            who: who.into(),
            dir,
            kind,
        }
    }

    #[test]
    fn contact_details_and_subscriptions() {
        let cases: Vec<(&str, Intent)> = vec![
            ("Priya's email", contact("priya", "email")),
            ("what's Priya's email address?", contact("priya", "email")),
            ("email address for Theo", contact("theo", "email")),
            ("what is the email for Julia Brandt", contact("julia brandt", "email")),
            ("Mike's email", contact("mike", "email")),
            ("how do I reach Dana", contact("dana", "contact")),
            ("how can I contact Ravi", contact("ravi", "contact")),
            ("how do I get in touch with Linden", contact("linden", "contact")),
            ("Priya's contact details", contact("priya", "contact")),
            ("how do I call Dana", contact("dana", "phone")),
            ("Dana's phone number", contact("dana", "phone")),
            ("their email", contact("their", "email")),
            ("what's his phone number", contact("his", "phone")),
            ("¿cuál es el correo de Lucía?", contact("lucia", "email")),
            ("what subscriptions do I pay for", Intent::Subscriptions),
            ("my subscriptions", Intent::Subscriptions),
            ("how much do I spend on subscriptions a month", Intent::Subscriptions),
            ("what am I paying for every month", Intent::Subscriptions),
            ("¿qué suscripciones tengo?", Intent::Subscriptions),
            ("who emails me about the budget", Intent::WhoAbout { topic: "the budget".into() }),
        ];
        for (q, want) in cases {
            assert_eq!(p(q), want, "{q}");
        }
        // "their" needs someone to refer to; the answer asks (run.rs).
        assert!(is_pronoun("their") && is_pronoun("his"));
    }

    #[test]
    fn intent_table() {
        use Dir::*;
        use Focus::*;
        let cases: Vec<(&str, Intent)> = vec![
            // The questions that motivated Ask.
            (
                "when did I last speak to Mike from Kettle on the Knoll / Fernwood?",
                lc("mike from kettle on the knoll / fernwood", Any),
            ),
            (
                "when did we work for Mike KOTK, from when to now?",
                rel("mike kotk", Span),
            ),
            ("when did he let us go?", rel("he", End)),
            // Last contact.
            ("when did I last email Priya?", lc("priya", ToThem)),
            (
                "When did I last hear from priya@linden.example",
                lc("priya@linden.example", FromThem),
            ),
            ("when did we last talk to Linden", lc("linden", Any)),
            (
                "when was the last time I emailed Sam Rivera",
                lc("sam rivera", ToThem),
            ),
            ("when was the last time Sam emailed me", lc("sam", FromThem)),
            ("when did Sam last email me?", lc("sam", FromThem)),
            ("when was my last email with Priya", lc("priya", Any)),
            ("last conversation with Priya", lc("priya", Any)),
            ("have I talked to Priya recently?", lc("priya", Any)),
            ("did I email Omar", lc("omar", ToThem)),
            ("when did i last meet with dana", lc("dana", Any)),
            (
                "when's the last time i heard from dana",
                lc("dana", FromThem),
            ),
            ("how long since I last emailed Omar", lc("omar", ToThem)),
            ("When did I email Priya last?", lc("priya", ToThem)),
            // First contact.
            ("first email from Priya", fc("priya", FromThem)),
            ("when did I first email Omar", fc("omar", ToThem)),
            ("when did Priya first email me", fc("priya", FromThem)),
            ("when did I meet Dana", fc("dana", Any)),
            ("when was the first message with Linden", fc("linden", Any)),
            ("oldest email with Priya", fc("priya", Any)),
            ("the earliest email from Priya", fc("priya", FromThem)),
            ("first email I sent Priya", fc("priya", ToThem)),
            ("what was the first email Priya sent me", fc("priya", FromThem)),
            ("show me the first email between me and Linden", fc("linden", Any)),
            // When a working relationship began: the first email, framed.
            ("when did I hire Julia", fcf("julia", Any, First::Start)),
            ("when did we hire Linden?", fcf("linden", Any, First::Start)),
            ("when did Fernhill Bakery hire me", fcf("fernhill bakery", Any, First::Start)),
            ("when did I start working with Tom", fcf("tom", Any, First::Start)),
            (
                "when did we start working with Linden?",
                fcf("linden", Any, First::Start),
            ),
            ("when did we sign with Linden", fcf("linden", Any, First::Start)),
            ("how long have I known Priya?", fcf("priya", Any, First::Known)),
            ("how long have we been emailing Linden", fcf("linden", Any, First::Known)),
            ("cuanto tiempo hace que conozco a Priya", fcf("priya", Any, First::Known)),
            ("cuando contrate a Julia", fcf("julia", Any, First::Start)),
            // Relationship.
            ("how long have we worked with Linden", rel("linden", Span)),
            ("when did I work with Linden", rel("linden", Span)),
            ("when did we stop working with Linden", rel("linden", End)),
            ("when did Linden let me go", rel("linden", End)),
            ("when did they end the contract", rel("they", End)),
            ("are we still working with Linden", rel("linden", End)),
            ("my history with Priya", rel("priya", Span)),
            (
                "when did the engagement with Linden end",
                rel("linden", End),
            ),
            // Latest item.
            (
                "latest invoice from Linden",
                latest(Some("linden"), Kind::Invoice),
            ),
            (
                "what was the last receipt from Uber?",
                latest(Some("uber"), Kind::Receipt),
            ),
            (
                "show me the most recent contract from Priya",
                latest(Some("priya"), Kind::Contract),
            ),
            ("last thing Omar sent me", latest(Some("omar"), Kind::Any)),
            (
                "what did Omar send me last",
                latest(Some("omar"), Kind::Any),
            ),
            ("Priya's latest pdf", latest(Some("priya"), Kind::Pdf)),
            ("latest attachment", latest(None, Kind::Attachment)),
            (
                "find the last spreadsheet from Dana",
                latest(Some("dana"), Kind::Spreadsheet),
            ),
            ("last reply from Sam", latest(Some("sam"), Kind::Any)),
            // Counts.
            (
                "how many emails from Priya this year",
                count("priya", FromThem, Kind::Any),
            ),
            (
                "how many emails did Priya send me in 2025?",
                count("priya", FromThem, Kind::Any),
            ),
            (
                "how many emails have I sent to Omar",
                count("omar", ToThem, Kind::Any),
            ),
            (
                "how many messages with Linden",
                count("linden", Any, Kind::Any),
            ),
            (
                "how many invoices from Linden last year",
                count("linden", FromThem, Kind::Invoice),
            ),
            (
                "how often does Sam email me",
                count("sam", FromThem, Kind::Any),
            ),
            (
                "how many emails about the lease",
                Intent::CountTopic {
                    topic: "the lease".into(),
                },
            ),
            // Spend.
            (
                "how much did I spend on Uber this year?",
                Intent::Spend {
                    merchant: "uber".into(),
                },
            ),
            (
                "how much have we paid Linden",
                Intent::Spend {
                    merchant: "linden".into(),
                },
            ),
            (
                "how much did Uber charge me last month",
                Intent::Spend {
                    merchant: "uber".into(),
                },
            ),
            (
                "what's my total Uber spend in 2026",
                Intent::Spend {
                    merchant: "uber".into(),
                },
            ),
            (
                "total spent on doordash",
                Intent::Spend {
                    merchant: "doordash".into(),
                },
            ),
            // Who.
            (
                "who emailed me about the lease renewal?",
                Intent::WhoAbout {
                    topic: "the lease renewal".into(),
                },
            ),
            (
                "who sent the W-9",
                Intent::WhoAbout {
                    topic: "w-9".into(),
                },
            ),
            (
                "who mentioned pricing last week",
                Intent::WhoAbout {
                    topic: "pricing".into(),
                },
            ),
            (
                "who is Priya?",
                Intent::WhoIs {
                    who: "priya".into(),
                },
            ),
            (
                "who's mike@kettleontheknoll.example",
                Intent::WhoIs {
                    who: "mike@kettleontheknoll.example".into(),
                },
            ),
            (
                "tell me about Dana Whitfield",
                Intent::WhoIs {
                    who: "dana whitfield".into(),
                },
            ),
            ("who emails me the most", Intent::TopSenders),
            // Open loops.
            ("what am I waiting on?", Intent::WaitingOn { who: None }),
            (
                "what am I waiting on from Omar",
                Intent::WaitingOn {
                    who: Some("omar".into()),
                },
            ),
            (
                "am I waiting on Priya",
                Intent::WaitingOn {
                    who: Some("priya".into()),
                },
            ),
            (
                "did Priya reply?",
                Intent::WaitingOn {
                    who: Some("priya".into()),
                },
            ),
            (
                "has Omar gotten back to me",
                Intent::WaitingOn {
                    who: Some("omar".into()),
                },
            ),
            ("who hasn't replied to me", Intent::WaitingOn { who: None }),
            (
                "what do I owe replies to?",
                Intent::OweReplies { who: None },
            ),
            (
                "who do I need to reply to",
                Intent::OweReplies { who: None },
            ),
            ("what needs a reply", Intent::OweReplies { who: None }),
            ("unanswered emails", Intent::OweReplies { who: None }),
            (
                "do I owe Priya a reply?",
                Intent::OweReplies {
                    who: Some("priya".into()),
                },
            ),
            (
                "did I reply to Omar",
                Intent::OweReplies {
                    who: Some("omar".into()),
                },
            ),
            // Dates in text.
            (
                "when is my lease renewal?",
                Intent::When {
                    topic: "my lease renewal".into(),
                },
            ),
            (
                "when was the dinner",
                Intent::When {
                    topic: "the dinner".into(),
                },
            ),
            (
                "what day is the offsite",
                Intent::When {
                    topic: "the offsite".into(),
                },
            ),
            (
                "when does the lease expire",
                Intent::When {
                    topic: "the lease".into(),
                },
            ),
            // A bill: answered from extracted invoices first.
            ("when is the invoice due", Intent::Bills { what: None }),
            // Not questions we understand.
            ("quarterly report", Intent::Unknown),
            (
                "why is the sky blue",
                Intent::Passage {
                    topic: "the sky blue".into(),
                },
            ),
            ("who is it", Intent::Unknown),
        ];
        assert!(cases.len() >= 50);
        let mut failures = Vec::new();
        for (q, want) in &cases {
            let got = p(q);
            if &got != want {
                failures.push(format!("{q:?}\n   got  {got:?}\n   want {want:?}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} failed:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }

    fn fl(place: Option<&str>, which: Which, field: Option<&'static str>) -> Intent {
        Intent::Flight {
            place: place.map(String::from),
            which,
            field,
        }
    }
    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    /// Questions about extracted facts, person + topic questions, finding
    /// things, open questions, and Spanish: question → intent and slots.
    #[test]
    fn intent_table_facts_topics_spanish() {
        use Which::*;
        let cases: Vec<(&str, Intent)> = vec![
            // Flights.
            (
                "When is my flight to Lisbon?",
                fl(Some("lisbon"), Next, None),
            ),
            ("my flight to Lisbon", fl(Some("lisbon"), Any, None)),
            ("when's my next flight", fl(None, Next, None)),
            ("whens my next flight", fl(None, Next, None)),
            ("whens my flight to lisbon", fl(Some("lisbon"), Next, None)),
            ("when do I fly to Porto", fl(Some("porto"), Next, None)),
            ("when did I fly to Boston", fl(Some("boston"), Last, None)),
            (
                "what time does my flight to Denver leave",
                fl(Some("denver"), Next, None),
            ),
            (
                "what's my confirmation code for the flight to Lisbon",
                fl(Some("lisbon"), Any, Some("confirmation")),
            ),
            (
                "what is my flight number to Lisbon",
                fl(Some("lisbon"), Any, Some("number")),
            ),
            ("upcoming flights", fl(None, Next, None)),
            ("show me my upcoming flights", fl(None, Next, None)),
            ("my last flight", fl(None, Last, None)),
            ("when do I fly home", fl(None, Next, None)),
            // Stays.
            (
                "where am I staying in Lisbon?",
                Intent::Stay {
                    place: some("lisbon"),
                    which: Next,
                },
            ),
            (
                "where did we stay",
                Intent::Stay {
                    place: None,
                    which: Last,
                },
            ),
            (
                "where did I stay in Porto",
                Intent::Stay {
                    place: some("porto"),
                    which: Last,
                },
            ),
            (
                "my hotel in Porto",
                Intent::Stay {
                    place: some("porto"),
                    which: Any,
                },
            ),
            (
                "when do I check in",
                Intent::Stay {
                    place: None,
                    which: Any,
                },
            ),
            (
                "what's the airbnb in Madrid",
                Intent::Stay {
                    place: some("madrid"),
                    which: Any,
                },
            ),
            // Parcels.
            ("where's my package?", Intent::Package { what: None }),
            (
                "where is my Paperleaf order",
                Intent::Package {
                    what: some("paperleaf"),
                },
            ),
            (
                "tracking for my Paperleaf order",
                Intent::Package {
                    what: some("paperleaf"),
                },
            ),
            (
                "tracking number for my order from Hearth & Loom",
                Intent::Package {
                    what: some("hearth and loom"),
                },
            ),
            (
                "when will my rug arrive",
                Intent::Package { what: some("rug") },
            ),
            (
                "has my Paperleaf order shipped?",
                Intent::Package {
                    what: some("paperleaf"),
                },
            ),
            (
                "what packages are on the way",
                Intent::Package { what: None },
            ),
            // Orders.
            (
                "what did I order from Paperleaf",
                Intent::Orders {
                    merchant: some("paperleaf"),
                },
            ),
            (
                "my last Northfield order",
                Intent::Orders {
                    merchant: some("northfield"),
                },
            ),
            ("my recent orders", Intent::Orders { merchant: None }),
            ("what did I buy recently", Intent::Orders { merchant: None }),
            (
                "order number for my Paperleaf order",
                Intent::Orders {
                    merchant: some("paperleaf"),
                },
            ),
            // Bills.
            (
                "when is my Brightwave bill due?",
                Intent::Bills {
                    what: some("brightwave"),
                },
            ),
            (
                "when is the invoice from Ledgerly due",
                Intent::Bills {
                    what: some("ledgerly"),
                },
            ),
            ("when is my rent due", Intent::Bills { what: some("rent") }),
            (
                "how much is the new rent?",
                Intent::Bills { what: some("rent") },
            ),
            ("what bills are due", Intent::Bills { what: None }),
            ("upcoming bills", Intent::Bills { what: None }),
            (
                "how much is my Brightwave bill",
                Intent::Bills {
                    what: some("brightwave"),
                },
            ),
            (
                "how much do I owe Ledgerly",
                Intent::Bills {
                    what: some("ledgerly"),
                },
            ),
            // Bookings.
            (
                "when is my dinner reservation",
                Intent::Booking {
                    what: Some("dinner".into()),
                },
            ),
            (
                "when is my reservation at Fern & Fig",
                Intent::Booking {
                    what: some("fern and fig"),
                },
            ),
            (
                "my tickets for The Lanterns",
                Intent::Booking {
                    what: some("lanterns"),
                },
            ),
            ("upcoming reservations", Intent::Booking { what: None }),
            // Codes.
            (
                "what's my Rydeo verification code",
                Intent::Code {
                    service: some("rydeo"),
                },
            ),
            (
                "what is the verification code from Rydeo",
                Intent::Code {
                    service: some("rydeo"),
                },
            ),
            ("latest login code", Intent::Code { service: None }),
            (
                "verification code from Rydeo",
                Intent::Code {
                    service: some("rydeo"),
                },
            ),
            // Contact details.
            (
                "what's Priya's phone number",
                Intent::ContactInfo {
                    who: "priya".into(),
                    field: "phone",
                },
            ),
            (
                "Dana's address",
                Intent::ContactInfo {
                    who: "dana".into(),
                    field: "address",
                },
            ),
            (
                "phone number for Omar Haddad",
                Intent::ContactInfo {
                    who: "omar haddad".into(),
                    field: "phone",
                },
            ),
            (
                "how do I reach Priya",
                Intent::ContactInfo {
                    who: "priya".into(),
                    field: "contact",
                },
            ),
            (
                "what is priya's email address",
                Intent::ContactInfo {
                    who: "priya".into(),
                    field: "email",
                },
            ),
            // Replies and what people said.
            (
                "did Priya reply about the contract?",
                Intent::DidReply {
                    who: "priya".into(),
                    topic: "the contract".into(),
                },
            ),
            (
                "has Omar gotten back to me about the budget",
                Intent::DidReply {
                    who: "omar".into(),
                    topic: "the budget".into(),
                },
            ),
            (
                "what did Priya say about pricing?",
                Intent::Said {
                    who: some("priya"),
                    topic: "pricing".into(),
                },
            ),
            (
                "what does Grace think about the offsite",
                Intent::Said {
                    who: some("grace"),
                    topic: "the offsite".into(),
                },
            ),
            (
                "did Mike mention anything about the invoice",
                Intent::Said {
                    who: some("mike"),
                    topic: "the invoice".into(),
                },
            ),
            (
                "what did we decide about the vendor contract",
                Intent::Said {
                    who: None,
                    topic: "the vendor contract".into(),
                },
            ),
            (
                "what was the final decision on the logo",
                Intent::Said {
                    who: None,
                    topic: "the logo".into(),
                },
            ),
            // Finding things.
            (
                "find the contract from Priya",
                Intent::LatestItem {
                    who: some("priya"),
                    kind: Kind::Contract,
                },
            ),
            (
                "find the lease from Dana",
                Intent::Find {
                    who: some("dana"),
                    topic: "lease".into(),
                },
            ),
            (
                "find the offsite agenda",
                Intent::Find {
                    who: None,
                    topic: "offsite agenda".into(),
                },
            ),
            // Open questions.
            (
                "what is the wifi password for the offsite",
                Intent::Passage {
                    topic: "wifi password for the offsite".into(),
                },
            ),
            (
                "how do I reset my Ledgerly password",
                Intent::Passage {
                    topic: "reset my ledgerly password".into(),
                },
            ),
            (
                "why is the sky blue",
                Intent::Passage {
                    topic: "the sky blue".into(),
                },
            ),
            // Counts of extracted things.
            (
                "how many orders did I place with Paperleaf this year",
                Intent::CountFacts {
                    kind: FactKind::Order,
                    who: some("paperleaf"),
                },
            ),
            (
                "how many flights did I take",
                Intent::CountFacts {
                    kind: FactKind::Flight,
                    who: None,
                },
            ),
            (
                "how many packages from Northfield",
                Intent::CountFacts {
                    kind: FactKind::Shipment,
                    who: some("northfield"),
                },
            ),
            // Spend categories go through spend.
            (
                "how much did I spend on flights this year",
                Intent::Spend {
                    merchant: "flights".into(),
                },
            ),
            // Spanish.
            (
                "¿Cuándo fue la última vez que le escribí a Priya?",
                lc("priya", Dir::ToThem),
            ),
            (
                "cuando me escribio Omar por ultima vez",
                lc("omar", Dir::FromThem),
            ),
            (
                "¿Cuánto gasté en Uber este año?",
                Intent::Spend {
                    merchant: "uber".into(),
                },
            ),
            (
                "cuánto he pagado a Ledgerly",
                Intent::Spend {
                    merchant: "ledgerly".into(),
                },
            ),
            (
                "¿Cuántos correos de Priya?",
                count("priya", Dir::FromThem, Kind::Any),
            ),
            ("¿Dónde está mi paquete?", Intent::Package { what: None }),
            (
                "¿Dónde está mi pedido de Paperleaf?",
                Intent::Package {
                    what: some("paperleaf"),
                },
            ),
            (
                "¿Cuándo es mi vuelo a Lisboa?",
                fl(Some("lisboa"), Next, None),
            ),
            ("mi vuelo a Madrid", fl(Some("madrid"), Any, None)),
            (
                "¿Dónde me hospedo en Oporto?",
                Intent::Stay {
                    place: some("oporto"),
                    which: Any,
                },
            ),
            (
                "¿Cuándo vence la factura de Brightwave?",
                Intent::Bills {
                    what: some("brightwave"),
                },
            ),
            ("facturas pendientes", Intent::Bills { what: None }),
            (
                "¿Quién me escribió sobre el contrato?",
                Intent::WhoAbout {
                    topic: "el contrato".into(),
                },
            ),
            (
                "¿Qué dijo Priya sobre los precios?",
                Intent::Said {
                    who: some("priya"),
                    topic: "los precios".into(),
                },
            ),
            (
                "¿Quién es Lucía?",
                Intent::WhoIs {
                    who: "lucia".into(),
                },
            ),
            (
                "¿Cuál es el teléfono de Lucía Ferrer?",
                Intent::ContactInfo {
                    who: "lucia ferrer".into(),
                    field: "phone",
                },
            ),
            (
                "¿Estoy esperando respuesta de Omar?",
                Intent::WaitingOn { who: some("omar") },
            ),
            (
                "¿A quién le debo una respuesta?",
                Intent::OweReplies { who: None },
            ),
            (
                "código de verificación de Rydeo",
                Intent::Code {
                    service: some("rydeo"),
                },
            ),
            (
                "mi último código de verificación de Rydeo",
                Intent::Code {
                    service: some("rydeo"),
                },
            ),
            (
                "¿Cuándo es la cena?",
                Intent::When {
                    topic: "la cena".into(),
                },
            ),
        ];
        assert!(cases.len() >= 80, "{}", cases.len());
        let mut failures = Vec::new();
        for (q, want) in &cases {
            let got = p(q);
            if &got != want {
                failures.push(format!("{q:?}\n   got  {got:?}\n   want {want:?}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} failed:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn spanish_date_scopes() {
        let q = parse("¿Cuánto gasté en Uber este año?", today());
        assert_eq!(q.range_text.as_deref(), Some("this year"));
        let q = parse("cuántos correos de Priya el año pasado", today());
        assert_eq!(q.range.unwrap().from, NaiveDate::from_ymd_opt(2025, 1, 1));
        let q = parse("cuánto gasté en Uber en 2025", today());
        assert_eq!(
            q.intent,
            Intent::Spend {
                merchant: "uber".into()
            }
        );
        assert_eq!(q.range.unwrap().from, NaiveDate::from_ymd_opt(2025, 1, 1));
        let q = parse("quién me escribió sobre el contrato desde marzo", today());
        assert_eq!(q.range.unwrap().from, NaiveDate::from_ymd_opt(2026, 3, 1));
        assert!(looks_like_question("¿dónde está mi paquete?"));
        assert!(looks_like_question("my flight to lisbon"));
    }

    #[test]
    fn date_scope() {
        let q = parse("how many emails from Priya this year", today());
        assert_eq!(q.range_text.as_deref(), Some("this year"));
        let r = q.range.unwrap();
        assert_eq!(r.from, NaiveDate::from_ymd_opt(2026, 1, 1));
        assert_eq!(r.to, NaiveDate::from_ymd_opt(2027, 1, 1));

        let q = parse("how much did I spend on uber date:2025", today());
        assert_eq!(
            q.intent,
            Intent::Spend {
                merchant: "uber".into()
            }
        );
        assert_eq!(q.range.unwrap().from, NaiveDate::from_ymd_opt(2025, 1, 1));

        let q = parse("how much did I spend on uber during 2025?", today());
        assert_eq!(
            q.intent,
            Intent::Spend {
                merchant: "uber".into()
            }
        );
        assert_eq!(q.range_text.as_deref(), Some("2025"));

        let q = parse("who emailed me about pricing since march", today());
        assert_eq!(
            q.intent,
            Intent::WhoAbout {
                topic: "pricing".into()
            }
        );
        assert_eq!(q.range.unwrap().from, NaiveDate::from_ymd_opt(2026, 3, 1));

        // A lone month name stays a name.
        let q = parse("when did I last email may", today());
        assert_eq!(q.intent, lc("may", Dir::ToThem));
        assert!(q.range.is_none());
    }

    #[test]
    fn normalizes() {
        assert_eq!(
            normalize("What's Priya's email?"),
            vec!["what", "is", "priya", "'s", "email"]
        );
        assert_eq!(
            normalize("Mike from KOTK/Fernwood"),
            vec!["mike", "from", "kotk", "/", "fernwood"]
        );
        assert_eq!(
            normalize("hey penguin, when did I last email sam?"),
            vec!["when", "did", "i", "last", "email", "sam"]
        );
        assert_eq!(normalize("emails on 9/24"), vec!["emails", "on", "9/24"]);
    }

    #[test]
    fn question_detection() {
        assert!(looks_like_question("? mike"));
        assert!(looks_like_question("when did I last email Mike"));
        assert!(looks_like_question("how much did I spend on Uber"));
        assert!(looks_like_question("who is priya"));
        assert!(!looks_like_question("invoice from:mike"));
        assert!(!looks_like_question("when"));
        assert!(!looks_like_question("quarterly report"));
        // Totals, first emails and contact details are questions too.
        for q in [
            "total cost of my linear receipts this year",
            "sum of my Linear invoices",
            "add up my uber receipts",
            "Linear total this year",
            "first email with Priya",
            "oldest email from Dana",
            "their email",
            "Priya's email",
            "Dana Whitfield's phone number",
        ] {
            assert!(looks_like_question(q), "{q}");
        }
        assert!(!looks_like_question("invoice total"));
        assert!(!looks_like_question("priya's deck"));
    }
}
