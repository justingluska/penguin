//! Hand-written conversations that specific queries point at. Each one is
//! tagged `plant:<id>`; `queries.rs` judges them (and the background mail
//! that shares their concepts) with graded rules.
//!
//! Many are written so the words someone would search with are NOT in the
//! message (vocabulary mismatch): the plumber email never says "water
//! heater", the flight to Lisbon never says "Portugal" or "plane tickets",
//! the electricity bill never says "electricity". Keyword search should miss
//! those; search by meaning should not.

use super::world::*;
use super::{addr, fmt_local, me, to_html, Att, Corpus, Msg, Thread};
use crate::rng::Rng;
use crate::window::{Window, DAY};

const HOUR: i64 = 3_600_000;

/// Time for a plant: `days` ago at `hour` local.
fn ago(c: &Corpus, days: i64, hour: i64) -> i64 {
    c.days_ago(days, hour, 17)
}

fn inside(c: &Corpus, w: Window, id: &str) -> i64 {
    w.pick(c.now, &mut Rng::fork(c.seed, id))
}

/// One-message conversation.
fn one(
    acct: usize,
    id: &str,
    subject: &str,
    from: penguin_core::Address,
    date: i64,
    body: &str,
    tags: &[&str],
) -> Thread {
    Thread::new(acct, format!("p-{id}"), subject)
        .tag(format!("plant:{id}"))
        .tags(tags)
        .msg(Msg::new(from, vec![me(acct)], date, body))
}

pub fn add(c: &mut Corpus) {
    personal(c);
    work(c);
    studio(c);
    bulk(c);
}

