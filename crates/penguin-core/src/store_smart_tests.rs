//! Smart views on a fictional mailbox: facts are stored directly (the
//! extractors have their own tests) except in the end-to-end test, which
//! runs the real scanner.

use chrono::NaiveDate;
use rusqlite::params;

use super::*;
use crate::structured::{Bill, Flight, Lodging, Money, Order, Reservation, Shipment};
use crate::types::*;
use crate::Store;

const ME: &str = "sam@okafor.example";
const OTHER: &str = "sam@northwind.example";

fn ms(y: i32, m: u32, d: u32) -> i64 {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis()
}

/// "Today" in these tests: Sunday, Sep 27, 2026 (UTC).
fn now() -> i64 {
    ms(2026, 9, 27)
}

fn usd(v: f64) -> Option<Money> {
    Some(Money {
        value: v,
        currency: "USD".into(),
    })
}

fn msg(
    account: &str,
    id: &str,
    date: i64,
    from: (&str, &str),
    subject: &str,
    labels: &[&str],
) -> Message {
    Message {
        account_id: account.into(),
        id: id.into(),
        thread_id: format!("t-{id}"),
        date,
        from: Address {
            name: Some(from.0.into()),
            email: from.1.into(),
        },
        to: vec![Address {
            name: None,
            email: account.into(),
        }],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: subject.into(),
        snippet: subject.into(),
        body_text: subject.into(),
        body_html: None,
        label_ids: labels.iter().map(|l| l.to_string()).collect(),
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    }
}

