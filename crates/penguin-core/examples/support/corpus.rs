//! Deterministic synthetic mailbox for benchmarks: fictional people at
//! `.example` domains, Zipf-distributed vocabulary, threads with quoted
//! replies, newsletters with big HTML, attachments, labels, 3 accounts and
//! 8 years of dates (volume growing toward the present).
//!
//! Shared by several examples via `#[path]`; each compiles its own copy and
//! uses a subset of it, so unused items are expected per example.
#![allow(dead_code)]

use penguin_core::{Address, AttachmentMeta, Message};

pub const ACCOUNTS: [&str; 3] = [
    "alex@work.example",
    "alex@personal.example",
    "alex@side.example",
];
pub const DAY: i64 = 86_400_000;

/// splitmix64: tiny, fast, deterministic.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn chance(&mut self, p: f64) -> bool {
        self.f() < p
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

const FIRST: &[&str] = &[
    "Mike",
    "Ana",
    "Priya",
    "Bo",
    "Sam",
    "Samantha",
    "Liam",
    "Olivia",
    "Noah",
    "Emma",
    "Mateo",
    "Sofia",
    "Ethan",
    "Mia",
    "Lucas",
    "Isabella",
    "Aiden",
    "Chloe",
    "Diego",
    "Zoe",
    "Hiro",
    "Yuki",
    "Omar",
    "Leila",
    "Ravi",
    "Anika",
    "Kofi",
    "Ama",
    "Lars",
    "Ingrid",
    "Tomas",
    "Elena",
    "Marco",
    "Giulia",
    "Pierre",
    "Camille",
    "Jonas",
    "Lena",
    "Rafael",
    "Beatriz",
    "Wei",
    "Mei",
    "Jin",
    "Soo",
    "Ari",
    "Noa",
    "Dmitri",
    "Katya",
    "Sean",
    "Aoife",
    "Grace",
    "Henry",
    "Jack",
    "Ruby",
    "Leo",
    "Nora",
    "Felix",
    "Iris",
    "Hugo",
    "Clara",
    "Theo",
    "Maya",
    "Owen",
    "Ivy",
    "Caleb",
    "Luna",
    "Isaac",
    "Stella",
    "Julian",
    "Hazel",
    "Miles",
    "Violet",
    "Ezra",
    "Aurora",
    "Kai",
    "Nina",
    "Rosa",
    "Pablo",
    "Carmen",
    "José",
    "Zoë",
    "Renée",
    "François",
    "Søren",
];
const LAST: &[&str] = &[
    "Delgado",
    "Ruiz",
    "Nair",
    "Chen",
    "Ortiz",
    "Lee",
    "Smith",
    "Johnson",
    "Garcia",
    "Martinez",
    "Kim",
    "Nguyen",
    "Patel",
    "Singh",
    "Khan",
    "Ali",
    "Okafor",
    "Mensah",
    "Larsen",
    "Berg",
    "Novak",
    "Rossi",
    "Bianchi",
    "Dubois",
    "Moreau",
    "Schmidt",
    "Weber",
    "Silva",
    "Santos",
    "Wang",
    "Li",
    "Zhang",
    "Tanaka",
    "Sato",
    "Park",
    "Choi",
    "Cohen",
    "Levi",
    "Ivanov",
    "Petrova",
    "Murphy",
    "Kelly",
    "Walker",
    "Hall",
    "Young",
    "King",
    "Wright",
    "Scott",
    "Green",
    "Baker",
    "Adams",
    "Nelson",
    "Carter",
    "Mitchell",
    "Perez",
    "Roberts",
    "Turner",
    "Phillips",
    "Campbell",
    "Parker",
    "Evans",
    "Edwards",
    "Collins",
    "Stewart",
    "Morris",
    "Rogers",
    "Reed",
    "Cook",
    "Morgan",
    "Bell",
    "Cooper",
    "Richardson",
    "Cox",
    "Howard",
    "Ward",
    "Torres",
    "Peterson",
];
const DOMAINS: &[&str] = &[
    "acme.example",
    "globex.example",
    "initech.example",
    "umbrella.example",
    "hooli.example",
    "piedpiper.example",
    "stark.example",
    "wayne.example",
    "wonka.example",
    "cyberdyne.example",
    "tyrell.example",
    "soylent.example",
    "vandelay.example",
    "dundermifflin.example",
    "gringotts.example",
    "gmail.example",
    "outlook.example",
    "icloud.example",
    "fastmail.example",
    "proton.example",
    "lawfirm.example",
    "clinic.example",
    "school.example",
    "realty.example",
    "bank.example",
    "insurance.example",
    "cpa.example",
    "agency.example",
    "studio.example",
    "ventures.example",
];
const NEWS: &[(&str, &str)] = &[
    ("The Weekly Dispatch", "news@dispatch.example"),
    ("Morning Brew Digest", "hello@brew.example"),
    ("Product Hunt Daily", "daily@hunt.example"),
    ("GitHub", "noreply@git.example"),
    ("Linear", "notifications@linear.example"),
    ("Stripe", "receipts@stripe.example"),
    ("Amazon", "ship-confirm@shop.example"),
    ("Delta", "deltaairlines@air.example"),
    ("Uber Receipts", "uber.us@ride.example"),
    ("Chase", "no.reply.alerts@bank.example"),
    ("Google Calendar", "calendar-notification@cal.example"),
    ("Slack", "feedback@slack.example"),
    ("LinkedIn", "messages-noreply@linkedin.example"),
    ("Medium Daily Digest", "noreply@medium.example"),
    ("Substack", "newsletter@substack.example"),
    ("Zillow", "alerts@homes.example"),
    ("Figma", "no-reply@figma.example"),
    ("Notion", "notify@notion.example"),
    ("Vercel", "notifications@vercel.example"),
    ("AWS", "no-reply@aws.example"),
];

