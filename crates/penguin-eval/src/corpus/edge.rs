//! Edge-case targets: the mail the edge-case categories point at
//! (docs/SEARCH-CASES.md). Each conversation is tagged `plant:<id>` like
//! `planted.rs`, and each exists to make one kind of query answerable:
//!
//! - **people**: diacritics (José Núñez), letters that don't decompose
//!   (Søren Ødegård, Straße), CJK (田中 美咲), apostrophes (O'Brien),
//!   hyphens (Anne-Marie), nicknames (Robert who signs "Bob"), a display name
//!   unlike the address (Dr. Ruth Evans, revans@), an internationalized
//!   domain, subdomains, plus-addressing, emoji, an ampersand;
//! - **format**: a tracking number printed in groups, another printed as one
//!   run, European amounts (1.234,56 €), phone numbers with country codes,
//!   URLs, version numbers, case numbers with a slash;
//! - **morphology**: British spellings (cancelled, favourite, colour, grey,
//!   organise, licence, theatre, catalogue);
//! - **place**: user labels with spaces, nesting and hyphens; Trash; Spam;
//! - **attachment**: underscores, spaces and upper-case extensions in file
//!   names, a presentation, big files, HEIC, a PDF sent as octet-stream, CSV;
//! - **date**: messages either side of local midnight, a message on a known
//!   day whose day of month is above 12 (so D/M/Y and M/D/Y differ), one
//!   exactly two weeks old, one last weekend.
//!
//! Then user labels are applied to background mail the way a person files
//! it (`labels`). Everything is fictional and every domain ends in `.example`.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

use super::world::*;
use super::{addr, fmt_local, me, to_html, Att, Corpus, Msg, Thread};
use crate::window::{local_ms, today};

const MIN: i64 = 60_000;
const HOUR: i64 = 3_600_000;

/// Local time `h:m` on the day `days` before the corpus's today (exact
/// across DST changes, unlike `Corpus::days_ago`).
pub fn at(now: i64, days: i64, h: i64, m: i64) -> i64 {
    local_ms(today(now) - Duration::days(days)) + h * HOUR + m * MIN
}

/// The moving day: the first day at least 40 days back whose day of month
/// is above 12, so a day-first date (`14/08/2026`) can't be read month-first.
pub fn movers_day(now: i64) -> NaiveDate {
    let mut d = today(now) - Duration::days(40);
    while d.day() <= 12 {
        d -= Duration::days(1);
    }
    d
}

/// Days back of the Kestrel pair: one at 23:50 on `KESTREL_LATE` days ago,
/// the other at 00:10 the next day.
pub const KESTREL_LATE: i64 = 10;

/// The Saturday of the most recent weekend that has fully passed.
pub fn last_weekend(now: i64) -> NaiveDate {
    let t = today(now);
    let mut d = t - Duration::days(2);
    while d.weekday() != Weekday::Sat {
        d -= Duration::days(1);
    }
    d
}

fn one(
    acct: usize,
    id: &str,
    subject: &str,
    from: penguin_core::Address,
    date: i64,
    body: &str,
    tags: &[&str],
) -> Thread {
    Thread::new(acct, format!("e-{id}"), subject)
        .tag(format!("plant:{id}"))
        .tags(tags)
        .msg(Msg::new(from, vec![me(acct)], date, body))
}

pub fn add(c: &mut Corpus) {
    people(c);
    formats(c);
    morphology(c);
    attachments(c);
    dates(c);
    places(c);
    labels(c);
}