fn personal(c: &mut Corpus) {
    let mike = person("mike-d").addr();

    // The lease: PDF from Mike (the landlord's agent) last spring.
    let d = inside(c, Window::LastSpring, "lease");
    let t = Thread::new(PERSONAL, "p-lease", "Lease renewal for 14 Alder St, Unit 3")
        .tags(&["plant:lease", "kind:personal", "person:mike-d", "topic:housing"])
        .msg(Msg::new(mike.clone(), vec![me(PERSONAL)], d,
            "Hi Alex,\n\nAttached is the renewal agreement for another 12 months at 14 Alder St, Unit 3. Monthly rent goes to $2,450 starting June 1. Please sign and send it back by May 15.\n\nThe early termination clause is unchanged: two months' notice.\n\nBest,\nMike Delgado\nHarbor Realty")
            .att(Att::pdf("Alder-St-lease-2026.pdf", 412_000)))
        .msg(Msg::new(me(PERSONAL), vec![mike.clone()], d + 26 * HOUR,
            "Thanks Mike, signed copy attached.").reply().att(Att::pdf("Alder-St-lease-2026-signed.pdf", 430_000)));
    c.push(t);

    // An older note from the same Mike (distractor for "mike lease").
    let d = ago(c, 400, 10);
    c.push(one(PERSONAL, "parking", "Parking spot assignment", mike.clone(), d,
        "Hi Alex, starting next month your assigned parking spot is #22 in the back lot. The gate remote is in your mailbox. Mike", &["kind:personal", "person:mike-d", "topic:housing"]));

    // The plumber (never says "water heater").
    let d = ago(c, 9, 16);
    c.push(one(PERSONAL, "plumber", "Plumber visit on Thursday", mike.clone(), d,
        "Hi Alex,\n\nThe plumber from Rapid Rooter will come by Thursday between 9 and 11 am to swap out the boiler tank in your unit. The hot tap will be off for about three hours while they work.\n\nMike",
        &["kind:personal", "person:mike-d", "topic:housing"]));

    // Flight to Lisbon (upcoming): never says Portugal or "plane tickets".
    let booked = ago(c, 12, 20);
    let dep = c.now + 19 * DAY;
    let dep_s = fmt_local(dep, "%A, %B %-d");
    let text = format!("Hi Alex,\n\nYour booking is confirmed.\n\nConfirmation code: K7QX2M\nFlight SK 2210: San Francisco (SFO) → Lisbon (LIS)\nDeparts {dep_s} at 6:05 pm, arrives next day 1:40 pm\nReturn SK 2211: {}\n\nSeats 23A, 23B. Check in opens 24 hours before departure.", fmt_local(dep + 8 * DAY, "%A, %B %-d"));
    c.push(
        Thread::new(
            PERSONAL,
            "p-lisbon-flight",
            "Your Skyward Air flight to Lisbon is confirmed (K7QX2M)",
        )
        .tags(&[
            "plant:lisbon-flight",
            "kind:flight",
            "city:lisbon",
            "country:portugal",
            "airline:skyward",
            "pnr:k7qx2m",
            "upcoming",
        ])
        .msg(
            Msg::new(AIRLINES[0].addr(), vec![me(PERSONAL)], booked, text.clone())
                .html(to_html(&text))
                .label("CATEGORY_UPDATES"),
        ),
    );

    // Where we're staying in Lisbon (never says "hotel").
    c.push(one(PERSONAL, "lisbon-stay", "Your stay at Casa do Rio, Alfama", HOTELS[1].addr(), booked + 2 * HOUR,
        &format!("Olá Alex! Your guesthouse booking is complete.\n\nCasa do Rio, Rua dos Remédios 41, Alfama\nArrive {dep_s}, 7 nights, double room with river view\nBooking reference NA-883104\n\nYour host, Inês, will meet you with the keys."),
        &["kind:hotel", "city:lisbon", "country:portugal", "merchant:nestaway", "hotelconf:na-883104", "upcoming"]));

    // Dentist last spring (says "dental" and "cleaning", not "dentist").
    let d = inside(c, Window::LastSpring, "dental");
    c.push(one(PERSONAL, "dental", "Appointment reminder: Bright Smile Dental",
        addr("Bright Smile Dental", "frontdesk@brightsmile.example"), d,
        "Hi Alex, this is a reminder of your cleaning and checkup with Dr. Anjali Patel next Tuesday at 3:30 pm. Please arrive 10 minutes early. Reply C to confirm.",
        &["kind:personal", "topic:health", "merchant:brightsmile"]));
    // …and a later bill from the same practice (related, not the visit).
    c.push(one(PERSONAL, "dental-bill", "Your statement from Bright Smile Dental",
        addr("Bright Smile Dental", "billing@brightsmile.example"), ago(c, 30, 11),
        "Your balance after insurance is $45.00 for the visit on file. Pay online or at your next visit.",
        &["kind:bill", "topic:health", "merchant:brightsmile"]));

    // Cabin details from Jess.
    let jess = person("jess").addr();
    let d = ago(c, 5, 19);
    c.push(Thread::new(PERSONAL, "p-cabin", "Cabin details for the weekend")
        .tags(&["plant:cabin", "kind:personal", "person:jess", "topic:trip"])
        .msg(Msg::new(jess.clone(), vec![me(PERSONAL), person("sofia").addr()], d,
            "Hey both!\n\nThe cabin is at 88 Pine Hollow Rd. The door code is 4471 and the wifi network is PineHollow, password sunflower-42. Firewood is in the shed.\n\nSee you Friday!\nJess"))
        .msg(Msg::new(me(PERSONAL), vec![jess.clone()], d + 2 * HOUR, "Amazing, thank you! We'll bring breakfast stuff.").reply()));

    // Standing desk: order (with JSON-LD) and shipping. Says "sit-stand".
    let d = ago(c, 16, 21);
    let text = "Thanks for your order, Alex!\n\nOrder DC-55120\n1 × Electric sit-stand workstation, walnut top 60×30 in\nSubtotal $649.00\nShipping: free\nTotal charged: $649.00 (Visa ending 4417)";
    let html = to_html(text).replace("<body>", "<body><script type=\"application/ld+json\">{\"@context\":\"http://schema.org\",\"@type\":\"Order\",\"merchant\":{\"@type\":\"Organization\",\"name\":\"Deskcraft\"},\"orderNumber\":\"DC-55120\",\"priceCurrency\":\"USD\",\"price\":\"649.00\",\"acceptedOffer\":{\"@type\":\"Offer\",\"itemOffered\":{\"@type\":\"Product\",\"name\":\"Electric sit-stand workstation\"}}}</script>");
    c.push(
        Thread::new(PERSONAL, "p-desk-order", "Order confirmation DC-55120")
            .tags(&[
                "plant:desk-order",
                "kind:receipt",
                "merchant:deskcraft",
                "order:dc-55120",
                "amount:649.00",
                "jsonld:order",
            ])
            .msg(
                Msg::new(
                    addr("Deskcraft", "orders@deskcraft.example"),
                    vec![me(PERSONAL)],
                    d,
                    text,
                )
                .html(html)
                .label("CATEGORY_UPDATES"),
            ),
    );
    c.push(Thread::new(PERSONAL, "p-desk-ship", "Your Deskcraft package is on its way")
        .tags(&["plant:desk-ship", "kind:shipping", "carrier:swiftship", "merchant:deskcraft", "order:dc-55120", "tracking:sw4829105533"])
        .msg(Msg::new(CARRIERS[1].addr(), vec![me(PERSONAL)], ago(c, 3, 8),
            format!("Your package from Deskcraft (order DC-55120) has shipped. Tracking number SW4829105533. Expected delivery: {}. Freight delivery, someone must be home to sign.", fmt_local(c.now + 2 * DAY, "%A, %B %-d")))
            .label("CATEGORY_UPDATES")));

    // The vet (never says "dog" or "shots").
    c.push(one(PERSONAL, "vet", "Biscuit is due for her booster", addr("Oakwood Veterinary", "care@oakwoodvet.example"), ago(c, 7, 10),
        "Hi Alex, our records show Biscuit is due for her rabies booster and annual exam before the 30th. Book online or call us at (415) 555-0190. Wags, the Oakwood team.",
        &["kind:personal", "topic:pet"]));

    // Tax return from the accountant, with the refund amount.
    let d = inside(c, Window::LastSpring, "tax");
    c.push(Thread::new(PERSONAL, "p-tax", "Your 2025 return has been filed")
        .tags(&["plant:tax", "kind:personal", "topic:tax", "person:grace"])
        .msg(Msg::new(addr("Grace Liu, CPA", "grace@liucpa.example"), vec![me(PERSONAL)], d,
            "Hi Alex,\n\nGood news: your 2025 federal and state tax returns were e-filed today and accepted. You should see a refund of $1,284 by direct deposit within three weeks. A copy is attached for your records.\n\nGrace")
            .att(Att::pdf("Moreno-2025-Return.pdf", 980_000))));

    // Ben's wedding (save the date).
    let ben = person("ben-o").addr();
    c.push(one(PERSONAL, "wedding", "Save the date: Ben & Maya", ben.clone(), ago(c, 120, 18),
        "We're getting married! Save the date: Saturday, October 17, at Silverado Vineyards in Napa. Formal invitation to follow. Can't wait to celebrate with you. Ben & Maya",
        &["kind:personal", "person:ben-o", "topic:wedding"]));

    // Concert tickets (never says "seats").
    c.push(one(PERSONAL, "tickets", "Your tickets: Neon Harbor, Nov 8", addr("Ticketeer", "tickets@ticketeer.example"), ago(c, 21, 13),
        "You're going! Neon Harbor at the Fillmore Hall, Sunday Nov 8, doors 7 pm. Section B, Row 12, places 7 and 8. Mobile entry: show the barcode at the door. Order TK-661902.",
        &["kind:receipt", "merchant:ticketeer", "order:tk-661902", "topic:concert"]));

    // Bike shop (says "bike", not "bicycle"; the query uses "bicycle").
    c.push(one(PERSONAL, "bike", "Your bike is ready for pickup", addr("Spoke & Chain", "shop@spokeandchain.example"), ago(c, 40, 15),
        "Hi Alex, your bike is ready. We replaced the chain and the rear derailleur and trued the back wheel. Total $142.00, pay at pickup. Open until 6.",
        &["kind:receipt", "merchant:spokeandchain", "amount:142.00"]));

    // Jury duty (says "juror", "summons").
    c.push(one(PERSONAL, "jury", "Juror summons: please confirm", addr("Harbor County Superior Court", "jury@harborcourt.example"), ago(c, 18, 9),
        &format!("You have been summoned for service. Report on {} at 8:00 am to the Hall of Justice, 400 Main St, Room 110. Confirm online with juror number 7730214.", fmt_local(c.now + 36 * DAY, "%A, %B %-d")),
        &["kind:personal", "topic:civic"]));

    // Headphones refund (says "refund", "noise-cancelling headphones"; query "money back").
    c.push(one(PERSONAL, "refund", "Your refund has been processed", MERCHANTS[3].addr(), ago(c, 25, 12),
        "We've processed your refund of $89.99 for Noise-cancelling headphones (order PM-30418872). It will appear on your card in 3–5 business days.",
        &["kind:receipt", "merchant:parcelmart", "order:pm-30418872", "topic:refund"]));

    // Tahoe photos from Jess last summer (never says "pictures").
    let d = inside(c, Window::LastSummer, "tahoe");
    let t = Thread::new(PERSONAL, "p-tahoe", "Tahoe weekend")
        .tags(&[
            "plant:tahoe",
            "kind:personal",
            "person:jess",
            "topic:photos",
        ])
        .msg(
            Msg::new(
                jess.clone(),
                vec![me(PERSONAL)],
                d,
                "Here are the shots from the lake! The sunset one is my favourite.",
            )
            .att(Att::jpg("IMG_7781.jpg", 3_400_000))
            .att(Att::jpg("IMG_7784.jpg", 3_100_000))
            .att(Att::jpg("IMG_7790.jpg", 2_900_000)),
        );
    c.push(t);

    // Insurance claim (water damage).
    c.push(one(PERSONAL, "claim", "Claim RI-CLM-30918: adjuster assigned", BILLERS[7].addr(), ago(c, 50, 11),
        "Your claim RI-CLM-30918 for water damage in the kitchen has been assigned to adjuster Paul Nguyen. He will call within two business days to schedule an inspection.",
        &["kind:personal", "merchant:shieldline", "topic:insurance"]));

    // Neighbour's new number.
    c.push(one(PERSONAL, "dana-number", "New number", person("dana").addr(), ago(c, 60, 20),
        "Hi neighbour! Quick note: I switched carriers and my new cell is (415) 555-0138. The old one stops working Friday. Dana",
        &["kind:personal", "person:dana"]));

    // Passport renewal.
    c.push(one(PERSONAL, "passport", "Application received: passport renewal", addr("Passport Services", "status@passports.example"), ago(c, 34, 10),
        "We received your renewal application on file number 59-2210-448. Routine processing takes 6 to 8 weeks. You can check the status online with your last name and date of birth.",
        &["kind:personal", "topic:travel-docs"]));

    // Gym cancellation.
    c.push(one(PERSONAL, "gym", "Your membership has been cancelled", addr("Ironworks Gym", "members@ironworksgym.example"), ago(c, 150, 9),
        "We're sorry to see you go. Your Ironworks membership ends at the close of this billing cycle; you won't be charged again. Your guest passes remain valid for 30 days.",
        &["kind:personal", "topic:fitness"]));

    // Anniversary dinner booking.
    c.push(one(PERSONAL, "anniversary", "Reservation confirmed: Lumière, Saturday 8:00 pm", addr("Lumière", "reservations@lumiere.example"), ago(c, 4, 14),
        "Your table for 2 is confirmed for Saturday at 8:00 pm. Note on the booking: anniversary. We hold tables for 15 minutes. To change, reply to this email.",
        &["kind:personal", "topic:dinner", "merchant:lumiere"]));

    // --- Spanish ---
    let carmen = person("carmen").addr();
    c.push(Thread::new(PERSONAL, "p-paella", "La receta de la paella de la abuela")
        .tags(&["plant:paella", "kind:personal", "lang:es", "person:carmen", "topic:receta"])
        .msg(Msg::new(carmen.clone(), vec![me(PERSONAL)], ago(c, 200, 21),
            "Hola hijo:\n\nPor fin la he copiado del cuaderno de tu abuela. Para cuatro: 400 g de arroz bomba, un pollo troceado, conejo, judía verde, garrofó, tomate rallado, pimentón, azafrán y romero. El secreto es el sofrito, sin prisas, y no remover el arroz.\n\nBesos,\nMamá")));
    let arrive = c.now + 16 * DAY;
    c.push(Thread::new(PERSONAL, "p-parents-arrive", "Llegamos el día 12")
        .tags(&["plant:parents-arrive", "kind:personal", "lang:es", "person:carmen", "topic:visita", "upcoming"])
        .msg(Msg::new(carmen.clone(), vec![me(PERSONAL)], ago(c, 6, 22),
            format!("Hola cariño:\n\nYa tenemos los billetes. Aterrizamos en San Francisco el {} a las 15:40, vuelo AV 118 desde Madrid. ¿Nos puedes recoger en el aeropuerto? Tu padre trae el jamón.\n\nUn beso,\nMamá", fmt_local(arrive, "%-d/%-m"))))
        .msg(Msg::new(me(PERSONAL), vec![carmen.clone()], ago(c, 6, 23), "¡Claro que sí! Allí estaré.").reply()));
    let pablo = person("pablo").addr();
    c.push(one(PERSONAL, "valencia-flat", "Piso en Ruzafa para julio", pablo.clone(), ago(c, 90, 19),
        "¡Hola Alex! Un amigo alquila su piso en Ruzafa todo julio: dos habitaciones, terraza y a diez minutos de la playa en bici. Pide 1.600 euros el mes. Si os interesa, le digo que lo reserve.\n\nAbrazos,\nPablo",
        &["kind:personal", "lang:es", "person:pablo", "topic:housing", "city:valencia"]));
    c.push(one(PERSONAL, "otp-es", "Tu código de verificación: 771903", CODE_SENDERS[5].addr(), ago(c, 1, 8),
        "Tu código de verificación de Banco Solaris es 771903. Caduca en 10 minutos. No lo compartas con nadie.",
        &["kind:code", "lang:es", "merchant:solaris", "code:771903"]));
    c.push(one(PERSONAL, "otp-bank", "Your Ledgerly Bank verification code", CODE_SENDERS[0].addr(), ago(c, 0, 9),
        "Your verification code is 604218. It expires in 10 minutes. If you didn't request this, ignore this email.",
        &["kind:code", "merchant:ledgerly", "code:604218"]));
}