/// Store a message and one fact for it, as the scanner would.
fn put(store: &Store, m: Message, key: &str, fact: Extracted) {
    let (account, id) = (m.account_id.clone(), m.id.clone());
    store.upsert_messages(&[m]).unwrap();
    store
        .write(|tx| {
            let (rowid, date): (i64, i64) = tx.query_row(
                "SELECT rowid, date FROM messages WHERE account_id = ?1 AND id = ?2",
                params![account, id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let money = fact.money().cloned();
            tx.execute(
                "INSERT INTO extracted(msg, ord, kind, account_id, date, at, key, ref, amount, currency, source, data)
                 VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 3, ?10)",
                params![
                    rowid,
                    fact.kind(),
                    account,
                    date,
                    fact.when(),
                    key,
                    fact.reference(),
                    money.as_ref().map(|m| m.value),
                    money.as_ref().map(|m| m.currency.clone()),
                    serde_json::to_string(&fact).unwrap(),
                ],
            )?;
            tx.execute("UPDATE messages SET extracted = 1 WHERE rowid = ?1", [rowid])?;
            Ok(())
        })
        .unwrap();
}

fn order(merchant: &str, number: Option<&str>, total: f64, currency: &str) -> Extracted {
    Extracted::Order(Order {
        merchant: Some(merchant.into()),
        order_number: number.map(String::from),
        total: Some(Money {
            value: total,
            currency: currency.into(),
        }),
        ..Order::default()
    })
}

fn q() -> ListQuery {
    ListQuery {
        view: MailboxView::Smart(String::new()),
        tab: None,
        account_id: None,
        account_ids: None,
        limit: 100,
        before: None,
        unread_only: false,
        split: None,
    }
}

fn list(store: &Store, view: &str, scope: Option<&[AccountId]>) -> Vec<ThreadSummary> {
    store.list_smart(view, scope, &q(), now(), 0).unwrap()
}

fn titles(rows: &[ThreadSummary]) -> Vec<String> {
    rows.iter()
        .map(|r| r.smart.as_ref().unwrap().title.clone())
        .collect()
}

fn groups(rows: &[ThreadSummary]) -> Vec<String> {
    rows.iter()
        .map(|r| r.smart.as_ref().unwrap().group.clone().unwrap_or_default())
        .collect()
}

fn count(store: &Store, view: &str) -> u32 {
    store
        .smart_counts(&[view.to_string()], None, now(), 0)
        .unwrap()[0]
        .count
}

#[test]
fn ids_are_known_and_views_are_off_the_thread_view_table() {
    for id in SMART_VIEWS {
        assert!(is_smart_view(id), "{id}");
    }
    assert!(is_smart_view("files:pdf"));
    assert!(!is_smart_view("files:exe"));
    assert!(!is_smart_view("receipts:pdf"));
    assert!(!is_smart_view("inbox"));
    // The JSON the UI sends.
    let v: MailboxView = serde_json::from_str(r#"{"kind":"smart","labelId":"receipts"}"#).unwrap();
    assert_eq!(v, MailboxView::Smart("receipts".into()));
    let v: MailboxView =
        serde_json::from_str(r#"{"kind":"query","labelId":"from:priya"}"#).unwrap();
    assert_eq!(v, MailboxView::Query("from:priya".into()));
    // An unknown id is an empty list, not an error.
    let store = Store::open_in_memory().unwrap();
    assert!(list(&store, "horoscopes", None).is_empty());
}

#[test]
fn receipts_count_one_order_once_and_total_this_month_per_currency() {
    let store = Store::open_in_memory().unwrap();
    let rydeo = ("Rydeo Receipts", "receipts@rydeo.example");
    let paper = ("Paperleaf", "orders@paperleaf.example");
    put(
        &store,
        msg(
            ME,
            "r1",
            ms(2026, 9, 3),
            rydeo,
            "Your Tuesday trip",
            &["INBOX"],
        ),
        "rydeo",
        order("Rydeo", None, 23.40, "USD"),
    );
    put(
        &store,
        msg(ME, "r2", ms(2026, 9, 12), rydeo, "Your Friday trip", &[]),
        "rydeo",
        order("Rydeo", None, 9.10, "USD"),
    );
    // Paperleaf: the confirmation and the receipt of one order.
    put(
        &store,
        msg(
            ME,
            "p1",
            ms(2026, 9, 14),
            paper,
            "Order PL-2291 confirmed",
            &[],
        ),
        "paperleaf",
        order("Paperleaf", Some("PL-2291"), 64.00, "USD"),
    );
    put(
        &store,
        msg(
            ME,
            "p2",
            ms(2026, 9, 15),
            paper,
            "Your receipt for PL-2291",
            &[],
        ),
        "paperleaf",
        order("Paperleaf", Some("PL-2291"), 64.00, "USD"),
    );
    // A euro receipt, last month's, one in Trash, one in another account.
    put(
        &store,
        msg(
            ME,
            "e1",
            ms(2026, 9, 20),
            ("Café Lumo", "hola@lumo.example"),
            "Recibo",
            &[],
        ),
        "lumo",
        order("Café Lumo", None, 30.00, "EUR"),
    );
    put(
        &store,
        msg(ME, "old", ms(2026, 8, 30), rydeo, "Your Sunday trip", &[]),
        "rydeo",
        order("Rydeo", None, 12.00, "USD"),
    );
    put(
        &store,
        msg(ME, "bin", ms(2026, 9, 21), rydeo, "Your trip", &["TRASH"]),
        "rydeo",
        order("Rydeo", None, 99.00, "USD"),
    );
    put(
        &store,
        msg(
            OTHER,
            "w1",
            ms(2026, 9, 22),
            paper,
            "Order PL-9 confirmed",
            &[],
        ),
        "paperleaf",
        order("Paperleaf", Some("PL-9"), 5.00, "USD"),
    );

    let mine = vec![ME.to_string()];
    let rows = list(&store, "receipts", Some(&mine));
    assert_eq!(
        titles(&rows),
        ["Café Lumo", "Paperleaf", "Rydeo", "Rydeo", "Rydeo"]
    );
    assert_eq!(
        groups(&rows),
        [
            "September 2026",
            "September 2026",
            "September 2026",
            "September 2026",
            "August 2026"
        ]
    );
    // The newest email of the order stands for it.
    assert_eq!(rows[1].thread_id, "t-p2");
    assert_eq!(
        rows[1].smart.as_ref().unwrap().reference.as_deref(),
        Some("PL-2291")
    );
    assert_eq!(rows[1].last_date, ms(2026, 9, 15));

    let info = store
        .smart_view_info("receipts", Some(&mine), now(), 0)
        .unwrap();
    let this = &info.stats[0];
    assert_eq!(this.label, "This month");
    assert_eq!(
        this.amounts,
        vec![
            usd(96.5).unwrap(),
            Money {
                value: 30.0,
                currency: "EUR".into()
            }
        ]
    );
    assert_eq!(info.stats[1].value.as_deref(), Some("4"));
    assert_eq!(info.stats[2].amounts, vec![usd(12.0).unwrap()]);
    assert_eq!(
        store
            .smart_counts(&["receipts".into()], Some(&mine), now(), 0)
            .unwrap()[0]
            .count,
        4
    );
    // Every account: the other account's order too.
    assert_eq!(list(&store, "receipts", None).len(), 6);
    // An empty scope (a profile with no accounts) is empty.
    assert!(list(&store, "receipts", Some(&[])).is_empty());
}

fn flight(from: &str, to: &str, number: &str, conf: &str, depart: &str, arrive: &str) -> Extracted {
    Extracted::Flight(Flight {
        airline: Some("TAP Air Portugal".into()),
        airline_code: Some("TP".into()),
        flight_number: Some(number.into()),
        confirmation: Some(conf.into()),
        depart_airport: Some(from.into()),
        arrive_airport: Some(to.into()),
        depart_time: Some(depart.into()),
        arrive_time: Some(arrive.into()),
        ..Flight::default()
    })
}

#[test]
fn travel_lists_upcoming_trips_first_then_the_past() {
    let store = Store::open_in_memory().unwrap();
    let tap = ("TAP Air Portugal", "reservas@flytap.example");
    put(
        &store,
        msg(ME, "f1", ms(2026, 9, 1), tap, "Your trip to Lisbon", &[]),
        "tap air portugal",
        flight(
            "SFO",
            "LIS",
            "TP 238",
            "QX7P2K",
            "2026-10-02T19:05",
            "2026-10-03T14:40",
        ),
    );
    // The same booking sent again after a schedule change: one row.
    put(
        &store,
        msg(
            ME,
            "f1b",
            ms(2026, 9, 10),
            tap,
            "Schedule change: your trip to Lisbon",
            &[],
        ),
        "tap air portugal",
        flight(
            "SFO",
            "LIS",
            "TP 238",
            "QX7P2K",
            "2026-10-02T19:15",
            "2026-10-03T14:50",
        ),
    );
    put(
        &store,
        msg(ME, "f2", ms(2026, 9, 1), tap, "Your return flight", &[]),
        "tap air portugal",
        flight(
            "LIS",
            "SFO",
            "TP 237",
            "QX7P2K",
            "2026-10-09T11:00",
            "2026-10-09T15:00",
        ),
    );
    put(
        &store,
        msg(
            ME,
            "h1",
            ms(2026, 9, 2),
            ("Casa Alfama", "stay@casa-alfama.example"),
            "Booking confirmed",
            &[],
        ),
        "casa alfama",
        Extracted::Lodging(Lodging {
            name: Some("Casa Alfama".into()),
            checkin: Some("2026-10-03".into()),
            checkout: Some("2026-10-09".into()),
            confirmation: Some("CA-551".into()),
            ..Lodging::default()
        }),
    );
    // A train later in the year: its own group.
    put(
        &store,
        msg(
            ME,
            "tr",
            ms(2026, 9, 5),
            ("Railo", "tickets@railo.example"),
            "Your train tickets",
            &[],
        ),
        "railo",
        Extracted::Reservation(Reservation {
            category: "train".into(),
            name: Some("Porto → Lisbon".into()),
            start: Some("2026-11-14T08:10".into()),
            confirmation: Some("RL-88".into()),
            ..Reservation::default()
        }),
    );
    // A restaurant booking is not travel.
    put(
        &store,
        msg(
            ME,
            "rest",
            ms(2026, 9, 5),
            ("Mesa", "book@mesa.example"),
            "Table booked",
            &[],
        ),
        "mesa",
        Extracted::Reservation(Reservation {
            category: "restaurant".into(),
            name: Some("Mesa".into()),
            start: Some("2026-10-01T20:00".into()),
            party_size: Some(2),
            ..Reservation::default()
        }),
    );
    put(
        &store,
        msg(ME, "past", ms(2026, 6, 1), tap, "Your trip to Boston", &[]),
        "tap air portugal",
        flight(
            "SFO",
            "BOS",
            "TP 900",
            "ZZ1111",
            "2026-06-10T07:00",
            "2026-06-10T15:30",
        ),
    );

    let rows = list(&store, "travel", None);
    assert_eq!(
        titles(&rows),
        [
            "SFO \u{2192} LIS",
            "Casa Alfama",
            "LIS \u{2192} SFO",
            "Porto → Lisbon",
            "SFO \u{2192} BOS"
        ]
    );
    assert_eq!(
        groups(&rows),
        [
            "Trip to Lisbon \u{b7} Oct 2\u{2013}9",
            "Trip to Lisbon \u{b7} Oct 2\u{2013}9",
            "Trip to Lisbon \u{b7} Oct 2\u{2013}9",
            "Upcoming \u{b7} Nov 14",
            "Past",
        ]
    );
    let first = rows[0].smart.as_ref().unwrap();
    // The re-sent confirmation wins (newest email): the new time.
    assert_eq!(rows[0].thread_id, "t-f1b");
    assert_eq!(first.at.as_deref(), Some("2026-10-02T19:15"));
    assert_eq!(first.reference.as_deref(), Some("QX7P2K"));
    assert_eq!(
        first.detail.as_deref(),
        Some("TP 238 \u{b7} TAP Air Portugal")
    );
    assert_eq!(first.status.as_deref(), Some("upcoming"));
    assert_eq!(
        rows[4].smart.as_ref().unwrap().status.as_deref(),
        Some("past")
    );
    assert_eq!(count(&store, "travel"), 4);
    let info = store.smart_view_info("travel", None, now(), 0).unwrap();
    assert_eq!(info.stats[0].value.as_deref(), Some("4"));
    assert_eq!(info.stats[1].value.as_deref(), Some("SFO \u{2192} LIS"));

    let rows = list(&store, "reservations", None);
    assert_eq!(titles(&rows), ["Mesa"]);
    assert_eq!(
        rows[0].smart.as_ref().unwrap().detail.as_deref(),
        Some("Table for 2")
    );
    assert_eq!(count(&store, "reservations"), 1);
}

fn parcel(tracking: &str, status: &str, expected: Option<&str>) -> Extracted {
    Extracted::Shipment(Shipment {
        carrier: Some("UPS".into()),
        tracking_number: Some(tracking.into()),
        verified: true,
        status: Some(status.into()),
        expected: expected.map(String::from),
        merchant: Some("Hearth & Loom".into()),
        ..Shipment::default()
    })
}

#[test]
fn packages_follow_the_newest_news_per_tracking_number() {
    let store = Store::open_in_memory().unwrap();
    let ups = ("UPS", "mcinfo@ups.example");
    put(
        &store,
        msg(
            ME,
            "s1",
            ms(2026, 9, 24),
            ups,
            "Your package has shipped",
            &[],
        ),
        "hearth & loom",
        parcel("1Z999AA10123456784", "shipped", Some("2026-09-29")),
    );
    put(
        &store,
        msg(ME, "s2", ms(2026, 9, 27), ups, "Out for delivery", &[]),
        "hearth & loom",
        parcel("1Z999AA10123456784", "outForDelivery", Some("2026-09-27")),
    );
    put(
        &store,
        msg(ME, "d1", ms(2026, 9, 25), ups, "Delivered", &[]),
        "hearth & loom",
        parcel("1Z999AA10198765430", "delivered", None),
    );
    put(
        &store,
        msg(ME, "n1", ms(2026, 9, 26), ups, "Shipped", &[]),
        "hearth & loom",
        parcel("1Z999AA10111111111", "inTransit", Some("2026-10-01")),
    );
    // No news for a month: not "on the way".
    put(
        &store,
        msg(ME, "st", ms(2026, 8, 20), ups, "Shipped", &[]),
        "hearth & loom",
        parcel("1Z999AA10122222222", "inTransit", Some("2026-08-25")),
    );

    let rows = list(&store, "packages", None);
    let st: Vec<String> = rows
        .iter()
        .map(|r| r.smart.as_ref().unwrap().status.clone().unwrap())
        .collect();
    assert_eq!(
        st,
        ["outForDelivery", "inTransit", "delivered", "inTransit"]
    );
    assert_eq!(
        groups(&rows),
        ["On the way", "On the way", "Delivered", "Earlier"]
    );
    assert_eq!(rows[0].thread_id, "t-s2");
    assert_eq!(rows[0].smart.as_ref().unwrap().title, "Hearth & Loom");
    assert_eq!(
        rows[0].smart.as_ref().unwrap().detail.as_deref(),
        Some("UPS")
    );
    assert_eq!(count(&store, "packages"), 2);
    let info = store.smart_view_info("packages", None, now(), 0).unwrap();
    assert_eq!(info.stats[0].value.as_deref(), Some("2"));
    assert_eq!(info.stats[1].label, "Arriving today");
}

fn bill(
    biller: &str,
    invoice: Option<&str>,
    amount: f64,
    due: Option<&str>,
    status: Option<&str>,
) -> Extracted {
    Extracted::Bill(Bill {
        biller: Some(biller.into()),
        invoice_number: invoice.map(String::from),
        amount_due: usd(amount),
        due_date: due.map(String::from),
        status: status.map(String::from),
        ..Bill::default()
    })
}

#[test]
fn bills_are_unpaid_until_a_payment_email_says_otherwise() {
    let store = Store::open_in_memory().unwrap();
    let bw = ("Brightwave Energy", "billing@brightwave.example");
    let ledg = ("Ledgerly", "invoices@ledgerly.example");
    put(
        &store,
        msg(ME, "b1", ms(2026, 9, 10), bw, "Your September bill", &[]),
        "brightwave energy",
        bill(
            "Brightwave Energy",
            Some("BW-0925"),
            84.20,
            Some("2026-10-05"),
            Some("due"),
        ),
    );
    put(
        &store,
        msg(ME, "b2", ms(2026, 9, 1), ledg, "Invoice INV-311", &[]),
        "ledgerly",
        bill(
            "Ledgerly",
            Some("INV-311"),
            240.00,
            Some("2026-09-20"),
            Some("due"),
        ),
    );
    // Paid: a later "payment received" for the same invoice.
    put(
        &store,
        msg(
            ME,
            "b3",
            ms(2026, 9, 2),
            ("Aquafon", "billing@aquafon.example"),
            "Water bill",
            &[],
        ),
        "aquafon",
        bill(
            "Aquafon",
            Some("AQ-77"),
            41.00,
            Some("2026-09-25"),
            Some("due"),
        ),
    );
    put(
        &store,
        msg(
            ME,
            "b3p",
            ms(2026, 9, 20),
            ("Aquafon", "billing@aquafon.example"),
            "Payment received",
            &[],
        ),
        "aquafon",
        bill("Aquafon", Some("AQ-77"), 41.00, None, Some("paid")),
    );
    // A reminder for the Brightwave bill: one row, not two.
    put(
        &store,
        msg(
            ME,
            "b1r",
            ms(2026, 9, 26),
            bw,
            "Reminder: your bill is due",
            &[],
        ),
        "brightwave energy",
        bill(
            "Brightwave Energy",
            Some("BW-0925"),
            84.20,
            Some("2026-10-05"),
            Some("due"),
        ),
    );
    // Long past due with no payment email (autopay): listed, not flagged.
    put(
        &store,
        msg(
            ME,
            "b4",
            ms(2026, 5, 1),
            ("Telco", "bills@telco.example"),
            "May bill",
            &[],
        ),
        "telco",
        bill("Telco", Some("T-5"), 55.00, Some("2026-05-20"), Some("due")),
    );

    let rows = list(&store, "bills", None);
    assert_eq!(
        titles(&rows),
        ["Ledgerly", "Brightwave Energy", "Aquafon", "Telco"]
    );
    let st: Vec<String> = rows
        .iter()
        .map(|r| r.smart.as_ref().unwrap().status.clone().unwrap())
        .collect();
    assert_eq!(st, ["overdue", "due", "paid", "unpaid"]);
    assert_eq!(groups(&rows), ["Overdue", "Due", "Paid", "Earlier"]);
    assert_eq!(rows[1].thread_id, "t-b1r");
    assert_eq!(count(&store, "bills"), 2);
    let info = store.smart_view_info("bills", None, now(), 0).unwrap();
    assert_eq!(info.stats[0].amounts, vec![usd(324.2).unwrap()]);
    assert_eq!(info.stats[1].label, "Overdue");
    assert_eq!(info.stats[1].tone.as_deref(), Some("warn"));
    assert_eq!(
        info.stats[2].value.as_deref(),
        Some("Brightwave Energy \u{b7} Oct 5")
    );
}

#[test]
fn subscriptions_are_steady_repeat_charges_from_one_merchant() {
    let store = Store::open_in_memory().unwrap();
    let tune = ("Tunely", "receipts@tunely.example");
    // Monthly $9.99, four times.
    for (i, (m, d)) in [(6, 3), (7, 3), (8, 3), (9, 3)].iter().enumerate() {
        put(
            &store,
            msg(
                ME,
                &format!("tn{i}"),
                ms(2026, *m, *d),
                tune,
                "Your Tunely receipt",
                &[],
            ),
            "tunely",
            order("Tunely", Some(&format!("TN-{i}")), 9.99, "USD"),
        );
    }
    // Rides at irregular times: not a subscription.
    for (i, (m, d)) in [(8, 2), (8, 5), (8, 19), (9, 1), (9, 22)]
        .iter()
        .enumerate()
    {
        put(
            &store,
            msg(
                ME,
                &format!("ry{i}"),
                ms(2026, *m, *d),
                ("Rydeo", "receipts@rydeo.example"),
                "Your trip",
                &[],
            ),
            "rydeo",
            order("Rydeo", None, 10.0 + i as f64, "USD"),
        );
    }
    // Yearly, twice.
    put(
        &store,
        msg(
            ME,
            "y1",
            ms(2025, 3, 10),
            ("Cloudbox", "billing@cloudbox.example"),
            "Receipt",
            &[],
        ),
        "cloudbox",
        order("Cloudbox", Some("CB-1"), 99.0, "USD"),
    );
    put(
        &store,
        msg(
            ME,
            "y2",
            ms(2026, 3, 10),
            ("Cloudbox", "billing@cloudbox.example"),
            "Receipt",
            &[],
        ),
        "cloudbox",
        order("Cloudbox", Some("CB-2"), 99.0, "USD"),
    );
    // Monthly that stopped in May.
    for (i, m) in [2, 3, 4, 5].iter().enumerate() {
        put(
            &store,
            msg(
                ME,
                &format!("gy{i}"),
                ms(2026, *m, 15),
                ("Gymly", "hello@gymly.example"),
                "Membership",
                &[],
            ),
            "gymly",
            order("Gymly", Some(&format!("GY-{i}")), 30.0, "USD"),
        );
    }
    // Monthly, but the amount jumps around: a shop, not a plan.
    for (i, (m, v)) in [(6, 12.0), (7, 80.0), (8, 25.0), (9, 140.0)]
        .iter()
        .enumerate()
    {
        put(
            &store,
            msg(
                ME,
                &format!("sh{i}"),
                ms(2026, *m, 8),
                ("Grocer", "orders@grocer.example"),
                "Order",
                &[],
            ),
            "grocer",
            order("Grocer", Some(&format!("GR-{i}")), *v, "USD"),
        );
    }

    let rows = list(&store, "subscriptions", None);
    assert_eq!(titles(&rows), ["Tunely", "Cloudbox", "Gymly"]);
    let s: Vec<&SmartRow> = rows.iter().map(|r| r.smart.as_ref().unwrap()).collect();
    assert_eq!(s[0].detail.as_deref(), Some("Monthly \u{b7} 4 charges"));
    assert_eq!(s[0].at.as_deref(), Some("2026-09-03"));
    assert_eq!(s[0].end.as_deref(), Some("2026-10-03"));
    assert_eq!(s[0].status.as_deref(), Some("active"));
    assert_eq!(s[1].detail.as_deref(), Some("Yearly \u{b7} 2 charges"));
    assert_eq!(s[2].status.as_deref(), Some("stopped"));
    assert_eq!(groups(&rows), ["Active", "Active", "Stopped?"]);
    assert_eq!(count(&store, "subscriptions"), 2);
    let info = store
        .smart_view_info("subscriptions", None, now(), 0)
        .unwrap();
    // 9.99 + 99/12
    assert_eq!(info.stats[1].amounts, vec![usd(18.24).unwrap()]);
}

#[test]
fn people_you_know_codes_files_newsletters_and_saved_searches() {
    let store = Store::open_in_memory().unwrap();
    let priya = ("Priya Raman", "priya@linden.example");
    // You wrote to Priya once.
    let mut sent = msg(
        ME,
        "s1",
        ms(2026, 8, 1),
        ("Sam Okafor", ME),
        "Lunch?",
        &["SENT"],
    );
    sent.to = vec![Address {
        name: None,
        email: "Priya@Linden.example".into(),
    }];
    sent.thread_id = "t-lunch".into();
    let mut from_priya = msg(
        ME,
        "p1",
        ms(2026, 9, 25),
        priya,
        "Re: Lunch?",
        &["INBOX", "UNREAD"],
    );
    from_priya.thread_id = "t-lunch".into();
    let stranger = msg(
        ME,
        "x1",
        ms(2026, 9, 26),
        ("Stranger", "hi@cold-outreach.example"),
        "Quick question",
        &["INBOX"],
    );
    let mut news = msg(
        ME,
        "n1",
        ms(2026, 9, 26),
        ("Linden Digest", "digest@linden.example"),
        "This week at Linden",
        &["INBOX"],
    );
    news.list_unsubscribe = Some("<https://linden.example/unsub>".into());
    let mut code = msg(
        ME,
        "c1",
        now() - 3_600_000,
        ("Rydeo", "no-reply@rydeo.example"),
        "Your Rydeo verification code",
        &["INBOX"],
    );
    code.body_text = "Your verification code is 482913. It expires in 10 minutes.".into();
    let mut old_code = msg(
        ME,
        "c2",
        now() - 5 * DAY,
        ("Rydeo", "no-reply@rydeo.example"),
        "Your Rydeo verification code",
        &["INBOX"],
    );
    old_code.body_text = "Your verification code is 771204.".into();
    let mut pdf = msg(ME, "f1", ms(2026, 9, 20), priya, "Lease draft", &[]);
    pdf.attachments = vec![
        AttachmentMeta {
            id: "a1".into(),
            filename: "photo.jpg".into(),
            mime_type: "image/jpeg".into(),
            size: 1000,
            content_id: None,
            inline: false,
        },
        AttachmentMeta {
            id: "a2".into(),
            filename: "lease-2026.pdf".into(),
            mime_type: "application/pdf".into(),
            size: 2000,
            content_id: None,
            inline: false,
        },
    ];
    let mut sheet = msg(ME, "f2", ms(2026, 9, 21), priya, "Budget", &["UNREAD"]);
    sheet.attachments = vec![AttachmentMeta {
        id: "a3".into(),
        filename: "budget.xlsx".into(),
        mime_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
        size: 3000,
        content_id: None,
        inline: false,
    }];
    store
        .upsert_messages(&[sent, from_priya, stranger, news, code, old_code, pdf, sheet])
        .unwrap();
    // Ingest scans codes only in mail from the real clock's last 48 hours;
    // the app's startup backfill covers the rest. The test's clock is fixed
    // (now()), so run that backfill as the app would, or it rots with time.
    store.backfill_otp(now() - 30 * DAY).unwrap();

    let people = list(&store, "people", None);
    assert_eq!(
        people
            .iter()
            .map(|t| t.thread_id.as_str())
            .collect::<Vec<_>>(),
        ["t-lunch"]
    );
    assert!(people[0].smart.is_none());

    let codes = list(&store, "codes", None);
    assert_eq!(
        codes
            .iter()
            .map(|t| t.thread_id.as_str())
            .collect::<Vec<_>>(),
        ["t-c1"]
    );
    assert_eq!(
        codes[0].otp.as_ref().and_then(|o| o.code.as_deref()),
        Some("482913")
    );
    assert_eq!(count(&store, "codes"), 1);

    let files = list(&store, "files", None);
    assert_eq!(
        files
            .iter()
            .map(|t| t.thread_id.as_str())
            .collect::<Vec<_>>(),
        ["t-f2", "t-f1"]
    );
    let pdfs = list(&store, "files:pdf", None);
    assert_eq!(titles(&pdfs), ["lease-2026.pdf"]);
    assert_eq!(
        pdfs[0].smart.as_ref().unwrap().detail.as_deref(),
        Some("+1 more")
    );
    assert_eq!(titles(&list(&store, "files:sheets", None)), ["budget.xlsx"]);
    assert!(list(&store, "files:docs", None).is_empty());
    // Counts: unread conversations among the rows, read without titles.
    assert_eq!(count(&store, "files"), 1);
    assert_eq!(count(&store, "files:sheets"), 1);
    assert_eq!(count(&store, "files:pdf"), 0);
    assert_eq!(count(&store, "files:docs"), 0);
    assert_eq!(
        count(&store, "people") as usize,
        people.iter().filter(|t| t.unread).count()
    );

    let news = list(&store, "newsletters", None);
    assert_eq!(
        news.iter()
            .map(|t| t.thread_id.as_str())
            .collect::<Vec<_>>(),
        ["t-n1"]
    );

    let saved = store
        .list_saved_search(
            "from:priya",
            None,
            &ListQuery {
                view: MailboxView::Query("from:priya".into()),
                ..q()
            },
        )
        .unwrap();
    assert_eq!(
        saved
            .iter()
            .map(|t| t.thread_id.as_str())
            .collect::<Vec<_>>(),
        ["t-lunch", "t-f2", "t-f1"]
    );
    let unread = store
        .smart_counts(
            &["query:from:priya".into(), "people".into()],
            None,
            now(),
            0,
        )
        .unwrap();
    // from:priya: the lunch thread and the unread budget sheet.
    assert_eq!(unread.iter().map(|c| c.count).collect::<Vec<_>>(), [2, 1]);
    // Through list_threads, as the app calls it.
    let via = store
        .list_threads(&ListQuery {
            view: MailboxView::Query("from:priya has:attachment".into()),
            ..q()
        })
        .unwrap();
    assert_eq!(via.len(), 2);
    assert!(store
        .list_threads(&ListQuery {
            view: MailboxView::Query("".into()),
            ..q()
        })
        .is_err());
}

#[test]
fn facts_from_the_real_scanner_reach_the_views() {
    let store = Store::open_in_memory().unwrap();
    let mut m = msg(
        ME,
        "rc",
        ms(2026, 9, 21),
        ("Rydeo Receipts", "receipts@rydeo.example"),
        "Your Monday trip with Rydeo",
        &["INBOX"],
    );
    m.body_text = "Thanks for riding\nTrip fare $18.20\nTotal $23.40".into();
    store.upsert_messages(&[m]).unwrap();
    // Before the scanner: nothing yet, and the header says it's still reading.
    assert!(list(&store, "receipts", None).is_empty());
    let info = store.smart_view_info("receipts", None, now(), 0).unwrap();
    assert!(info
        .note
        .as_deref()
        .unwrap()
        .starts_with("Still reading 1 email"));
    store.extract_pending(100).unwrap();
    let rows = list(&store, "receipts", None);
    assert_eq!(titles(&rows), ["Rydeo"]);
    assert_eq!(rows[0].smart.as_ref().unwrap().amount, usd(23.40));
    assert!(store
        .smart_view_info("receipts", None, now(), 0)
        .unwrap()
        .note
        .is_none());
}