fn people(c: &mut Corpus) {
    let n = c.now;
    let jose = addr("José Núñez", "jose.nunez@correo.example");
    c.push(
        Thread::new(PERSONAL, "e-jose-cadiz", "Fotos del viaje a Cádiz")
            .tags(&["plant:jose-cadiz", "kind:personal", "person:jose", "lang:es", "topic:photos"])
            .msg(
                Msg::new(jose.clone(), vec![me(PERSONAL)], at(n, 30, 20, 14),
                    "¡Hola Alex!\n\nTe mando por fin las fotos del viaje a Cádiz: la playa de la Caleta al atardecer y la cena en el puerto. La del faro es mi favorita.\n\nUn abrazo,\nJosé")
                .att(Att::jpg("caleta-atardecer.jpg", 2_600_000))
                .att(Att::jpg("faro.jpg", 2_200_000)),
            ),
    );
    c.push(one(PERSONAL, "jose-cena", "Cena del sábado en Málaga", jose, at(n, 75, 19, 2),
        "Alex, ¿te apuntas a la cena del sábado en Málaga? Reservé en El Pimpi a las nueve. José",
        &["kind:personal", "person:jose", "lang:es"]));

    c.push(one(WORK, "soren", "Nordlys pilot: kickoff notes", addr("Søren Ødegård", "soren@nordlys.example"), at(n, 26, 15, 40),
        "Hi Alex,\n\nThanks for the kickoff. Notes: the pilot covers two warehouses, success is measured on pick accuracy, and we meet every second Tuesday. I'll send the data export by Friday.\n\nBest regards,\nSøren Ødegård\nNordlys Logistics",
        &["kind:work", "person:soren", "topic:vendor"]));

    c.push(one(WORK, "tanaka-cjk", "東京オフィスの会議室予約について", addr("田中 美咲", "misaki.tanaka@tokyo.northwind.example"), at(n, 9, 10, 5),
        "Alexさん\n\n10月14日の午後、東京オフィスの会議室Bを予約しました。プロジェクターも使えます。\n\n(Room B at the Tokyo office is booked for the afternoon of October 14; the projector works.)\n\n田中",
        &["kind:work", "person:misaki", "city:tokyo", "lang:ja"]));

    c.push(one(WORK, "obrien", "Greenleaf renewal pricing", addr("Siobhán O'Brien", "siobhan.obrien@greenleaf.example"), at(n, 17, 11, 20),
        "Hi Alex,\n\nBefore we sign the renewal, could you send pricing for 40 seats on the annual plan? Procurement wants it by the end of the month.\n\nThanks,\nSiobhán O'Brien\nGreenleaf",
        &["kind:work", "person:siobhan", "topic:customer"]));

    c.push(
        Thread::new(STUDIO, "e-anne-marie", "Illustrations for the Fernhill packaging")
            .tags(&["plant:anne-marie", "kind:client", "person:annemarie", "topic:logo"])
            .msg(
                Msg::new(addr("Anne-Marie Dubois", "am.dubois@atelier-lune.example"), vec![me(STUDIO)], at(n, 44, 9, 30),
                    "Bonjour Alex,\n\nHere are the illustrations for the bakery packaging, version 3: the wheat border and the little bicycle. Tell me if Julia wants the warmer palette.\n\nAnne-Marie\nAtelier Lune")
                .att(Att::pdf("fernhill-illustrations-v3.pdf", 2_400_000)),
            ),
    );

    c.push(
        Thread::new(PERSONAL, "e-kowalski", "Estate documents for your signature")
            .tags(&["plant:kowalski", "kind:personal", "person:kowalski", "topic:legal"])
            .msg(
                Msg::new(addr("Robert Kowalski", "rkowalski@kowalski-law.example"), vec![me(PERSONAL)], at(n, 23, 14, 45),
                    "Alex,\n\nAttached is the draft of your will. Please read it, note any changes, and we'll schedule a signing with two witnesses.\n\nBob\nKowalski Law")
                .att(Att::docx("Moreno-will-draft.docx", 88_000)),
            ),
    );

    c.push(one(PERSONAL, "evans", "Your lab results are ready", addr("Dr. Ruth Evans", "revans@harborclinic.example"), at(n, 13, 16, 10),
        "Hello Alex, your blood test results from last week are ready in the patient portal. Everything is within the normal range; your vitamin D is a little low, so I'd suggest a supplement. Dr. Evans, Harbor Family Clinic",
        &["kind:personal", "person:evans", "topic:health"]));

    // Plus-addressing: shopping goes to alexmoreno+shopping@…
    let plus = addr(ME_NAME, "alexmoreno+shopping@mailbox.example");
    let lamps = addr("Lumen Lamps", "orders@lumenlamps.example");
    let text = "Thanks for your order, Alex!\n\nOrder LL-77120\n1 × Brass arc floor lamp\nTotal $329.00\n\nWe'll email you a tracking number when it ships.";
    c.push(
        Thread::new(PERSONAL, "e-lamp-order", "Order LL-77120 confirmed")
            .tags(&["plant:lamp-order", "kind:receipt", "merchant:lumenlamps", "order:ll-77120", "plus:shopping"])
            .msg(Msg::new(lamps.clone(), vec![plus.clone()], at(n, 11, 21, 3), text).html(to_html(text)).label("CATEGORY_UPDATES")),
    );
    c.push(
        Thread::new(PERSONAL, "e-lamp-ship", "Your Lumen Lamps order has shipped")
            .tags(&["plant:lamp-ship", "kind:shipping", "merchant:lumenlamps", "order:ll-77120", "plus:shopping", "tracking:1z4f8a620311872290"])
            .msg(Msg::new(CARRIERS[0].addr(), vec![plus.clone()], at(n, 8, 7, 55),
                "Your Lumen Lamps order LL-77120 is on its way.\n\nTracking number: 1Z 4F8 A62 03 1187 2290\nEstimated delivery: Thursday").label("CATEGORY_UPDATES")),
    );
    c.push(
        Thread::new(PERSONAL, "e-plus-books", "Your Pagebound order PB-88213")
            .tags(&["plant:plus-books", "kind:receipt", "merchant:pagebound", "plus:shopping"])
            .msg(Msg::new(MERCHANTS[5].addr(), vec![plus], at(n, 20, 18, 30),
                "Thanks for your order, Alex.\n\nOrder number: PB-88213\n1 × The Salt Road (paperback)\nTotal: $18.99").label("CATEGORY_UPDATES")),
    );
    let text = "Birdwatch Monthly\n\nThis month: autumn warblers on the coast, and how to tell a Cooper's hawk from a sharp-shinned hawk.";
    c.push(
        Thread::new(PERSONAL, "e-plus-news", "Birdwatch Monthly: autumn warblers")
            .tags(&["plant:plus-news", "kind:newsletter", "plus:newsletters"])
            .msg(Msg::new(addr("Birdwatch Monthly", "digest@birdwatch.example"), vec![addr(ME_NAME, "alexmoreno+newsletters@mailbox.example")], at(n, 16, 6, 5), text)
                .html(to_html(text)).bulk().label("CATEGORY_UPDATES")),
    );

    c.push(one(STUDIO, "muller", "Lieferadresse für die Musterkarten", addr("Jürgen Müller", "jurgen@müller-bau.example"), at(n, 36, 9, 12),
        "Hallo Alex,\n\nbitte schicken Sie die Musterkarten an: Müller Bau GmbH, Hauptstraße 12, 80331 München.\n\nVielen Dank!\nJürgen Müller",
        &["kind:client", "person:jurgen", "lang:de"]));

    c.push(one(PERSONAL, "party", "🎉 Sofia's surprise party — Saturday 🎂", person("jess").addr(), at(n, 3, 21, 40),
        "Shh 🤫 Sofia's surprise party is at mine this Saturday at 7. Bring a bottle 🍾 and don't tell her! Jess",
        &["kind:personal", "person:jess", "topic:party"]));

    c.push(one(PERSONAL, "birch-bloom", "Your Birch & Bloom order is confirmed", addr("Birch & Bloom Florists", "hello@birchandbloom.example"), at(n, 5, 12, 25),
        "Thank you, Alex! 12 garden roses with eucalyptus, delivered Saturday at 7:30 pm to Lumière for your table. Card: \"Happy anniversary\". Total $86.50.",
        &["kind:receipt", "merchant:birchbloom"]));

    let month = fmt_local(at(n, 32, 12, 0), "%B");
    c.push(
        Thread::new(PERSONAL, "e-ledgerly-statement", format!("Your {month} statement is ready"))
            .tags(&["plant:ledgerly-statement", "kind:bill", "merchant:ledgerly"])
            .msg(Msg::new(addr("Ledgerly Bank", "alerts@notify.ledgerly.example"), vec![me(PERSONAL)], at(n, 15, 6, 30),
                format!("Your {month} statement for checking account ending 2291 is ready. Sign in to view it."))
                .att(Att::pdf("Ledgerly-statement.pdf", 120_000)).label("CATEGORY_UPDATES")),
    );
}

