//! Procedural background mail: the realistic bulk that planted targets have
//! to be found in. Each family has its own random stream (`Rng::fork`), so
//! changing one family's size leaves the others' mail unchanged.
//!
//! Text comes from phrase banks with several wordings per idea, so two
//! messages about the same thing rarely share every word (paraphrase
//! variety), and distractors look like the targets (other flights, other
//! invoices, other Mikes).

use penguin_core::Address;

use super::world::*;
use super::{fmt_local, me, to_html, Att, Corpus, Msg, Thread};
use crate::rng::Rng;
use crate::window::DAY;

pub fn add(c: &mut Corpus) {
    let seed = c.seed;
    work(c, &mut Rng::fork(seed, "work"), 1450);
    personal(c, &mut Rng::fork(seed, "personal"), 430);
    spanish(c, &mut Rng::fork(seed, "spanish"), 190);
    studio(c, &mut Rng::fork(seed, "studio"), 340);
    newsletters(c, &mut Rng::fork(seed, "newsletters"), 1000);
    promotions(c, &mut Rng::fork(seed, "promos"), 420);
    let orders = receipts(c, &mut Rng::fork(seed, "receipts"), 620);
    shipping(c, &mut Rng::fork(seed, "shipping"), &orders);
    bills(c, &mut Rng::fork(seed, "bills"));
    invoices(c, &mut Rng::fork(seed, "invoices"), 90);
    travel(c, &mut Rng::fork(seed, "travel"), 95);
    invites(c, &mut Rng::fork(seed, "invites"), 470);
    codes(c, &mut Rng::fork(seed, "codes"), 460);
    social(c, &mut Rng::fork(seed, "social"), 430);
}