const COMMON: &str = "the of and to a in is it you that he was for on are with as I his they be at one have this from or had by hot word but what some we can out other were all there when up use your how said an each she which do their time if will way about many then them write would like so these her long make thing see him two has look more day could go come did number sound no most people my over know water than call first who may down side been now find any new work part take get place made live where after back little only round man year came show every good me give our under name very through just form sentence great think say help low line differ turn cause much mean before move right boy old too same tell does set three want air well also play small end put home read hand port large spell add even land here must big high such follow act why ask men change went light kind off need house picture try us again animal point mother world near build self earth father head stand own page should country found answer school grow study still learn plant cover food sun four between state keep eye never last let thought city tree cross farm hard start might story saw far sea draw left late run while press close night real life few north open seem together next white children begin got walk example ease paper group always music those both mark often letter until mile river car feet care second book carry took science eat room friend began idea fish mountain stop once base hear horse cut sure watch color face wood main enough plain girl usual young ready above ever red list though feel talk bird soon body dog family direct pose leave song measure door product black short numeral class wind question happen complete ship area half rock order fire south problem piece told knew pass since top whole king space heard best hour better true during hundred five remember step early hold west ground interest reach fast verb sing listen six table travel less morning ten simple several vowel toward war lay against pattern slow center love person money serve appear road map rain rule govern pull cold notice voice unit power town fine certain fly fall lead cry dark machine note wait plan figure star box noun field rest correct able pound done beauty drive stood contain front teach week final gave green oh quick develop ocean warm free minute strong special mind behind clear tail produce fact street inch multiply nothing course stay wheel full force blue object decide surface deep moon island foot system busy test record boat common gold possible plane stead dry wonder laugh thousand ago ran check game shape equate miss brought heat snow tire bring yes distant fill east paint language among meeting invoice contract lease renewal budget quarterly report deadline schedule agenda proposal review draft approval payment receipt shipping order account password security update release deploy launch customer client project roadmap feedback design hiring offer interview onboarding benefits insurance tax refund statement transfer deposit balance subscription renewal trial upgrade discount sale coupon event webinar conference flight hotel reservation itinerary boarding confirmation dinner lunch coffee weekend vacation birthday wedding party kids soccer practice doctor appointment dentist prescription vet grooming mortgage closing inspection appraisal escrow landlord tenant repair plumber electrician estimate quote warranty return exchange pickup delivery tracking package";