fn formats(c: &mut Corpus) {
    let n = c.now;
    c.push(
        Thread::new(PERSONAL, "e-usps", "Your Gearloft package is on its way")
            .tags(&["plant:usps", "kind:shipping", "merchant:gearloft", "tracking:9400111899562537866361"])
            .msg(Msg::new(CARRIERS[0].addr(), vec![me(PERSONAL)], at(n, 4, 9, 15),
                "Your package from Gearloft is on its way.\n\nTracking: 9400111899562537866361\nExpected delivery: Friday by 8 pm").label("CATEGORY_UPDATES")),
    );
    c.push(one(PERSONAL, "hotel-madrid", "Confirmación de reserva PR-55817", addr("Hotel Prado Real", "reservas@pradoreal.example"), at(n, 47, 13, 0),
        "Estimado Alex:\n\nSu reserva PR-55817 está confirmada: 2 noches, habitación doble superior, del 3 al 5 de diciembre.\n\nImporte total: 1.234,56 €\nTeléfono del hotel: +34 915 55 01 23\n\nHotel Prado Real, Madrid",
        &["kind:hotel", "city:madrid", "country:spain", "lang:es", "merchant:pradoreal"]));
    c.push(one(WORK, "london-hotel", "Booking confirmed: 3 nights in London", addr("The Kensington Rooms", "stay@kensingtonrooms.example"), at(n, 58, 10, 40),
        "Dear Alex,\n\nYour booking KR-20931 is confirmed: 3 nights, arriving on the 12th. Breakfast is included. If you'll arrive after midnight, call us on +44 20 7946 0958.\n\nThe Kensington Rooms",
        &["kind:hotel", "city:london", "country:uk", "merchant:kensington"]));
    c.push(one(WORK, "fjordsoft-url", "v2 migration guide link", person("greta").addr(), at(n, 64, 9, 50),
        "Hi Alex, as promised, the guide is at https://portal.fjordsoft.example/v2/migration-guide and the changelog is on the same page. Ping me with questions. Greta",
        &["kind:work", "person:greta", "topic:vendor"]));
    c.push(one(WORK, "deploy-version", "Deploying v2.14.3 tonight", person("mateo").addr(), at(n, 2, 15, 30),
        "Release v2.14.3 goes out at 9 pm: it fixes the sync retry bug and the slow login. Rollback plan: redeploy v2.14.2. Mateo",
        &["kind:work", "person:mateo", "topic:launch"]));
    c.push(one(PERSONAL, "baggage", "Your baggage claim CX-2026/0419", addr("Skyward Air", "baggage@skywardair.example"), at(n, 31, 17, 5),
        "We're sorry your bag was delayed. We have opened case CX-2026/0419 and will deliver the bag to your address within 48 hours. Keep your receipts for essentials; we reimburse up to $150.",
        &["kind:personal", "merchant:skyward", "topic:baggage"]));
}