fn work(c: &mut Corpus) {
    let sam = person("sam").addr();
    // Team retreat (never says "offsite").
    let t = Thread::new(WORK, "p-retreat", "Team retreat logistics: Pine Lodge")
        .tags(&["plant:retreat", "kind:work", "topic:retreat", "person:sam"])
        .msg(Msg::new(sam.clone(), vec![me(WORK), person("priya").addr(), person("mateo").addr()], ago(c, 14, 10),
            format!("Hi all,\n\nThe team retreat is booked at Pine Lodge near Truckee, {} to {}. A bus leaves the office at 8 am on day one. Rooms are shared (two per room); the list is attached. Bring hiking shoes.\n\nSam", fmt_local(c.now + 25 * DAY, "%B %-d"), fmt_local(c.now + 27 * DAY, "%B %-d")))
            .att(Att::xlsx("pine-lodge-rooms.xlsx", 22_000)))
        .msg(Msg::new(person("priya").addr(), vec![sam.clone(), me(WORK)], ago(c, 14, 12), "Can we get a vegetarian option for the dinner?").reply())
        .msg(Msg::new(sam.clone(), vec![me(WORK), person("priya").addr()], ago(c, 13, 9), "Yes, already arranged with the lodge.").reply());
    c.push(t);

    // Budget sign-off (says "Approved", not "sign off"; the query says "finance sign off").
    let kwame = person("kwame").addr();
    c.push(Thread::new(WORK, "p-atlas-budget", "Approved: Atlas FY26 budget")
        .tags(&["plant:atlas-budget", "kind:work", "topic:budget", "project:atlas", "person:kwame"])
        .msg(Msg::new(kwame.clone(), vec![me(WORK), person("omar").addr()], ago(c, 45, 16),
            "Alex,\n\nThe Atlas FY26 plan is approved at $1.2M, including the two contractor roles. Final numbers attached; the only change from your draft is the conference line, trimmed to $30k.\n\nKwame")
            .att(Att::xlsx("Atlas-FY26-budget-final.xlsx", 64_000))));

    // Phishing warning (says "phishing", "fake invoice"; query "scam").
    c.push(one(WORK, "phishing", "Heads up: phishing emails targeting finance", person("yuki").addr(), ago(c, 33, 11),
        "Team, we're seeing emails that pretend to come from Kwame asking for urgent wire transfers, with a fake invoice attached. Don't open the attachment; forward any you get to security@northwind.example and delete them.",
        &["kind:work", "topic:security", "person:yuki"]));

    // Promotion letter.
    c.push(one(WORK, "promotion", "Congratulations, Alex", person("omar").addr(), ago(c, 210, 17),
        "Alex, I'm delighted to confirm your promotion to Staff Engineer, effective the first of next month. The letter with the details of your new level is attached. Well deserved. Omar",
        &["kind:work", "topic:career", "person:omar"]).att_last(Att::pdf("Promotion-letter-Moreno.pdf", 98_000)));

    // Winter PTO approved (says "PTO", "out of office"; query "vacation").
    c.push(one(WORK, "pto", "PTO request approved: Dec 21 – Jan 4", person("lena").addr(), ago(c, 11, 15),
        "Your time-off request for December 21 through January 4 has been approved. Please set your out-of-office reply and hand off on-call before you leave.",
        &["kind:work", "topic:timeoff", "person:lena"]));

    // Replacement laptop (query "new computer").
    c.push(one(WORK, "laptop", "Your replacement laptop is ready", sam.clone(), ago(c, 8, 13),
        "Hi Alex, your new 16-inch laptop is set up and waiting at the front desk (asset tag NW-8816). Bring the old one back by Friday so we can wipe it.",
        &["kind:work", "topic:it", "person:sam"]));

    // API sunset from a partner (query "shut off").
    c.push(one(WORK, "api-sunset", "Fjordsoft v1 endpoints sunset on March 31", person("greta").addr(), ago(c, 70, 9),
        "Hi Alex, a reminder that the v1 REST endpoints will be retired on March 31. After that date, calls return 410 Gone. The v2 migration guide is linked in our developer portal. Greta",
        &["kind:work", "topic:vendor", "person:greta"]));

    // Signed contract.
    c.push(Thread::new(WORK, "p-linden-msa", "Signed MSA attached")
        .tags(&["plant:linden-msa", "kind:work", "topic:customer", "person:ana"])
        .msg(Msg::new(person("ana").addr(), vec![me(WORK), person("diego").addr()], ago(c, 95, 14),
            "Hi Alex, attached is the countersigned master services agreement. Our procurement team will send the PO next week. Looking forward to working together! Ana")
            .att(Att::pdf("Linden-Partners-MSA-countersigned.pdf", 640_000))));

    // Overdue invoice.
    let theo = person("theo").addr();
    c.push(Thread::new(WORK, "p-inv-20417", "Past due: invoice INV-20417")
        .tags(&["plant:inv-20417", "kind:invoice", "direction:in", "person:theo", "invoice:inv-20417"])
        .msg(Msg::new(theo.clone(), vec![me(WORK), kwame.clone()], ago(c, 22, 10),
            "Hello Alex,\n\nOur records show invoice INV-20417 for $18,400.00 (migration support, Q2) is now 30 days past due. Could you check on its status with your accounts payable team?\n\nThanks,\nTheo Laurent\nCrestline")
            .att(Att::pdf("INV-20417.pdf", 91_000)))
        .msg(Msg::new(kwame.clone(), vec![theo.clone(), me(WORK)], ago(c, 21, 15), "Apologies Theo, it's scheduled for this Friday's payment run.").reply()));

    // Talk accepted (query "did my talk get in").
    c.push(one(WORK, "talk", "DevHarbor 2026: your proposal was accepted", addr("DevHarbor Program Committee", "program@devharbor.example"), ago(c, 27, 16),
        "Congratulations! Your session \"Local-first sync at scale\" has been selected for DevHarbor 2026. You have a 30-minute slot on day two. Please confirm your speaker details by the 15th.",
        &["kind:work", "topic:conference"]));

    // Self-review deadline.
    c.push(one(WORK, "self-review", "Reminder: self-assessments due Friday", person("priya").addr(), ago(c, 2, 9),
        "Hi Alex, friendly reminder that self-assessments for this cycle are due Friday at 5 pm. Keep it to a page: impact, growth, and one thing you'd do differently. Priya",
        &["kind:work", "topic:review", "person:priya"]));

    // On-call rotation.
    c.push(one(WORK, "oncall", "Pager rotation for November", person("mateo").addr(), ago(c, 6, 11),
        "Here's the rotation for November. Alex: Nov 2–9 and Nov 23–30. Priya: Nov 9–16. Ben: Nov 16–23. Swap requests in the channel, please.",
        &["kind:work", "topic:incident", "person:mateo"]));

    // Customer not renewing (query "cancelling").
    c.push(one(WORK, "bluepeak-churn", "Our decision on the renewal", person("ravi").addr(), ago(c, 15, 17),
        "Alex, after a lot of discussion we've decided not to renew when our term ends in December. It isn't the product, it's budget: our parent company is consolidating tools. Happy to talk about timing. Ravi",
        &["kind:work", "topic:customer", "person:ravi"]));

    // Visa invitation letter for the Tokyo trip (query "japan").
    c.push(Thread::new(WORK, "p-visa", "Invitation letter for your Tokyo trip")
        .tags(&["plant:visa", "kind:work", "topic:travel-docs", "person:lena", "city:tokyo"])
        .msg(Msg::new(person("lena").addr(), vec![me(WORK)], ago(c, 55, 14),
            "Hi Alex, attached is the signed invitation letter from our Tokyo office for your business visa application. The consulate also needs your itinerary and a bank statement.")
            .att(Att::pdf("Invitation-letter-Tokyo.pdf", 120_000))));

    // Board deck (query "slides").
    c.push(Thread::new(WORK, "p-board-deck", "Final board deck")
        .tags(&["plant:board-deck", "kind:work", "topic:planning", "person:chloe"])
        .msg(Msg::new(person("chloe").addr(), vec![me(WORK), person("omar").addr()], ago(c, 19, 18),
            "Here's the final version for Thursday's board meeting. I folded in your Atlas numbers on page 7.")
            .att(Att::pdf("Board-deck-Q3-final.pdf", 5_600_000))));

    // Database cutover (query "database migration").
    c.push(one(WORK, "cutover", "Postgres cutover moved to Saturday 2am", person("mateo").addr(), ago(c, 3, 17),
        "Heads up: the Postgres cutover is now Saturday at 2 am Pacific. Expect about 20 minutes of read-only mode. I'll post updates in the incident channel.",
        &["kind:work", "topic:launch", "person:mateo"]));

    // Expense report returned (query "reimbursement rejected").
    c.push(one(WORK, "expense-returned", "Expense report returned: Chicago trip", kwame.clone(), ago(c, 29, 10),
        "Hi Alex, I've sent your Chicago expense report back: the hotel receipt for the second night is missing. Attach it and resubmit and I'll approve it the same day.",
        &["kind:work", "topic:expense", "person:kwame", "city:chicago"]));
}