fn syllable_word(rng: &mut Rng) -> String {
    const ON: &[&str] = &[
        "b", "c", "d", "f", "g", "h", "j", "k", "l", "m", "n", "p", "r", "s", "t", "v", "w", "z",
        "br", "cr", "st", "tr", "pl", "gr", "sh", "ch", "th",
    ];
    const NU: &[&str] = &["a", "e", "i", "o", "u", "ai", "ea", "ou", "io", "y"];
    const CO: &[&str] = &[
        "", "n", "r", "s", "t", "l", "m", "x", "nd", "rk", "st", "ng",
    ];
    let n = 2 + rng.below(3);
    let mut w = String::new();
    for _ in 0..n {
        w.push_str(rng.pick(ON));
        w.push_str(rng.pick(NU));
    }
    w.push_str(rng.pick(CO));
    w
}

pub struct Person {
    pub name: String,
    pub email: String,
}

pub struct Corpus {
    rng: Rng,
    vocab: Vec<String>,
    /// cumulative Zipf weights over vocab
    cdf: Vec<f64>,
    pub people: Vec<Person>,
    now: i64,
    next_id: u64,
}

impl Corpus {
    pub fn new(seed: u64, now: i64) -> Corpus {
        let mut rng = Rng::new(seed);
        let mut vocab: Vec<String> = COMMON.split_whitespace().map(str::to_string).collect();
        let mut seen: std::collections::HashSet<String> = vocab.iter().cloned().collect();
        while vocab.len() < 40_000 {
            let w = syllable_word(&mut rng);
            if seen.insert(w.clone()) {
                vocab.push(w);
            }
        }
        // Zipf s=1.07 over ranks.
        let mut acc = 0.0;
        let cdf = (0..vocab.len())
            .map(|r| {
                acc += 1.0 / ((r + 1) as f64).powf(1.07);
                acc
            })
            .collect();
        let mut people = Vec::new();
        for i in 0..4000 {
            let first = *rng.pick(FIRST);
            let last = *rng.pick(LAST);
            let dom = *rng.pick(DOMAINS);
            let local = match i % 3 {
                0 => format!("{}.{}", first.to_lowercase(), last.to_lowercase()),
                1 => format!("{}{}", &first.to_lowercase()[..1], last.to_lowercase()),
                _ => format!("{}{}", first.to_lowercase(), i),
            };
            let local: String = local
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
                .collect();
            people.push(Person {
                name: format!("{first} {last}"),
                email: format!("{local}@{dom}"),
            });
        }
        Corpus {
            rng,
            vocab,
            cdf,
            people,
            now,
            next_id: 1,
        }
    }

    fn word(&mut self) -> String {
        let total = *self.cdf.last().unwrap();
        let x = self.rng.f() * total;
        let i = self
            .cdf
            .partition_point(|&c| c < x)
            .min(self.vocab.len() - 1);
        self.vocab[i].clone()
    }

    fn sentence(&mut self, out: &mut String) {
        let n = 6 + self.rng.below(14);
        for i in 0..n {
            let w = self.word();
            if i == 0 {
                let mut c = w.chars();
                out.push_str(&c.next().unwrap().to_uppercase().collect::<String>());
                out.push_str(c.as_str());
            } else {
                out.push(' ');
                out.push_str(&w);
            }
        }
        out.push_str(if self.rng.chance(0.1) { "?" } else { "." });
    }

    fn paragraphs(&mut self, words: usize) -> String {
        let mut out = String::with_capacity(words * 7);
        let mut count = 0;
        while count < words {
            let before = out.len();
            self.sentence(&mut out);
            count += out[before..].split_whitespace().count();
            out.push(if self.rng.chance(0.25) { '\n' } else { ' ' });
            if self.rng.chance(0.08) {
                out.push('\n');
            }
        }
        out
    }