fn morphology(c: &mut Corpus) {
    let n = c.now;
    c.push(one(PERSONAL, "uk-cancelled", "Your order TP-40921 has been cancelled", addr("Thistle & Page", "orders@thistlepage.example"), at(n, 22, 10, 10),
        "We're sorry: your order TP-40921 has been cancelled and refunded to your card. Your favourite titles are back in stock soon; browse our autumn catalogue.",
        &["kind:receipt", "merchant:thistlepage", "topic:refund"]));
    c.push(one(STUDIO, "colour-proofs", "Colour proofs for the new bags", person("julia").addr(), at(n, 18, 14, 0),
        "Hi Alex, the colour proofs look great, but the grey on the flour bag is a bit dark. Can we organise a quick call on Thursday? Julia",
        &["kind:client", "person:julia", "topic:logo"]));
    c.push(one(PERSONAL, "licence", "Your driving licence renewal", addr("Vehicle Licensing Office", "renewals@vlo.example"), at(n, 52, 8, 20),
        "Your driving licence expires next month. Renew online, or book an appointment at your nearest licensing centre with your current licence and a recent photo.",
        &["kind:personal", "topic:licence"]));
    c.push(one(PERSONAL, "theatre", "Your theatre tickets: Twelfth Night", addr("Harbourside Theatre", "boxoffice@harboursidetheatre.example"), at(n, 14, 19, 30),
        "Thank you for booking! Twelfth Night, Friday at 7:30 pm, stalls row F, seats 11 and 12. Collect at the box office or show this email.",
        &["kind:receipt", "merchant:harbourside", "topic:theatre"]));
}