fn studio(c: &mut Corpus) {
    let julia = person("julia").addr();
    c.push(Thread::new(STUDIO, "p-fernhill-final", "Fernhill logo — final files")
        .tags(&["plant:fernhill-final", "kind:client", "topic:logo", "person:julia"])
        .msg(Msg::new(me(STUDIO), vec![julia.clone()], ago(c, 65, 11), "Hi Julia, here are the final logo files in every format, plus the one-page usage guide.")
            .att(Att::pdf("Fernhill-logo-usage.pdf", 800_000)).att(Att("Fernhill-logo-final.zip".into(), "application/zip", 14_000_000)))
        .msg(Msg::new(julia.clone(), vec![me(STUDIO)], ago(c, 64, 9), "We love it. Approved! Printing the new bags this week.").reply()));

    c.push(Thread::new(STUDIO, "p-bianchi-late", "Re: Invoice MS-2026-007 — Bianchi Wines")
        .tags(&["plant:bianchi-late", "kind:invoice", "person:marco", "invoice:ms-2026-007"])
        .msg(Msg::new(person("marco").addr(), vec![me(STUDIO)], ago(c, 10, 16),
            "Ciao Alex, sorry for the delay on this one. Harvest has been chaos. Our accountant will send the transfer next Tuesday. Grazie for your patience! Marco")));

    c.push(one(STUDIO, "castillo-quote", "Presupuesto para la nueva web", person("irene").addr(), ago(c, 38, 12),
        "Hola Alex:\n\n¿Nos podrías enviar un presupuesto para rediseñar la web de Casa Castillo? Necesitamos reservas online, carta en tres idiomas y una galería de fotos. Nos gustaría tenerla lista antes de la temporada de verano.\n\nUn saludo,\nIrene Castillo",
        &["kind:client", "lang:es", "person:irene", "topic:website"]));

    c.push(one(STUDIO, "domain", "morenostudio.example expires in 14 days", addr("NameHarbor", "renewals@nameharbor.example"), ago(c, 12, 7),
        "Your registration for morenostudio.example expires in 14 days. Auto-renew is off. Renew now to keep your site and email working.",
        &["kind:bill", "merchant:nameharbor", "topic:domain"]));
}