    /// Message date: 8 years, density rising toward now.
    fn date(&mut self) -> i64 {
        let span = 8.0 * 365.0 * DAY as f64;
        let back = span * (1.0 - self.rng.f().powf(0.6));
        self.now - back as i64 - self.rng.below(1000) as i64
    }

    fn id(&mut self) -> String {
        self.next_id += 1;
        format!("{:016x}", self.next_id * 2654435761)
    }

    fn attachment(&mut self) -> AttachmentMeta {
        let r = self.rng.below(10);
        let (filename, mime) = match r {
            0..=2 => (
                format!("Invoice_INV-{}.pdf", 10000 + self.rng.below(89999)),
                "application/pdf",
            ),
            3 => (
                format!("{}_contract_{}.pdf", self.word(), 2018 + self.rng.below(9)),
                "application/pdf",
            ),
            4 | 5 => (
                format!("IMG_{}.jpg", 1000 + self.rng.below(8999)),
                "image/jpeg",
            ),
            6 => (
                format!("{}-{}-budget.xlsx", self.word(), self.word()),
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            7 => (
                format!("{} notes.docx", self.word()),
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            8 => (
                format!("Q{}-deck.pptx", 1 + self.rng.below(4)),
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            ),
            _ => (
                format!("screenshot-{}.png", self.rng.below(100000)),
                "image/png",
            ),
        };
        AttachmentMeta {
            id: format!("att-{}", self.rng.next() % 1_000_000_000),
            filename,
            mime_type: mime.into(),
            size: 10_000 + self.rng.below(5_000_000) as u64,
            content_id: None,
            inline: false,
        }
    }

    fn html_wrap(&mut self, text: &str) -> String {
        let mut h = String::with_capacity(text.len() * 4);
        h.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>body{font-family:Helvetica,Arial,sans-serif}.btn{background:#0a84ff;color:#fff;padding:12px 24px;border-radius:6px}td{padding:0 12px}</style></head><body><table width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" role=\"presentation\"><tr><td align=\"center\"><table width=\"600\">");
        for p in text.split('\n').filter(|p| !p.trim().is_empty()) {
            h.push_str("<tr><td style=\"font-size:16px;line-height:24px;color:#333333;padding:8px 24px\"><p style=\"margin:0\">");
            h.push_str(&p.replace('&', "&amp;").replace('<', "&lt;"));
            h.push_str("</p></td></tr>");
        }
        h.push_str("<tr><td><a class=\"btn\" href=\"https://track.example/c?u=abcdef0123456789&amp;id=42\">Read more</a><img src=\"https://track.example/o.gif?id=42\" width=\"1\" height=\"1\"></td></tr></table></td></tr></table></body></html>");
        h
    }

    /// Marketing-style HTML like real newsletters (typically 20-60 KB): a big
    /// <style> block, nested layout tables, verbose inline styles, image rows
    /// and per-recipient tracking links (unique tokens compress poorly).
    fn marketing_html(&mut self, text: &str) -> String {
        let mut h = String::with_capacity(40_000);
        h.push_str("<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\"><html xmlns=\"http://www.w3.org/1999/xhtml\"><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\"><title></title><style type=\"text/css\">");
        for i in 0..40 {
            h.push_str(&format!(".c{i}{{font-family:'Helvetica Neue',Helvetica,Arial,sans-serif;font-size:{}px;line-height:{}px;color:#{:06x};padding:{}px {}px;mso-line-height-rule:exactly}}@media only screen and (max-width:600px){{.m{i}{{width:100%!important;padding:0 {}px!important}}}}", 12 + i % 8, 18 + i % 10, self.rng.next() & 0xffffff, i % 16, 8 + i % 24, i % 20));
        }
        h.push_str("</style></head><body style=\"margin:0;padding:0;background-color:#f4f4f4\"><div style=\"display:none;max-height:0;overflow:hidden\">");
        h.push_str(&"&#847;&zwnj;&nbsp;".repeat(60));
        h.push_str("</div><center><table role=\"presentation\" border=\"0\" cellpadding=\"0\" cellspacing=\"0\" width=\"100%\" style=\"border-collapse:collapse;mso-table-lspace:0pt;mso-table-rspace:0pt\"><tr><td align=\"center\" valign=\"top\"><table role=\"presentation\" border=\"0\" cellpadding=\"0\" cellspacing=\"0\" width=\"600\" class=\"m1\" style=\"max-width:600px;background:#ffffff\">");
        for (i, p) in text
            .split('\n')
            .filter(|p| !p.trim().is_empty())
            .enumerate()
        {
            let tok = self.rng.next();
            if i % 3 == 0 {
                h.push_str(&format!("<tr><td align=\"center\" style=\"padding:0\"><a href=\"https://click.track.example/ls/click?upn={tok:016x}{:016x}-2F{:016x}\" target=\"_blank\" style=\"text-decoration:none\"><img src=\"https://cdn.track.example/img/{:016x}.jpg\" width=\"600\" height=\"300\" alt=\"\" border=\"0\" style=\"display:block;width:100%;max-width:600px;height:auto;border:0;outline:none\"></a></td></tr>", self.rng.next(), self.rng.next(), self.rng.next()));
            }
            h.push_str(&format!("<tr><td class=\"c{} m{}\" style=\"font-family:'Helvetica Neue',Helvetica,Arial,sans-serif;font-size:16px;line-height:26px;color:#333333;padding:12px 32px;text-align:left;mso-line-height-rule:exactly\"><p style=\"margin:0 0 12px 0\"><span style=\"font-size:16px;color:#333333\">", i % 40, i % 40));
            h.push_str(&p.replace('&', "&amp;").replace('<', "&lt;"));
            h.push_str(&format!("</span></p><a href=\"https://click.track.example/ls/click?upn={tok:016x}-3D{:016x}\" style=\"color:#0a84ff;font-weight:bold;text-decoration:underline\">Read more &rarr;</a></td></tr>", self.rng.next()));
        }
        h.push_str("<tr><td style=\"padding:24px 32px;font-size:12px;line-height:18px;color:#888888;font-family:Arial,sans-serif\">");
        for _ in 0..6 {
            h.push_str(&format!("<a href=\"https://click.track.example/ls/click?upn={:016x}{:016x}\" style=\"color:#888888\">Link</a> &middot; ", self.rng.next(), self.rng.next()));
        }
        h.push_str("You are receiving this email because you subscribed. 123 Example Street, Suite 400, Springfield. <a href=\"https://click.track.example/unsub\" style=\"color:#888888\">Unsubscribe</a> | <a href=\"https://click.track.example/prefs\" style=\"color:#888888\">Manage preferences</a></td></tr></table></td></tr></table></center>");
        h.push_str(&format!("<img src=\"https://open.track.example/o/{:016x}{:016x}.gif\" width=\"1\" height=\"1\" alt=\"\" style=\"display:none\"></body></html>", self.rng.next(), self.rng.next()));
        h
    }

    /// One thread (1..n messages). Returns messages newest last.
    pub fn thread(&mut self, account_ix: usize) -> Vec<Message> {
        let account = ACCOUNTS[account_ix];
        let thread_id = self.id();
        let kind = self.rng.f();
        let start = self.date();
        let recent = self.now - start < 21 * DAY;
        if kind < 0.45 {
            // Newsletter / notification: one message, long HTML.
            let (name, email) = *self.rng.pick(NEWS);
            let words = 150 + self.rng.below(900);
            let body = self.paragraphs(words);
            let html = self.marketing_html(&body);
            let subject = format!("{} {} {}", self.word(), self.word(), self.word());
            let cat = *self.rng.pick(&[
                "CATEGORY_PROMOTIONS",
                "CATEGORY_UPDATES",
                "CATEGORY_SOCIAL",
                "CATEGORY_FORUMS",
            ]);
            let mut labels = vec![cat.to_string()];
            if recent {
                labels.push("INBOX".into());
                if self.rng.chance(0.4) {
                    labels.push("UNREAD".into());
                }
            }
            let mut attachments = Vec::new();
            if name == "Stripe" || name == "Uber Receipts" || self.rng.chance(0.03) {
                attachments.push(self.attachment());
            }
            attachments.push(AttachmentMeta {
                id: "logo".into(),
                filename: "logo.png".into(),
                mime_type: "image/png".into(),
                size: 4000,
                content_id: Some("logo".into()),
                inline: true,
            });
            let snippet: String = body.chars().take(140).collect();
            return vec![Message {
                account_id: account.into(),
                id: self.id(),
                thread_id,
                date: start,
                from: Address {
                    name: Some(name.into()),
                    email: email.into(),
                },
                to: vec![Address {
                    name: Some("Alex".into()),
                    email: account.into(),
                }],
                cc: vec![],
                bcc: vec![],
                reply_to: vec![],
                subject,
                snippet,
                body_text: penguin_core::text::html_to_text(&html),
                body_html: Some(html),
                label_ids: labels,
                attachments,
                message_id_header: None,
                in_reply_to: None,
                references: vec![],
                sender_authenticated: false,
                list_unsubscribe: Some(format!(
                    "<mailto:unsub@{}>",
                    email.split('@').nth(1).unwrap()
                )),
                list_unsubscribe_post: Some(false),
            }];
        }
        // Conversation with 1-3 people, 1..8 messages, alternating incl. you.
        let n_people = 1 + self.rng.below(3);
        let people: Vec<usize> = (0..n_people).map(|_| self.power_person()).collect();
        let len = 1 + (self.rng.f().powf(2.2) * 8.0) as usize;
        let subject_base = format!("{} {} {}", self.word(), self.word(), self.word());
        let starred = self.rng.chance(0.02);
        let important = self.rng.chance(0.3);
        let user_label = if self.rng.chance(0.15) {
            Some(format!("Label_{}", 1 + self.rng.below(20)))
        } else {
            None
        };
        let mut out: Vec<Message> = Vec::new();
        let mut date = start;
        let mut history = String::new();
        for i in 0..len {
            let from_me = i % 2 == 1 || (i == 0 && self.rng.chance(0.15));
            let other = &self.people[people[self.rng.below(people.len())]];
            let (from, to) = if from_me {
                (
                    Address {
                        name: Some("Alex".into()),
                        email: account.into(),
                    },
                    Address {
                        name: Some(other.name.clone()),
                        email: other.email.clone(),
                    },
                )
            } else {
                (
                    Address {
                        name: Some(other.name.clone()),
                        email: other.email.clone(),
                    },
                    Address {
                        name: Some("Alex".into()),
                        email: account.into(),
                    },
                )
            };
            let words = 20 + self.rng.below(220);
            let mut body = self.paragraphs(words);
            if self.rng.chance(0.05) {
                body.push_str(&format!(
                    " Reference number INV-{} and order #{}.",
                    10000 + self.rng.below(89999),
                    100000 + self.rng.below(899999)
                ));
            }
            let authored = body.clone();
            if !history.is_empty() {
                body.push_str(&format!(
                    "\n\nOn Mon, Mar 2, 2026 at 10:15 AM {} <{}> wrote:\n",
                    to.name.clone().unwrap_or_default(),
                    to.email
                ));
                for line in history.lines().take(60) {
                    body.push_str("> ");
                    body.push_str(line);
                    body.push('\n');
                }
            }
            history = authored;
            let mut labels = Vec::new();
            if from_me {
                labels.push("SENT".to_string());
            } else if recent || self.rng.chance(0.01) {
                labels.push("INBOX".into());
                if self.rng.chance(0.3) {
                    labels.push("UNREAD".into());
                }
            }
            if important {
                labels.push("IMPORTANT".into());
            }
            if starred && i == 0 {
                labels.push("STARRED".into());
            }
            if let Some(l) = &user_label {
                labels.push(l.clone());
            }
            if self.rng.chance(0.004) {
                labels = vec!["TRASH".into()];
            }
            let attachments = if self.rng.chance(0.12) {
                (0..1 + self.rng.below(2))
                    .map(|_| self.attachment())
                    .collect()
            } else {
                vec![]
            };
            let snippet: String = body.chars().take(140).collect();
            let html = if self.rng.chance(0.6) {
                Some(self.html_wrap(&body))
            } else {
                None
            };
            out.push(Message {
                account_id: account.into(),
                id: self.id(),
                thread_id: thread_id.clone(),
                date,
                from,
                to: vec![to],
                cc: if self.rng.chance(0.2) {
                    let ix = self.power_person();
                    let p = &self.people[ix];
                    vec![Address {
                        name: Some(p.name.clone()),
                        email: p.email.clone(),
                    }]
                } else {
                    vec![]
                },
                bcc: vec![],
                reply_to: vec![],
                subject: if i == 0 {
                    subject_base.clone()
                } else {
                    format!("Re: {subject_base}")
                },
                snippet,
                body_text: body,
                body_html: html,
                label_ids: labels,
                attachments,
                message_id_header: Some(format!("<{}@mail.example>", self.rng.next())),
                in_reply_to: None,
                references: vec![],
                sender_authenticated: false,
                list_unsubscribe: None,
                list_unsubscribe_post: None,
            });
            date += (self.rng.f() * 2.0 * DAY as f64) as i64 + 60_000;
            if date > self.now {
                break;
            }
        }
        out
    }

    /// People follow a power law too: a few correspondents dominate.
    fn power_person(&mut self) -> usize {
        ((self.rng.f().powf(3.0)) * self.people.len() as f64) as usize % self.people.len()
    }

    /// Hand-written known items the benchmark queries look for.
    pub fn planted(&mut self) -> Vec<Message> {
        let acct = ACCOUNTS[0];
        let mk = |id: &str,
                  date: i64,
                  from: (&str, &str),
                  subject: &str,
                  body: &str,
                  att: Option<(&str, &str)>| Message {
            account_id: acct.into(),
            id: id.into(),
            thread_id: format!("t-{id}"),
            date,
            from: Address {
                name: Some(from.0.into()),
                email: from.1.into(),
            },
            to: vec![Address {
                name: Some("Alex".into()),
                email: acct.into(),
            }],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: subject.into(),
            snippet: body.chars().take(100).collect(),
            body_text: body.into(),
            body_html: None,
            label_ids: vec!["IMPORTANT".into()],
            attachments: att
                .map(|(f, m)| {
                    vec![AttachmentMeta {
                        id: "a1".into(),
                        filename: f.into(),
                        mime_type: m.into(),
                        size: 250_000,
                        content_id: None,
                        inline: false,
                    }]
                })
                .unwrap_or_default(),
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: false,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        };
        let now = self.now;
        vec![
            mk("planted-lease", now - 180 * DAY, ("Mike Delgado", "mike.delgado@realty.example"), "Lease renewal for the Elm St unit",
               "Hi Alex, attached is the lease renewal. The early termination clause is on page 3. Let me know by Friday.", Some(("Elm_St_Lease_Renewal_2026.pdf", "application/pdf"))),
            mk("planted-invoice", now - 400 * DAY, ("Billing", "billing@cpa.example"), "Your invoice INV-20417",
               "Invoice INV-20417 for tax preparation services is attached. Payment due in 30 days.", Some(("INV-20417.pdf", "application/pdf"))),
            mk("planted-wifi", now - 30 * DAY, ("Ana Ruiz", "ana.ruiz@gmail.example"), "cabin wifi",
               "The wifi password for the cabin is correct-horse-battery. The router is behind the couch.", None),
        ]
    }
}