fn attachments(c: &mut Corpus) {
    let n = c.now;
    c.push(one(WORK, "q3-report", "Q3 numbers", person("ben").addr(), at(n, 12, 17, 20),
        "Alex, the Q3 report is attached (final, v2). Churn is flat; expansion revenue is up 9%.",
        &["kind:work", "person:ben", "topic:planning"]).att_last(Att::xlsx("Q3_report_final_v2.xlsx", 180_000)));
    c.push(one(WORK, "atlas-roadmap", "Atlas roadmap deck for Monday", person("priya").addr(), at(n, 7, 16, 0),
        "Here's the roadmap deck for Monday's review. Slides 4-6 are the new milestones.",
        &["kind:work", "person:priya", "project:atlas", "topic:planning"])
        .att_last(Att("Atlas-roadmap-2027.pptx".into(), "application/vnd.openxmlformats-officedocument.presentationml.presentation", 7_200_000)));
    c.push(one(PERSONAL, "wedding-toast", "The toast video", person("ben-o").addr(), at(n, 100, 22, 5),
        "Finally exported it! Here's the video of your toast at the wedding. Maya cried. Ben",
        &["kind:personal", "person:ben-o", "topic:wedding"]).att_last(Att("wedding-toast.mov".into(), "video/quicktime", 48_000_000)));
    c.push(one(STUDIO, "site-photos", "Shop photos for the new site", person("tom").addr(), at(n, 27, 11, 0),
        "Hi Alex, all the shop photos are in the zip, full resolution. Use whatever works for the homepage. Tom",
        &["kind:client", "person:tom", "topic:website"]).att_last(Att("walsh-site-photos.zip".into(), "application/zip", 18_000_000)));
    c.push(one(PERSONAL, "scan-addendum", "Scanned the addendum", person("sofia").addr(), at(n, 57, 20, 30),
        "Scanned the lease addendum from the printer, it's attached. Can you check page 2?",
        &["kind:personal", "person:sofia", "topic:housing"]).att_last(Att::pdf("Scan 2026-03-14 at 10.42.pdf", 1_100_000)));
    c.push(one(PERSONAL, "heic", "photo of the receipt", person("sofia").addr(), at(n, 16, 13, 10),
        "for the warranty, here's the photo of the receipt for the washing machine",
        &["kind:personal", "person:sofia"]).att_last(Att("IMG_2291.HEIC".into(), "image/heic", 2_100_000)));
    c.push(one(STUDIO, "octet-pdf", "Signed PO attached", person("marco").addr(), at(n, 33, 10, 45),
        "Ciao Alex, the signed purchase order for the label redesign is attached. Marco",
        &["kind:client", "person:marco"]).att_last(Att("PO-5816.PDF".into(), "application/octet-stream", 140_000)));
    c.push(one(PERSONAL, "csv", "Your transactions export", addr("Ledgerly Bank", "alerts@notify.ledgerly.example"), at(n, 9, 7, 0),
        "The export you requested is attached: all transactions for the last 90 days.",
        &["kind:bill", "merchant:ledgerly"]).att_last(Att("transactions-export.csv".into(), "text/csv", 40_000)));
}

fn dates(c: &mut Corpus) {
    let n = c.now;
    let d = movers_day(n);
    let days = (today(n) - d).num_days();
    c.push(one(PERSONAL, "movers", "Movers confirmed for your moving day", addr("Bluebird Movers", "crew@bluebirdmovers.example"), at(n, days, 10, 5),
        "Hi Alex, your crew of three is confirmed for moving day, 8 am. Please have the boxes labelled by room and the parking spot in front kept free. Bluebird Movers",
        &["kind:personal", "topic:moving"]));
    let sam = person("sam").addr();
    c.push(one(WORK, "kestrel-late", "Kestrel cluster back up", sam.clone(), at(n, KESTREL_LATE, 23, 50),
        "The Kestrel cluster is back up after the disk swap. Monitoring overnight. Sam",
        &["kind:work", "person:sam", "topic:incident"]));
    c.push(one(WORK, "kestrel-early", "Kestrel cluster down again", sam, at(n, KESTREL_LATE - 1, 0, 10),
        "Kestrel is down again, same disk errors. Paging the storage team. Sam",
        &["kind:work", "person:sam", "topic:incident"]));
    c.push(one(PERSONAL, "compost", "Compost bins now collected on Wednesdays", addr("Harbor County Recycling", "notices@harborrecycling.example"), at(n, 14, 11, 0),
        "Starting next week, green compost bins are collected on Wednesdays instead of Mondays. Put them out by 7 am.",
        &["kind:personal", "topic:civic"]));
    // Newer mail with the same words, so each date filter has to do the
    // work: without it, recency ranks these first.
    c.push(one(PERSONAL, "movers-review", "How did your move go?", addr("Bluebird Movers", "crew@bluebirdmovers.example"), at(n, 5, 9, 0),
        "Thanks for moving with Bluebird Movers! Tell us how the crew did: it takes one minute.",
        &["kind:personal", "topic:moving"]));
    c.push(one(PERSONAL, "compost-stickers", "Compost bins: new stickers", addr("Harbor County Recycling", "notices@harborrecycling.example"), at(n, 3, 10, 0),
        "Green compost bins need the new blue sticker from next month. Stickers arrive by post.",
        &["kind:personal", "topic:civic"]));
    c.push(one(PERSONAL, "brunch-next", "brunch sunday?", person("jess").addr(), at(n, 0, 8, 0),
        "same place as last time? the pancakes were worth it", &["kind:personal", "person:jess"]));
    let sat = last_weekend(n);
    let days = (today(n) - sat).num_days();
    c.push(
        Thread::new(PERSONAL, "e-brunch", "brunch pics")
            .tags(&["plant:brunch", "kind:personal", "person:jess", "topic:photos"])
            .msg(Msg::new(person("jess").addr(), vec![me(PERSONAL)], at(n, days, 14, 20),
                "the pancakes deserved a photo shoot").att(Att::jpg("pancakes.jpg", 1_800_000))),
    );
}