fn bulk(c: &mut Corpus) {
    let pp = NEWSLETTERS.iter().find(|n| n.key == "panpantry").unwrap();
    let text = "The no-knead sourdough method\n\nMix, rest overnight, fold twice, bake in a hot Dutch oven. Twelve hours of waiting, five minutes of work, and a crackling crust.";
    c.push(
        Thread::new(
            PERSONAL,
            "p-sourdough",
            "Pan & Pantry: The no-knead sourdough method",
        )
        .tags(&["plant:sourdough", "kind:newsletter", "newsletter:panpantry"])
        .msg(
            Msg::new(pp.addr(), vec![me(PERSONAL)], c.days_ago(80, 6, 5), text)
                .html(to_html(text))
                .bulk()
                .label("CATEGORY_UPDATES"),
        ),
    );
    let sw = NEWSLETTERS
        .iter()
        .find(|n| n.key == "strideweekly")
        .unwrap();
    let text = "Taper week: what to do before race day\n\nCut your mileage by half, keep a little speed, sleep more, and don't try new shoes. Your legs will thank you at mile 20.";
    c.push(
        Thread::new(
            PERSONAL,
            "p-taper",
            "Stride Weekly: Taper week, what to do before race day",
        )
        .tags(&["plant:taper", "kind:newsletter", "newsletter:strideweekly"])
        .msg(
            Msg::new(sw.addr(), vec![me(PERSONAL)], c.days_ago(140, 6, 5), text)
                .html(to_html(text))
                .bulk()
                .label("CATEGORY_UPDATES"),
        ),
    );
}