/// `{key}` substitution.
pub fn fill(t: &str, vars: &[(&str, &str)]) -> String {
    let mut s = t.to_string();
    for (k, v) in vars {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// Days ago, skewed toward the present (mail volume grows over time).
fn skew(rng: &mut Rng, max_days: i64) -> i64 {
    (rng.f().powf(1.7) * max_days as f64) as i64 + 1
}

fn money(rng: &mut Rng, lo: i64, hi: i64) -> String {
    format!("{}.{:02}", rng.range(lo, hi), rng.below(100))
}

/// `n.0 + below(n.1)` distinct sentences from `bank`.
fn sentence_pick(rng: &mut Rng, bank: &[&str], n: (usize, usize), vars: &[(&str, &str)]) -> String {
    let n = n.0 + rng.below(n.1);
    let mut used = Vec::new();
    let mut out = Vec::new();
    while out.len() < n.min(bank.len()) {
        let i = rng.below(bank.len());
        if used.contains(&i) {
            continue;
        }
        used.push(i);
        out.push(fill(bank[i], vars));
    }
    out.join(" ")
}

const MONTHS: &[&str] = &[
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: &[&str] = &["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"];

// ---------------------------------------------------------------- work ----

const PROJECTS: &[&str] = &[
    "Atlas",
    "Beacon",
    "Cobalt",
    "Driftwood",
    "Ember",
    "Falcon",
    "Granite",
    "Juniper",
];

/// (topic key, subjects, body sentences). `{p}` project, `{d}` weekday,
/// `{m}` month, `{n}` a number, `{who}` a colleague's first name.
const WORK_TOPICS: &[(&str, &[&str], &[&str])] = &[
    (
        "budget",
        &[
            "{p} budget for next quarter",
            "Q{q} spend review",
            "Forecast update: {p}",
            "Budget variance in {m}",
            "Headcount cost for {p}",
        ],
        &[
            "I pulled the numbers for {p} and we're about {n}% over the forecast.",
            "Can you review the spend line items before {d}?",
            "Finance wants the revised forecast by end of month.",
            "The variance is mostly contractor hours and the new staging cluster.",
            "Let's hold discretionary spend until the quarterly review.",
            "I moved two line items from tooling to headcount.",
            "Kwame asked for a one-page summary of where the money goes.",
            "We have room for one more contractor if we trim the conference budget.",
        ],
    ),
    (
        "launch",
        &[
            "{p} launch checklist",
            "Ship date for {p}",
            "{p} rollout plan",
            "Go/no-go for {p} on {d}",
            "Release notes draft: {p} {n}.0",
        ],
        &[
            "We're on track to ship {p} on {d} if QA signs off.",
            "The rollout starts at 10% and ramps over a week.",
            "Marketing needs the release notes by {d}.",
            "Two blockers left on the checklist, both in review.",
            "Can we freeze the branch on {d} evening?",
            "Support wants a heads-up before the release goes out.",
            "Feature flags are set up for the staged rollout.",
            "Let's do the go/no-go call at 11.",
        ],
    ),
    (
        "hiring",
        &[
            "Interview loop for the {r} role",
            "Candidate feedback: {r}",
            "Offer approval for {r}",
            "Scheduling onsite interviews",
            "Referral for the {r} opening",
        ],
        &[
            "Please add your interview feedback in the scorecard by {d}.",
            "The candidate did well on the system design round.",
            "Lena is coordinating the onsite schedule.",
            "We need one more interviewer for the culture round.",
            "The offer is approved pending the comp band check.",
            "I'd lean hire, with some concerns about ownership.",
            "Can you do a 30 minute call with the candidate on {d}?",
        ],
    ),
    (
        "incident",
        &[
            "Postmortem: {p} outage on {d}",
            "Incident {n}: elevated error rate",
            "Root cause for the {p} downtime",
            "Pager handoff notes",
            "Follow-ups from the {p} incident",
        ],
        &[
            "Error rates spiked to {n}% for about twenty minutes.",
            "Root cause was an expired certificate on the load balancer.",
            "I'll write up the postmortem and circulate it by {d}.",
            "Action items are in the tracker, owners assigned.",
            "Paging worked, but the runbook was out of date.",
            "We rolled back the deploy and error rates recovered.",
            "Customers in the EU region were affected the most.",
        ],
    ),
    (
        "design",
        &[
            "{p} mockups v{n}",
            "Design review: {p} onboarding",
            "Feedback on the {p} prototype",
            "Icons and spacing for {p}",
            "Updated flows for {p} settings",
        ],
        &[
            "I uploaded the new mockups, the empty states are still rough.",
            "Can we tighten the spacing on the settings screen?",
            "Hannah wants to run the prototype past five users.",
            "The onboarding flow drops from six steps to four.",
            "I prefer option B for the navigation.",
            "Dark mode colors need another pass for contrast.",
            "Let's review the flows together on {d}.",
        ],
    ),
    (
        "security",
        &[
            "Access review for {m}",
            "SSO rollout for {p}",
            "Pen test findings",
            "Rotate the {p} API keys",
            "Security questionnaire from {cust}",
        ],
        &[
            "Please confirm the access list for your team by {d}.",
            "The pen test found two medium issues and no criticals.",
            "We need to rotate the keys before the audit.",
            "Yuki is running the access review this quarter.",
            "SSO is enforced for everyone starting {d}.",
            "The questionnaire asks about encryption at rest and backups.",
        ],
    ),
    (
        "planning",
        &[
            "Q{q} roadmap draft",
            "Priorities for {p} next quarter",
            "OKR check-in",
            "Planning offsite agenda",
            "Tradeoffs for the {p} roadmap",
        ],
        &[
            "Here is the first draft of the roadmap, comments welcome.",
            "We can't do both the migration and the redesign this quarter.",
            "Let's rank the top five bets by impact.",
            "The OKRs look ambitious but doable.",
            "Omar wants a single slide per team.",
            "I moved the reporting work to next quarter.",
        ],
    ),
    (
        "customer",
        &[
            "{cust} renewal",
            "Escalation from {cust}",
            "{cust} contract questions",
            "Call notes: {cust}",
            "{cust} feature request",
        ],
        &[
            "{cust} wants to renew but asked for a volume discount.",
            "They hit the rate limit twice this week and are frustrated.",
            "Diego is setting up a call with their team on {d}.",
            "The contract renews at the end of {m}.",
            "They asked whether we support SAML and audit logs.",
            "I promised an update on the export bug by {d}.",
        ],
    ),
    (
        "vendor",
        &[
            "Crestline statement of work",
            "Vendor pricing for {p}",
            "Renewing the Crestline contract",
            "Quote from Crestline",
            "Vendor security review",
        ],
        &[
            "Theo sent the updated statement of work.",
            "Their quote came in {n}% higher than last year.",
            "Legal needs to look at the liability clause.",
            "We could bring this in-house if the price doesn't move.",
            "Nina is reviewing the vendor terms.",
        ],
    ),
    (
        "expense",
        &[
            "Expense report for {m}",
            "Reimbursement question",
            "Travel expenses from the {c} trip",
            "Corporate card receipts",
        ],
        &[
            "Please submit receipts for the {c} trip by {d}.",
            "The reimbursement will be in the next payroll.",
            "Meals over the per diem need a note.",
            "I attached the receipts from the conference.",
        ],
    ),
    (
        "status",
        &[
            "Weekly update: {p}",
            "{p} status, week {n}",
            "Where we are on {p}",
            "Friday notes",
        ],
        &[
            "This week we closed {n} tickets and opened five.",
            "The API migration is 70% done.",
            "Blocked on the data export from the platform team.",
            "Next week: finish the importer and start load testing.",
            "No major risks right now.",
        ],
    ),
    (
        "data",
        &[
            "Dashboard for {p} usage",
            "Metrics question",
            "Retention numbers for {m}",
            "SQL for the weekly report",
        ],
        &[
            "Weekly active users are up {n}% since the redesign.",
            "Retention dipped after the pricing change.",
            "Ben built a dashboard with the funnel by cohort.",
            "Can you double-check the query? The totals look off.",
            "The churn numbers exclude trial accounts.",
        ],
    ),
];

const WORK_REPLIES: &[&str] = &[
    "Thanks, that works for me.",
    "Sounds good.",
    "Can we push this to {d}?",
    "Agreed, let's do it.",
    "I have a few concerns, see below.",
    "Looping in {who}.",
    "Makes sense. I'll take the first pass.",
    "Can you share the doc?",
    "Done, updated.",
    "Let's discuss at standup.",
    "+1",
    "I'm out {d}, but {who} can cover.",
    "Good catch, fixed.",
    "Will do.",
    "Any update on this?",
];

const ROLES: &[&str] = &[
    "senior backend",
    "product designer",
    "data analyst",
    "engineering manager",
    "support lead",
];
const CUSTOMERS: &[&str] = &["Linden Partners", "Bluepeak", "Fjordsoft"];

fn work(c: &mut Corpus, rng: &mut Rng, n: usize) {
    for i in 0..n {
        let (topic, subjects, bank) = WORK_TOPICS[rng.zipf(WORK_TOPICS.len())];
        let p = *rng.pick(PROJECTS);
        let q = (1 + rng.below(4)).to_string();
        let m = *rng.pick(MONTHS);
        let d = *rng.pick(WEEKDAYS);
        let num = (2 + rng.below(40)).to_string();
        let r = *rng.pick(ROLES);
        let cust = *rng.pick(CUSTOMERS);
        let city = rng.pick(CITIES).1;
        let npeople = 1 + rng.below(3);
        let mut people: Vec<&Person> = Vec::new();
        while people.len() < npeople {
            let p = &COLLEAGUES[rng.zipf(COLLEAGUES.len())];
            if !people.iter().any(|x| x.key == p.key) {
                people.push(p);
            }
        }
        if topic == "customer" {
            let idx = CUSTOMERS.iter().position(|x| *x == cust).unwrap();
            people.push(&PARTNERS[1 + idx]);
        }
        if topic == "vendor" {
            people.push(&PARTNERS[0]);
        }
        let vars = [
            ("p", p),
            ("q", q.as_str()),
            ("m", m),
            ("d", d),
            ("n", num.as_str()),
            ("r", r),
            ("cust", cust),
            ("c", city),
            ("who", people[0].first()),
        ];
        let subject = fill(rng.pick(subjects), &vars);
        let mut t = Thread::new(WORK, format!("w{i:05}"), subject)
            .tag("kind:work")
            .tag(format!("topic:{topic}"))
            .tag(format!("project:{}", p.to_lowercase()));
        for p in &people {
            t = t.tag(p.tag());
        }
        let len = 2 + (rng.f().powf(1.1) * 12.0) as usize;
        let mut date = c.days_ago(skew(rng, 1100), rng.range(8, 18), rng.range(0, 59));
        let start_from_me = rng.chance(0.35);
        for k in 0..len {
            let from_me = if k == 0 {
                start_from_me
            } else {
                rng.chance(0.4)
            };
            let other = people[rng.below(people.len())];
            let (from, to): (Address, Vec<Address>) = if from_me {
                (me(WORK), people.iter().map(|p| p.addr()).collect())
            } else {
                let mut to = vec![me(WORK)];
                to.extend(
                    people
                        .iter()
                        .filter(|p| p.key != other.key)
                        .map(|p| p.addr()),
                );
                (other.addr(), to)
            };
            let greet = if from_me {
                format!("Hi {},", other.first())
            } else {
                "Hi Alex,".to_string()
            };
            let sign = if from_me {
                "Alex".to_string()
            } else {
                other.first().to_string()
            };
            let body = if k == 0 {
                format!(
                    "{greet}\n\n{}\n\n{sign}",
                    sentence_pick(rng, bank, (2, 3), &vars)
                )
            } else {
                let mut s = fill(rng.pick(WORK_REPLIES), &vars);
                if rng.chance(0.6) {
                    s.push(' ');
                    s.push_str(&sentence_pick(rng, bank, (1, 2), &vars));
                }
                format!("{s}\n\n{sign}")
            };
            let mut msg = Msg::new(from, to, date, body);
            if k > 0 && rng.chance(0.85) {
                msg = msg.reply();
            }
            if rng.chance(0.08) {
                let f = match topic {
                    "budget" => Att::xlsx(
                        format!("{p}-forecast-Q{q}.xlsx"),
                        48_000 + rng.below(90_000) as u64,
                    ),
                    "launch" => Att::pdf(format!("{p}-launch-plan.pdf"), 210_000),
                    "design" => Att::pdf(format!("{p}-mockups-v{num}.pdf"), 2_400_000),
                    "hiring" => Att::pdf(
                        format!("candidate-packet-{}.pdf", r.replace(' ', "-")),
                        180_000,
                    ),
                    "incident" => Att::docx(format!("postmortem-{p}.docx"), 60_000),
                    "expense" => Att::pdf(format!("expenses-{m}.pdf"), 90_000),
                    _ => Att::pdf(format!("{p}-{topic}-notes.pdf"), 120_000),
                };
                msg = msg.att(f);
            }
            if rng.chance(0.05) {
                msg = msg.label("STARRED");
            }
            t = t.msg(msg);
            date += rng.range(20, 60 * 26) * 60_000;
        }
        c.push(t);
    }
}

// ------------------------------------------------------------ personal ----

const PERSONAL_TOPICS: &[(&str, &[&str], &[&str])] = &[
    (
        "dinner",
        &[
            "Dinner {d}?",
            "Tacos this week?",
            "Dinner at ours on {d}",
            "Table for four",
        ],
        &[
            "Are you free for dinner on {d}?",
            "We could try the new Thai place on 5th.",
            "I'll book a table for 7:30.",
            "Bring the kids if you like.",
            "Let's do our place, I'll cook.",
        ],
    ),
    (
        "hike",
        &[
            "Hike on {d}?",
            "Trail plans",
            "Weekend hike",
            "Sunrise hike idea",
        ],
        &[
            "Want to hike the ridge trail this weekend?",
            "Meet at the trailhead at 8.",
            "Bring layers, it gets windy at the top.",
            "It's about 9 miles with the loop.",
        ],
    ),
    (
        "birthday",
        &[
            "{who}'s birthday",
            "Surprise party for {who}",
            "Birthday gift ideas",
        ],
        &[
            "We're planning a small surprise for {who}.",
            "Can you chip in for the gift?",
            "The party starts at 6, don't be late.",
            "I was thinking a nice pen or a book.",
        ],
    ),
    (
        "books",
        &[
            "Book club: next pick",
            "Did you finish the book?",
            "Book club on {d}",
        ],
        &[
            "Next month's pick is a short novel, promise.",
            "I couldn't put it down.",
            "We're meeting at Jess's this time.",
            "Bring snacks, I'll bring wine.",
        ],
    ),
    (
        "photos",
        &[
            "Photos from the weekend",
            "Pictures!",
            "Some photos for you",
        ],
        &[
            "Here are the photos from Saturday.",
            "The one by the lake came out great.",
            "Sending a few more, the rest are in the shared album.",
        ],
    ),
    (
        "moving",
        &["Help moving on {d}?", "Moving boxes", "New place!"],
        &[
            "Could you help us move on {d}? Pizza on me.",
            "I have extra boxes if you need them.",
            "The new place has a tiny balcony.",
            "We need to be out by noon.",
        ],
    ),
    (
        "concert",
        &["Tickets for {d}", "Concert next month?", "Spare ticket"],
        &[
            "I have a spare ticket for the show on {d}.",
            "Doors open at 7.",
            "It's the band we saw at the festival.",
            "Let me know by tomorrow.",
        ],
    ),
];

const PERSONAL_REPLIES: &[&str] = &[
    "Yes! Count me in.",
    "Can't this time, sorry.",
    "Sounds fun.",
    "What time?",
    "Perfect, see you then.",
    "Haha, of course.",
    "Let me check with Sofia.",
    "Love it.",
    "Maybe, I'll let you know.",
];

fn personal(c: &mut Corpus, rng: &mut Rng, n: usize) {
    let friends: Vec<&Person> = FRIENDS
        .iter()
        .filter(|p| !matches!(p.key, "carmen" | "rosa" | "pablo" | "mike-d"))
        .collect();
    for i in 0..n {
        let (topic, subjects, bank) = PERSONAL_TOPICS[rng.below(PERSONAL_TOPICS.len())];
        let other = friends[rng.zipf(friends.len())];
        let bday = friends[rng.below(friends.len())];
        let d = *rng.pick(&["Friday", "Saturday", "Sunday", "Thursday"]);
        let vars = [("d", d), ("who", bday.first())];
        let mut t = Thread::new(
            PERSONAL,
            format!("p{i:05}"),
            fill(rng.pick(subjects), &vars),
        )
        .tag("kind:personal")
        .tag(format!("topic:{topic}"))
        .tag(other.tag());
        let mut date = c.days_ago(skew(rng, 1000), rng.range(8, 22), rng.range(0, 59));
        let len = 1 + rng.below(7);
        for k in 0..len {
            let from_me = if k == 0 { rng.chance(0.4) } else { k % 2 == 1 };
            let (from, to) = if from_me {
                (me(PERSONAL), vec![other.addr()])
            } else {
                (other.addr(), vec![me(PERSONAL)])
            };
            let body = if k == 0 {
                sentence_pick(rng, bank, (2, 1), &vars)
            } else {
                fill(rng.pick(PERSONAL_REPLIES), &vars)
            };
            let mut m = Msg::new(from, to, date, body);
            if k > 0 {
                m = m.reply();
            }
            if topic == "photos" && k == 0 {
                for j in 0..(1 + rng.below(4)) {
                    m = m.att(Att::jpg(
                        format!("IMG_{}.jpg", 4000 + rng.below(5000) + j),
                        2_000_000 + rng.below(3_000_000) as u64,
                    ));
                }
            }
            t = t.msg(m);
            date += rng.range(30, 60 * 20) * 60_000;
        }
        c.push(t);
    }
}

// ------------------------------------------------------------- spanish ----

const ES_TOPICS: &[(&str, &[&str], &[&str])] = &[
    (
        "visita",
        &["¿Cuándo venís?", "Visita en {m}", "Planes para el verano"],
        &[
            "¿Ya sabéis cuándo venís a Valencia?",
            "La habitación de invitados está lista.",
            "Tu padre quiere ir a la playa si hace bueno.",
            "Avísame con tiempo para organizarlo todo.",
        ],
    ),
    (
        "receta",
        &[
            "La receta de la paella",
            "Receta del flan",
            "Lo que te prometí",
        ],
        &[
            "Aquí tienes la receta, sin trampas.",
            "El secreto es el sofrito, con paciencia.",
            "Usa arroz bomba si lo encuentras.",
            "Me cuentas cómo te sale.",
        ],
    ),
    (
        "cumple",
        &[
            "Cumpleaños de la abuela",
            "Regalo para {who}",
            "Fiesta sorpresa",
        ],
        &[
            "La abuela cumple 85 y queremos hacer algo especial.",
            "¿Te parece bien que pongamos cada uno veinte euros?",
            "Será el sábado a mediodía en casa de Rosa.",
            "No le digas nada, que es sorpresa.",
        ],
    ),
    (
        "fotos",
        &["Fotos del bautizo", "Las fotos que te dije", "Recuerdos"],
        &[
            "Te mando las fotos del bautizo.",
            "Mira qué guapos salís todos.",
            "Hay más en el álbum compartido.",
        ],
    ),
    (
        "salud",
        &["Cómo está papá", "Resultados del médico", "Noticias"],
        &[
            "Papá ya está mucho mejor, no te preocupes.",
            "El médico dice que todo va bien.",
            "Mañana tiene otra revisión, te cuento.",
        ],
    ),
    (
        "navidad",
        &[
            "Navidad este año",
            "¿Dónde cenamos en Nochebuena?",
            "Lotería de Navidad",
        ],
        &[
            "Este año la cena es en nuestra casa.",
            "¿Traes tú el turrón?",
            "He comprado un décimo para todos.",
        ],
    ),
];

const ES_REPLIES: &[&str] = &[
    "¡Genial! Gracias, mamá.",
    "Perfecto, lo hablamos.",
    "Qué bien, me alegro mucho.",
    "Vale, te llamo esta noche.",
    "Un abrazo muy fuerte.",
    "Claro que sí.",
];

fn spanish(c: &mut Corpus, rng: &mut Rng, n: usize) {
    let fam: Vec<&Person> = ["carmen", "rosa", "pablo", "luis"]
        .iter()
        .map(|k| person(k))
        .collect();
    for i in 0..n {
        let (topic, subjects, bank) = ES_TOPICS[rng.below(ES_TOPICS.len())];
        let other = fam[rng.zipf(fam.len())];
        let vars = [
            ("m", *rng.pick(&["julio", "agosto", "diciembre", "abril"])),
            ("who", "Luis"),
        ];
        let mut t = Thread::new(
            PERSONAL,
            format!("es{i:05}"),
            fill(rng.pick(subjects), &vars),
        )
        .tag("kind:personal")
        .tag("lang:es")
        .tag(format!("topic:{topic}"))
        .tag(other.tag());
        let mut date = c.days_ago(skew(rng, 1000), rng.range(9, 22), rng.range(0, 59));
        for k in 0..(1 + rng.below(6)) {
            let from_me = k % 2 == 1;
            let (from, to) = if from_me {
                (me(PERSONAL), vec![other.addr()])
            } else {
                (other.addr(), vec![me(PERSONAL)])
            };
            let body = if k == 0 {
                format!(
                    "Hola Alex:\n\n{}\n\nBesos,\n{}",
                    sentence_pick(rng, bank, (2, 2), &vars),
                    other.first()
                )
            } else {
                rng.pick(ES_REPLIES).to_string()
            };
            let mut m = Msg::new(from, to, date, body);
            if k > 0 {
                m = m.reply();
            }
            if topic == "fotos" && k == 0 {
                m = m.att(Att::jpg(
                    format!("bautizo_{}.jpg", rng.below(90) + 10),
                    3_100_000,
                ));
            }
            t = t.msg(m);
            date += rng.range(60, 60 * 30) * 60_000;
        }
        c.push(t);
    }
}

// -------------------------------------------------------------- studio ----

const STUDIO_TOPICS: &[(&str, &[&str], &[&str])] = &[
    (
        "logo",
        &[
            "Logo revisions for {client}",
            "{client} logo: round {n}",
            "Wordmark options",
        ],
        &[
            "Attached are three directions for the wordmark.",
            "We tightened the kerning on option two.",
            "The monogram works well at small sizes.",
            "Happy to do one more round.",
        ],
    ),
    (
        "website",
        &[
            "{client} website copy",
            "Homepage draft for {client}",
            "Site launch timeline",
        ],
        &[
            "The homepage copy is in the shared doc.",
            "We still need photos for the about page.",
            "Launch could be the week after next.",
            "Can you confirm the opening hours?",
        ],
    ),
    (
        "brand",
        &[
            "{client} brand guidelines",
            "Colour palette",
            "Brand book v{n}",
        ],
        &[
            "The brand book covers colours, type and tone.",
            "We swapped the green for a warmer olive.",
            "Here are the final guidelines as a PDF.",
        ],
    ),
    (
        "shoot",
        &[
            "Photo shoot on {d}",
            "Shot list for {client}",
            "Product photos",
        ],
        &[
            "The photographer can do {d} morning.",
            "Here's the shot list, let me know what's missing.",
            "We'll need the products ready by 9.",
        ],
    ),
];

fn studio(c: &mut Corpus, rng: &mut Rng, n: usize) {
    for i in 0..n {
        let (topic, subjects, bank) = STUDIO_TOPICS[rng.below(STUDIO_TOPICS.len())];
        let client = &CLIENTS[rng.zipf(CLIENTS.len())];
        let company = client.role.split(" (").next().unwrap();
        let num = (2 + rng.below(5)).to_string();
        let d = *rng.pick(WEEKDAYS);
        let vars = [("client", company), ("n", num.as_str()), ("d", d)];
        let mut t = Thread::new(STUDIO, format!("s{i:05}"), fill(rng.pick(subjects), &vars))
            .tag("kind:client")
            .tag(format!("topic:{topic}"))
            .tag(client.tag());
        let mut date = c.days_ago(skew(rng, 900), rng.range(8, 19), rng.range(0, 59));
        let spanish = client.key == "irene" && rng.chance(0.6);
        if spanish {
            t = t.tag("lang:es");
        }
        for k in 0..(1 + rng.below(9)) {
            let from_me = if k == 0 { rng.chance(0.6) } else { k % 2 == 0 };
            let (from, to) = if from_me {
                (me(STUDIO), vec![client.addr()])
            } else {
                (client.addr(), vec![me(STUDIO)])
            };
            let body = if spanish && !from_me {
                format!(
                    "Hola Alex,\n\n{}\n\nUn saludo,\nIrene",
                    rng.pick(&[
                        "Me encanta la segunda propuesta, pero el color es demasiado frío.",
                        "¿Podemos mover la reunión al jueves?",
                        "Te envío los textos para la web esta tarde.",
                        "Perfecto, adelante con la versión final.",
                    ])
                )
            } else if k == 0 {
                format!(
                    "Hi {},\n\n{}\n\nBest,\n{}",
                    if from_me { client.first() } else { "Alex" },
                    sentence_pick(rng, bank, (2, 1), &vars),
                    if from_me { "Alex" } else { client.first() }
                )
            } else {
                fill(
                    rng.pick(&[
                        "Thanks, looks great.",
                        "One small change: can we try a darker shade?",
                        "Approved!",
                        "Let's talk {d}.",
                        "Got it, will send by end of day.",
                    ]),
                    &vars,
                )
            };
            let mut m = Msg::new(from, to, date, body);
            if k > 0 {
                m = m.reply();
            }
            if from_me && k == 0 && rng.chance(0.4) {
                m = m.att(Att::pdf(
                    format!("{}-{topic}-v{num}.pdf", company.replace(' ', "-")),
                    1_800_000,
                ));
            }
            t = t.msg(m);
            date += rng.range(60, 60 * 40) * 60_000;
        }
        c.push(t);
    }
}

// --------------------------------------------------------- newsletters ----

const NEWS_HEADLINES: &[(&str, &[&str])] = &[
    (
        "morningbyte",
        &[
            "Chips, clouds and a quiet week for startups",
            "The database renaissance",
            "Why every app wants to be a platform",
            "Open-source licensing, again",
            "Batteries that charge in minutes",
            "The browser wars are back",
            "Remote work, three years in",
            "A new programming language every week",
            "Security keys go mainstream",
        ],
    ),
    (
        "panpantry",
        &[
            "Five weeknight pastas",
            "Sheet-pan dinners for busy weeks",
            "The only banana bread you need",
            "Soups for cold nights",
            "Grilling season starts now",
            "Pickles, jams and preserving the summer",
            "Better breakfast tacos",
            "A lemon cake for every occasion",
        ],
    ),
    (
        "strideweekly",
        &[
            "How to build your base mileage",
            "Choosing your first marathon",
            "Stretching myths",
            "Running in the heat",
            "Trail shoes, reviewed",
            "Rest days matter",
        ],
    ),
    (
        "designnotes",
        &[
            "Type pairing for product UI",
            "Grids that breathe",
            "Designing empty states",
            "Motion with purpose",
            "The case for fewer settings",
            "Colour contrast in dark mode",
        ],
    ),
    (
        "harborlocal",
        &[
            "City council approves new bike lanes",
            "Farmers market returns to the pier",
            "Road closures this weekend",
            "School board meeting recap",
            "Library extends weekend hours",
            "New ferry schedule",
        ],
    ),
    (
        "fieldguide",
        &[
            "Birds to spot this month",
            "Planting for pollinators",
            "Tide pools at low tide",
            "Night sky highlights",
        ],
    ),
    (
        "elpulso",
        &[
            "Las noticias de la semana",
            "Cultura: lo que no te puedes perder",
            "Economía: los precios siguen subiendo",
            "Deportes: resumen de la jornada",
            "Tecnología para todos",
        ],
    ),
];

const NEWS_BODY: &[&str] = &[
    "In this issue: {h}, plus three links worth your time.",
    "Thanks for reading. Forward this to a friend who'd enjoy it.",
    "Our pick of the week is below.",
    "Reply to this email to tell us what you think.",
    "You're receiving this because you subscribed.",
    "This issue is sponsored by our readers.",
];

fn newsletters(c: &mut Corpus, rng: &mut Rng, n: usize) {
    for i in 0..n {
        let k = rng.zipf(NEWSLETTERS.len());
        let s = &NEWSLETTERS[k];
        let (_, heads) = NEWS_HEADLINES
            .iter()
            .find(|(key, _)| *key == s.key)
            .unwrap();
        let h = *rng.pick(heads);
        let acct = if matches!(s.key, "morningbyte" | "designnotes") && rng.chance(0.5) {
            WORK
        } else {
            PERSONAL
        };
        let date = c.days_ago(skew(rng, 700), 6, rng.range(0, 59));
        let es = s.key == "elpulso";
        let text = if es {
            format!("{h}\n\nEn este boletín: {h}. Gracias por leernos.\n\nPara darte de baja, haz clic aquí.")
        } else {
            format!(
                "{h}\n\n{}\n\nUnsubscribe · Manage preferences",
                sentence_pick(rng, NEWS_BODY, (3, 1), &[("h", h)])
            )
        };
        let html = format!(
            "<html><head><style>.c{{max-width:600px}}</style></head><body><table class=\"c\"><tr><td><h1>{h}</h1>{}</td></tr></table></body></html>",
            to_html(&text)
        );
        let mut t = Thread::new(acct, format!("n{i:05}"), format!("{}: {h}", s.name))
            .tag("kind:newsletter")
            .tag(format!("newsletter:{}", s.key));
        if es {
            t = t.tag("lang:es");
        }
        let m = Msg::new(s.addr(), vec![me(acct)], date, text)
            .html(html)
            .bulk()
            .label("CATEGORY_UPDATES");
        c.push(t.msg(m));
    }
}

const PROMO_SUBJECTS: &[&str] = &[
    "{pct}% off everything this weekend",
    "Last chance: sale ends tonight",
    "New arrivals you'll love",
    "Your cart is waiting",
    "Members save {pct}% today",
    "Free shipping on all orders",
    "A gift for you inside",
];

fn promotions(c: &mut Corpus, rng: &mut Rng, n: usize) {
    for i in 0..n {
        let s = &PROMOS[rng.zipf(PROMOS.len())];
        let pct = (10 + 5 * rng.below(9)).to_string();
        let subj = fill(rng.pick(PROMO_SUBJECTS), &[("pct", &pct)]);
        let text = format!(
            "{subj}\n\nShop now at {}. Terms apply. Unsubscribe.",
            s.name
        );
        let date = c.days_ago(skew(rng, 600), rng.range(7, 20), 0);
        let t = Thread::new(
            PERSONAL,
            format!("pr{i:05}"),
            format!("{subj} | {}", s.name),
        )
        .tag("kind:promotion")
        .tag(s.tag());
        let m = Msg::new(s.addr(), vec![me(PERSONAL)], date, text.clone())
            .html(to_html(&text))
            .bulk()
            .label("CATEGORY_PROMOTIONS");
        c.push(t.msg(m));
    }
}

// ------------------------------------------------------------ receipts ----

pub struct Order {
    pub merchant: usize,
    pub number: String,
    pub item: String,
    pub date: i64,
}

const ITEMS: &[(&str, &[&str])] = &[
    (
        "parcelmart",
        &[
            "USB-C charging cable",
            "Wireless mouse",
            "Laptop sleeve",
            "Desk lamp",
            "Phone case",
            "HDMI adapter",
            "Kitchen scale",
            "French press",
        ],
    ),
    (
        "gearloft",
        &[
            "Trail running shoes",
            "Rain shell jacket",
            "Hiking socks (3 pack)",
            "Headlamp",
            "Water filter",
            "Sleeping pad",
        ],
    ),
    (
        "pagebound",
        &[
            "The Quiet Harbor (paperback)",
            "Field Notes on Birds",
            "A History of Maps",
            "Cooking with Fire",
        ],
    ),
    (
        "greengrocer",
        &["Weekly vegetable box", "Fruit basket", "Pantry restock"],
    ),
    (
        "appmarket",
        &[
            "Weather Pro (yearly)",
            "PhotoTweak",
            "Notes+ subscription",
            "Puzzle Pack",
        ],
    ),
    (
        "mercadosol",
        &[
            "Aceite de oliva virgen extra",
            "Cafetera italiana",
            "Juego de sartenes",
        ],
    ),
];

const RESTAURANTS: &[&str] = &[
    "Thai Basil",
    "Luigi's Pizza",
    "Sushi Koi",
    "Green Bowl",
    "Taquería El Sol",
    "Burger Barn",
    "Pho Saigon",
];
const PLACES: &[&str] = &[
    "Home",
    "Office",
    "Union Station",
    "Pier 39 Garage",
    "Mission St",
    "Harbor Clinic",
    "Northwind HQ",
];

fn json_ld_order(merchant: &str, number: &str, item: &str, total: &str, currency: &str) -> String {
    format!(
        "<script type=\"application/ld+json\">{{\"@context\":\"http://schema.org\",\"@type\":\"Order\",\"merchant\":{{\"@type\":\"Organization\",\"name\":\"{merchant}\"}},\"orderNumber\":\"{number}\",\"priceCurrency\":\"{currency}\",\"price\":\"{total}\",\"acceptedOffer\":{{\"@type\":\"Offer\",\"itemOffered\":{{\"@type\":\"Product\",\"name\":\"{item}\"}}}}}}</script>"
    )
}

fn receipts(c: &mut Corpus, rng: &mut Rng, n: usize) -> Vec<Order> {
    let mut orders = Vec::new();
    for i in 0..n {
        let mi = rng.zipf(MERCHANTS.len());
        let s = &MERCHANTS[mi];
        let date = c.days_ago(skew(rng, 900), rng.range(7, 23), rng.range(0, 59));
        let (subject, text, number, item, total, currency) = match s.key {
            "swiftcab" => {
                let total = money(rng, 8, 64);
                let (a, b) = (*rng.pick(PLACES), *rng.pick(PLACES));
                let wd = *rng.pick(&[
                    "Monday",
                    "Tuesday",
                    "Wednesday",
                    "Thursday",
                    "Friday",
                    "Saturday",
                    "Sunday",
                ]);
                let part = *rng.pick(&["morning", "afternoon", "evening", "night"]);
                let num = format!("SC-{}", rng.digits(7));
                (format!("Your {wd} {part} trip with Swiftcab"),
                 format!("Thanks for riding, Alex.\n\nTotal ${total}\nTrip {num}\nPickup: {a}\nDropoff: {b}\nDistance {}.{} mi\n\nPayment: Visa ending 4417", rng.range(1, 18), rng.below(10)),
                 num, format!("ride {a} to {b}"), total, "USD")
            }
            "dishdash" => {
                let total = money(rng, 14, 70);
                let r = *rng.pick(RESTAURANTS);
                let num = format!("DD{}", rng.digits(8));
                (format!("Your Dishdash order from {r}"),
                 format!("Your order from {r} is confirmed.\n\nOrder {num}\nSubtotal ${}\nDelivery fee $2.99\nTotal ${total}\n\nEnjoy your meal!", money(rng, 10, 50)),
                 num, r.to_string(), total, "USD")
            }
            "beanhouse" => {
                let total = money(rng, 3, 14);
                let num = format!("BH-{}", rng.digits(6));
                ("Your Beanhouse receipt".to_string(),
                 format!("Thanks for stopping by!\n\n{}\nTotal ${total}\nReceipt {num}\nStars earned: {}", rng.pick(&["Oat latte", "Cold brew", "Cortado and a croissant", "Drip coffee"]), rng.below(30)),
                 num, "coffee".into(), total, "USD")
            }
            key => {
                let items = ITEMS
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| *v)
                    .unwrap_or(&["Item"]);
                let item = rng.pick(items).to_string();
                let es = key == "mercadosol";
                let total = money(rng, 6, 180);
                let num = match key {
                    "parcelmart" => format!("PM-{}", rng.digits(8)),
                    "gearloft" => format!("GL{}", rng.digits(6)),
                    "pagebound" => format!("PB-{}", rng.digits(5)),
                    "appmarket" => format!("M{}", rng.code(9)),
                    "mercadosol" => format!("MS-{}", rng.digits(7)),
                    _ => format!("GG-{}", rng.digits(6)),
                };
                if es {
                    (format!("Confirmación de tu pedido {num}"),
                     format!("¡Gracias por tu compra, Alex!\n\nPedido {num}\n{item}\nTotal: {total} €\n\nTe avisaremos cuando salga del almacén."),
                     num, item, total, "EUR")
                } else {
                    (format!("Your {} order {num}", s.name),
                     format!("Thanks for your order, Alex.\n\nOrder number: {num}\n1 × {item}\nTotal: ${total}\n\nWe'll email you when it ships."),
                     num, item, total, "USD")
                }
            }
        };
        let with_ld = rng.chance(0.35);
        let mut html = to_html(&text);
        if with_ld {
            html = html.replace(
                "<body>",
                &format!(
                    "<body>{}",
                    json_ld_order(s.name, &number, &item, &total, currency)
                ),
            );
        }
        let mut t = Thread::new(PERSONAL, format!("r{i:05}"), subject)
            .tag("kind:receipt")
            .tag(s.tag())
            .tag(format!("order:{}", number.to_lowercase()))
            .tag(format!("amount:{total}"));
        if with_ld {
            t = t.tag("jsonld:order");
        }
        if s.key == "mercadosol" {
            t = t.tag("lang:es");
        }
        let body = if with_ld && rng.chance(0.5) {
            // HTML-only mail: the provider's plain text is the HTML's text.
            penguin_core::text::html_to_text(&html)
        } else {
            text
        };
        c.push(
            t.msg(
                Msg::new(s.addr(), vec![me(PERSONAL)], date, body)
                    .html(html)
                    .label("CATEGORY_UPDATES"),
            ),
        );
        if matches!(
            s.key,
            "parcelmart" | "gearloft" | "pagebound" | "mercadosol"
        ) {
            orders.push(Order {
                merchant: mi,
                number,
                item,
                date,
            });
        }
    }
    orders
}

fn shipping(c: &mut Corpus, rng: &mut Rng, orders: &[Order]) {
    for (i, o) in orders.iter().enumerate() {
        if !rng.chance(0.75) {
            continue;
        }
        let carrier = &CARRIERS[rng.below(CARRIERS.len())];
        let tracking = if carrier.key == "parcelpost" {
            format!("PP{}US", rng.digits(9))
        } else {
            format!("SW{}", rng.digits(10))
        };
        let merchant = MERCHANTS[o.merchant].name;
        let es = MERCHANTS[o.merchant].key == "mercadosol";
        let mut t = Thread::new(
            PERSONAL,
            format!("sh{i:05}"),
            if es {
                format!("Tu pedido {} está en camino", o.number)
            } else {
                format!("Your {merchant} package is on its way")
            },
        )
        .tag("kind:shipping")
        .tag(format!("carrier:{}", carrier.key))
        .tag(MERCHANTS[o.merchant].tag())
        .tag(format!("order:{}", o.number.to_lowercase()))
        .tag(format!("tracking:{}", tracking.to_lowercase()));
        if es {
            t = t.tag("lang:es");
        }
        let steps: &[&str] = if es {
            &[
                "Tu pedido {o} ha salido del almacén. Número de seguimiento: {t}.",
                "Tu paquete llega hoy. Seguimiento {t}.",
                "Entregado: tu paquete {t} se ha entregado.",
            ]
        } else {
            &["Good news! Your order {o} ({i}) has shipped with {c}. Tracking number: {t}. Estimated delivery in 3-5 business days.",
              "Out for delivery: your package {t} will arrive today by 8 pm.",
              "Delivered: your package {t} was left at the front door."]
        };
        let mut date = o.date + rng.range(1, 2) * DAY;
        for (k, st) in steps.iter().enumerate().take(1 + rng.below(3)) {
            let body = fill(
                st,
                &[
                    ("o", &o.number),
                    ("i", &o.item),
                    ("c", carrier.name),
                    ("t", &tracking),
                ],
            );
            let mut m =
                Msg::new(carrier.addr(), vec![me(PERSONAL)], date, body).label("CATEGORY_UPDATES");
            if k > 0 {
                m = m.subject(if es {
                    format!("Actualización del pedido {}", o.number)
                } else if k == 1 {
                    "Out for delivery".to_string()
                } else {
                    "Delivered".to_string()
                });
            }
            t = t.msg(m);
            date += rng.range(1, 3) * DAY;
        }
        c.push(t);
    }
}

// --------------------------------------------------------------- bills ----

fn bills(c: &mut Corpus, rng: &mut Rng) {
    let mut i = 0;
    for b in BILLERS {
        let (acct, lo, hi, every) = match b.key {
            "cedarvalley" => (PERSONAL, 45, 160, 1),
            "brightwave" => (PERSONAL, 59, 79, 1),
            "harbormobile" => (PERSONAL, 38, 72, 1),
            "pinecrest" => (PERSONAL, 30, 95, 2),
            "nimbus" => (STUDIO, 12, 48, 1),
            "streamly" => (PERSONAL, 15, 15, 1),
            "tunewave" => (PERSONAL, 10, 10, 1),
            _ => (PERSONAL, 640, 640, 12),
        };
        // One statement per calendar month (early in the month, as billers
        // send them), not every 30 days: 30-day steps drift against the
        // calendar, so depending on the generation day some month got none
        // and "the bill from March" had nothing to find.
        let this_month = crate::window::add_months(crate::window::today(c.now), 0);
        let mut months_back = 1;
        while months_back <= 26 {
            let first = crate::window::add_months(this_month, -(months_back as i32));
            let date = crate::window::local_ms(first)
                + (rng.range(2, 8) as i64) * DAY
                + rng.range(6, 10) as i64 * 3_600_000
                + rng.range(0, 59) as i64 * 60_000;
            let amount = money(rng, lo, hi);
            let acctno = format!("{}", 100_000 + (b.key.len() * 7919) % 900_000);
            let month = fmt_local(date, "%B %Y");
            let ym = fmt_local(date, "%Y-%m");
            let due = fmt_local(date + 21 * DAY, "%B %-d");
            let (subject, body) = match b.key {
                "streamly" | "tunewave" => (format!("Your {} receipt", b.name), format!("Thanks for being a member.\n\nPlan: Standard\nAmount charged: ${amount}\nBilling period: {month}")),
                "shieldline" => ("Your renters insurance renewal".to_string(), format!("Your policy renews next month.\n\nAnnual premium: ${amount}\nPolicy number RI-{acctno}")),
                _ => (format!("Your {} bill for {month}", b.name), format!("Hi Alex,\n\nYour statement for {month} is ready.\n\nAccount {acctno}\nAmount due: ${amount}\nDue date: {due}\n\nAutopay is on; no action needed.")),
            };
            let pdf = !matches!(b.key, "streamly" | "tunewave");
            let mut m = Msg::new(b.addr(), vec![me(acct)], date, body).label("CATEGORY_UPDATES");
            if pdf {
                m = m.att(Att::pdf(
                    format!("Statement_{ym}.pdf"),
                    140_000 + rng.below(80_000) as u64,
                ));
            }
            let t = Thread::new(acct, format!("b{i:05}"), subject)
                .tag("kind:bill")
                .tag(b.tag())
                .tag(format!("month:{ym}"))
                .tag(format!("amount:{amount}"))
                .msg(m);
            c.push(t);
            i += 1;
            months_back += every;
        }
    }
}

fn invoices(c: &mut Corpus, rng: &mut Rng, n: usize) {
    // Freelance invoices Alex sends (studio), and Crestline's to work.
    for i in 0..n {
        let client = &CLIENTS[rng.zipf(CLIENTS.len())];
        let company = client.role.split(" (").next().unwrap();
        let days = skew(rng, 800);
        let date = c.days_ago(days, rng.range(9, 17), rng.range(0, 59));
        let year = fmt_local(date, "%Y");
        let num = format!("MS-{year}-{:03}", 1 + i % 60);
        let amount = format!("{},{:03}.00", rng.range(1, 6), rng.below(1000));
        let work = *rng.pick(&[
            "logo design",
            "website build",
            "brand guidelines",
            "product photography",
            "menu redesign",
        ]);
        let mut t = Thread::new(STUDIO, format!("iv{i:05}"), format!("Invoice {num} — {company}"))
            .tag("kind:invoice")
            .tag("direction:out")
            .tag(client.tag())
            .tag(format!("invoice:{}", num.to_lowercase()))
            .msg(Msg::new(me(STUDIO), vec![client.addr()], date, format!("Hi {},\n\nPlease find attached invoice {num} for the {work}. Total ${amount}, due in 30 days.\n\nThanks!\nAlex", client.first()))
                .att(Att::pdf(format!("{num}.pdf"), 64_000)));
        if rng.chance(0.6) {
            t = t.msg(
                Msg::new(
                    client.addr(),
                    vec![me(STUDIO)],
                    date + rng.range(2, 20) * DAY,
                    "Paid today, thanks Alex!",
                )
                .reply(),
            );
        }
        c.push(t);
    }
    for i in 0..30 {
        let date = c.days_ago(20 + i * 30 + rng.range(0, 5), 9, 30);
        let num = format!("CR-{}", 40_100 + i * 13);
        let amount = format!("{},{:03}.00", rng.range(8, 14), rng.below(1000));
        c.push(
            Thread::new(WORK, format!("ivc{i:03}"), format!("Crestline invoice {num}"))
                .tag("kind:invoice")
                .tag("direction:in")
                .tag("person:theo")
                .tag(format!("invoice:{}", num.to_lowercase()))
                .msg(Msg::new(PARTNERS[0].addr(), vec![me(WORK)], date, format!("Hello,\n\nAttached is invoice {num} for monthly platform support. Amount: ${amount}. Payment terms net 30.\n\nTheo Laurent\nCrestline"))
                    .att(Att::pdf(format!("{num}.pdf"), 88_000))),
        );
    }
}

// -------------------------------------------------------------- travel ----

fn travel(c: &mut Corpus, rng: &mut Rng, n: usize) {
    for i in 0..n {
        let (ckey, city, code, country) = *rng.pick(CITIES);
        let airline = &AIRLINES[if matches!(ckey, "madrid" | "valencia") && rng.chance(0.5) {
            2
        } else {
            rng.below(2)
        }];
        let pnr = rng.code(6);
        let flight_no = format!(
            "{}{}",
            airline.name[..2].to_uppercase(),
            rng.range(100, 2999)
        );
        let booked = c.days_ago(skew(rng, 1000) + 20, rng.range(8, 22), rng.range(0, 59));
        let depart = booked + rng.range(10, 60) * DAY;
        let dep_s = fmt_local(depart, "%a, %b %-d");
        let ret_s = fmt_local(depart + rng.range(3, 9) * DAY, "%a, %b %-d");
        let acct = if rng.chance(0.4) { WORK } else { PERSONAL };
        let es = airline.key == "aerovia";
        let text = if es {
            format!("Hola Alex,\n\nTu reserva está confirmada.\n\nLocalizador: {pnr}\nVuelo {flight_no}: San Francisco (SFO) → {city} ({code})\nIda: {dep_s}\nVuelta: {ret_s}\n\nBuen viaje.")
        } else {
            format!("Hi Alex,\n\nYour booking is confirmed.\n\nConfirmation code: {pnr}\nFlight {flight_no}: San Francisco (SFO) → {city} ({code})\nDepart: {dep_s}\nReturn: {ret_s}\n\nCheck in opens 24 hours before departure.")
        };
        let mut html = to_html(&text);
        let ld = rng.chance(0.4);
        if ld {
            html = html.replace("<body>", &format!("<body><script type=\"application/ld+json\">{{\"@context\":\"http://schema.org\",\"@type\":\"FlightReservation\",\"reservationNumber\":\"{pnr}\",\"reservationFor\":{{\"@type\":\"Flight\",\"flightNumber\":\"{flight_no}\",\"airline\":{{\"@type\":\"Airline\",\"name\":\"{}\"}},\"departureAirport\":{{\"@type\":\"Airport\",\"iataCode\":\"SFO\"}},\"arrivalAirport\":{{\"@type\":\"Airport\",\"iataCode\":\"{code}\"}}}}}}</script>", airline.name));
        }
        let mut t = Thread::new(
            acct,
            format!("fl{i:05}"),
            if es {
                format!("Confirmación de reserva {pnr}")
            } else {
                format!(
                    "Your {} flight to {city} is confirmed ({pnr})",
                    airline.name
                )
            },
        )
        .tag("kind:flight")
        .tag(format!("city:{ckey}"))
        .tag(format!("country:{}", country.to_lowercase()))
        .tag(format!("airline:{}", airline.key))
        .tag(format!("pnr:{}", pnr.to_lowercase()));
        if es {
            t = t.tag("lang:es");
        }
        if ld {
            t = t.tag("jsonld:flight");
        }
        t = t.msg(
            Msg::new(airline.addr(), vec![me(acct)], booked, text)
                .html(html)
                .label("CATEGORY_UPDATES"),
        );
        if depart < c.now && rng.chance(0.7) {
            t = t.msg(Msg::new(airline.addr(), vec![me(acct)], depart - DAY, format!("It's time to check in for flight {flight_no} to {city}. Confirmation {pnr}. Boarding passes are available in the app.")).subject(format!("Check in now: {city}")));
        }
        c.push(t);
        // Most trips also have a hotel.
        if rng.chance(0.65) {
            let h = &HOTELS[rng.below(HOTELS.len())];
            let conf = rng.digits(9);
            let nights = rng.range(2, 7);
            let name = format!(
                "{} {city}",
                rng.pick(&["Harborview", "Grand", "Parkside", "Old Town", "Riverside"])
            );
            let body = format!("Your stay at {name} is booked.\n\nConfirmation number {conf}\nCheck-in: {dep_s}\n{nights} nights, 1 room\nTotal ${}\n\nFree cancellation until 48 hours before arrival.", money(rng, 300, 1800));
            c.push(
                Thread::new(
                    acct,
                    format!("ho{i:05}"),
                    format!("Booking confirmed: {name}"),
                )
                .tag("kind:hotel")
                .tag(format!("city:{ckey}"))
                .tag(format!("country:{}", country.to_lowercase()))
                .tag(format!("merchant:{}", h.key))
                .tag(format!("hotelconf:{conf}"))
                .msg(
                    Msg::new(
                        h.addr(),
                        vec![me(acct)],
                        booked + rng.range(0, 3) * DAY,
                        body,
                    )
                    .label("CATEGORY_UPDATES"),
                ),
            );
        }
    }
}

// ------------------------------------------------------------- invites ----

const INVITE_TITLES: &[(&str, &str)] = &[
    ("Weekly sync", "status"),
    ("{p} design review", "design"),
    ("1:1 Alex / {who}", "one-on-one"),
    ("Q{q} planning", "planning"),
    ("{p} launch go/no-go", "launch"),
    ("Interview: {r}", "hiring"),
    ("Budget review", "budget"),
    ("Incident retro", "incident"),
    ("Customer call: {cust}", "customer"),
    ("All hands", "allhands"),
];

fn invites(c: &mut Corpus, rng: &mut Rng, n: usize) {
    for i in 0..n {
        let (title, topic) = *rng.pick(INVITE_TITLES);
        let org = &COLLEAGUES[rng.zipf(COLLEAGUES.len())];
        let p = *rng.pick(PROJECTS);
        let q = (1 + rng.below(4)).to_string();
        let title = fill(
            title,
            &[
                ("p", p),
                ("who", org.first()),
                ("q", &q),
                ("r", rng.pick(ROLES)),
                ("cust", rng.pick(CUSTOMERS)),
            ],
        );
        let sent = c.days_ago(skew(rng, 800), rng.range(8, 18), rng.range(0, 59));
        let when = sent + rng.range(1, 14) * DAY;
        let when_s = fmt_local(when, "%a %b %-d, %Y 10:00am");
        let body = format!("{} has invited you to this event.\n\n{title}\nWhen: {when_s}\nWhere: Zoom / Room {}\n\nGoing? Yes · No · Maybe", org.name, rng.range(1, 9));
        let mut t = Thread::new(
            WORK,
            format!("in{i:05}"),
            format!("Invitation: {title} @ {when_s}"),
        )
        .tag("kind:invite")
        .tag(format!("topic:{topic}"))
        .tag(org.tag())
        .msg(Msg::new(org.addr(), vec![me(WORK)], sent, body).att(Att::ics()));
        if rng.chance(0.3) {
            t = t.msg(
                Msg::new(
                    me(WORK),
                    vec![org.addr()],
                    sent + 3_600_000,
                    format!("Alex Moreno has accepted this invitation.\n\n{title}"),
                )
                .subject(format!("Accepted: {title} @ {when_s}")),
            );
        }
        c.push(t);
    }
}

// --------------------------------------------------------------- codes ----

fn codes(c: &mut Corpus, rng: &mut Rng, n: usize) {
    for i in 0..n {
        let s = &CODE_SENDERS[rng.zipf(CODE_SENDERS.len())];
        let code = rng.digits(6);
        let es = s.key == "solaris";
        let (subject, body) = if es {
            (format!("Tu código de verificación: {code}"), format!("Tu código de verificación de Banco Solaris es {code}. Caduca en 10 minutos. No lo compartas con nadie."))
        } else {
            match rng.below(3) {
                0 => (format!("Your {} verification code", s.name), format!("Your verification code is {code}. It expires in 10 minutes. If you didn't request this, ignore this email.")),
                1 => (format!("{code} is your {} code", s.name), format!("Use {code} to sign in to {}. Don't share this code with anyone.", s.name)),
                _ => (format!("Sign-in attempt on {}", s.name), format!("We noticed a new sign-in. Enter {code} to confirm it's you.")),
            }
        };
        let acct = if s.key == "nimbus" {
            STUDIO
        } else if rng.chance(0.2) {
            WORK
        } else {
            PERSONAL
        };
        let date = c.days_ago(skew(rng, 700), rng.range(0, 23), rng.range(0, 59));
        let mut t = Thread::new(acct, format!("c{i:05}"), subject)
            .tag("kind:code")
            .tag(format!("merchant:{}", s.key))
            .tag(format!("code:{code}"));
        if es {
            t = t.tag("lang:es");
        }
        c.push(t.msg(Msg::new(s.addr(), vec![me(acct)], date, body).label("CATEGORY_UPDATES")));
    }
}

// -------------------------------------------------------------- social ----

fn social(c: &mut Corpus, rng: &mut Rng, n: usize) {
    for i in 0..n {
        let s = &SOCIAL[rng.zipf(SOCIAL.len())];
        let f = &FRIENDS[rng.below(FRIENDS.len())];
        let (subject, body) = match s.key {
            "circlely" => {
                let what = *rng.pick(&[
                    "commented on your photo",
                    "liked your post",
                    "mentioned you in a comment",
                    "shared a memory with you",
                ]);
                (
                    format!("{} {what}", f.name),
                    format!(
                        "{} {what}.\n\n\"{}\"\n\nSee it on Circlely.",
                        f.name,
                        rng.pick(&[
                            "Love this!",
                            "So good to see you",
                            "Where is this?",
                            "Miss you guys"
                        ])
                    ),
                )
            }
            "makersforum" => {
                let topic = *rng.pick(&[
                    "Best budget 3D printer?",
                    "Show your workbench",
                    "Soldering iron tips",
                    "Woodworking for beginners",
                ]);
                (
                    format!("Makers Forum digest: {topic}"),
                    format!(
                        "Popular this week: {topic}\n\n{} new replies in threads you follow.",
                        rng.range(2, 40)
                    ),
                )
            }
            _ => {
                let trail = *rng.pick(&[
                    "Ridge Loop",
                    "Coastal Bluffs",
                    "Redwood Creek",
                    "Summit Trail",
                ]);
                (
                    format!("New activity on {trail}"),
                    format!(
                        "{} completed {trail} ({} mi). Give kudos!",
                        f.name,
                        rng.range(3, 14)
                    ),
                )
            }
        };
        let date = c.days_ago(skew(rng, 600), rng.range(7, 23), rng.range(0, 59));
        let mut t = Thread::new(PERSONAL, format!("so{i:05}"), subject)
            .tag("kind:social")
            .tag(format!("merchant:{}", s.key));
        if s.key != "makersforum" {
            // The notification is about this friend: a name search should
            // count it as related.
            t = t.tag(f.tag());
        }
        c.push(
            t.msg(
                Msg::new(s.addr(), vec![me(PERSONAL)], date, body)
                    .bulk()
                    .label("CATEGORY_SOCIAL"),
            ),
        );
    }
}