fn places(c: &mut Corpus) {
    let n = c.now;
    c.push(one(PERSONAL, "gym-trash", "Come back for 50% off", addr("Ironworks Gym", "members@ironworksgym.example"), at(n, 20, 9, 0),
        "We miss you! Your comeback offer: half price for three months if you rejoin before the end of the month.",
        &["kind:promotion", "topic:fitness"]).label_first("TRASH"));
    c.push(one(PERSONAL, "spam-prize", "Congratulations! Claim your $500 gift card", addr("Rewards Center", "winner@prize-alerts.example"), at(n, 6, 3, 33),
        "You have been selected! Fill in the prize claim form within 24 hours to receive your reward.",
        &["kind:spam"]).label_first("SPAM"));
    // Tax paperwork that goes under "Tax Docs 2025".
    c.push(one(PERSONAL, "tax-1099", "Your 2025 Form 1099-INT is available", addr("Ledgerly Bank", "tax@ledgerly.example"), at(n, 200, 8, 0),
        "Your 2025 Form 1099-INT (interest income: $212.40) is available. A copy is attached.",
        &["kind:tax", "merchant:ledgerly", "topic:tax"]).att_last(Att::pdf("1099-INT-2025.pdf", 52_000)));
    c.push(one(PERSONAL, "tax-w2", "Your 2025 W-2 is ready", addr("Northwind Payroll", "payroll@northwind.example"), at(n, 230, 9, 0),
        "Your 2025 Form W-2 is attached. Keep it with your tax records.",
        &["kind:tax", "topic:tax"]).att_last(Att::pdf("W2-2025-Moreno.pdf", 61_000)));
}

/// User labels, applied the way someone files mail. Parent labels exist
/// (as in Gmail, where "Clients/Fernhill Bakery" implies "Clients").
fn labels(c: &mut Corpus) {
    let has = |t: &super::ThreadFacts, tag: &str| t.has(tag);
    c.label_threads(PERSONAL, "Receipts", |t| has(t, "kind:receipt"));
    c.label_threads(PERSONAL, "Tax Docs 2025", |t| {
        has(t, "plant:tax") || has(t, "plant:tax-1099") || has(t, "plant:tax-w2")
    });
    c.user_label(STUDIO, "Clients");
    c.label_threads(STUDIO, "Clients/Fernhill Bakery", |t| has(t, "person:julia"));
    c.label_threads(STUDIO, "Clients/Bianchi Wines", |t| has(t, "person:marco"));
    c.label_threads(STUDIO, "Clients/Casa Castillo", |t| has(t, "person:irene"));
    c.user_label(PERSONAL, "Travel");
    c.label_threads(PERSONAL, "Travel/Lisbon 2026", |t| {
        has(t, "plant:lisbon-flight") || has(t, "plant:lisbon-stay")
    });
    c.user_label(WORK, "Work");
    c.user_label(WORK, "Work/Atlas");
    c.label_threads(WORK, "Work/Atlas/Finance", |t| has(t, "project:atlas") && has(t, "topic:budget"));
    c.label_threads(PERSONAL, "Family", |t| {
        has(t, "person:carmen") || has(t, "person:luis") || has(t, "person:rosa")
    });
    c.label_threads(WORK, "Follow-up", |t| {
        has(t, "plant:self-review") || has(t, "plant:pto") || has(t, "plant:laptop")
    });
}
