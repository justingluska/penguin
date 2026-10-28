//! Send later and reminders across devices, through the real store: a Mac
//! and an iPhone (each a Store) sharing penguin-sync's in-memory server,
//! which checks change tags like CloudKit. The scheduler's question before
//! each send is `scheduled_send_turn`; these tests ask it the way
//! penguin-provider's `run_due` does and check that exactly one device
//! ever gets `Send`.

use penguin_sync::memory::MemoryServer;
use penguin_sync::outbox::{
    Outcome, ReminderBody, ScheduledSendBody, Turn, OWNER_FREE_MS, TAKEOVER_AFTER_MS,
};
use penguin_sync::{sync_once, thread_key, Kind};

use crate::types::*;
use crate::Store;

const A: &str = "ada@penguin.example";
const MIN: i64 = 60_000;
const T0: i64 = 1_760_000_000_000;
/// When the test schedules are due.
const DUE: i64 = T0 + 60 * MIN;

fn account(id: &str, provider: AccountProvider) -> Account {
    Account {
        id: id.into(),
        email: id.into(),
        color: "#123456".into(),
        added_at: 1,
        provider,
        ..Account::default()
    }
}

fn device_with(provider: AccountProvider) -> Store {
    let s = Store::open_in_memory().unwrap();
    s.upsert_account(&account(A, provider)).unwrap();
    s.set_sync_enabled(true).unwrap();
    s
}

fn device() -> Store {
    device_with(AccountProvider::Gmail)
}

fn sync(s: &Store, server: &MemoryServer, now: i64) {
    server.set_now(now);
    sync_once(s, &mut server.transport(), &|| now).unwrap();
}

fn schedule(s: &Store, id: &str, draft: &str) {
    s.add_scheduled_send(&ScheduledSend {
        id: id.into(),
        account_id: A.into(),
        draft_id: draft.into(),
        send_at: DUE,
        created_at: T0,
        remind_after_ms: None,
        attempts: 0,
        last_error: None,
        elsewhere: false,
    })
    .unwrap();
}

fn turn(s: &Store, id: &str, now: i64) -> Turn {
    s.scheduled_send_turn(A, id, now).unwrap()
}

fn is_send(t: &Turn) -> bool {
    *t == Turn::Send
}

fn rows(s: &Store) -> Vec<(String, bool)> {
    s.list_scheduled_sends(None)
        .unwrap()
        .into_iter()
        .map(|r| (r.id, r.elsewhere))
        .collect()
}

/// A Mac that scheduled s1, and a phone that has heard of it.
fn mac_and_phone() -> (Store, Store, MemoryServer) {
    let (mac, phone, server) = (device(), device(), MemoryServer::new());
    schedule(&mac, "s1", "d1");
    sync(&mac, &server, T0);
    sync(&phone, &server, T0);
    (mac, phone, server)
}

#[test]
fn a_schedule_shows_on_the_other_device_and_is_not_due_there_until_takeover() {
    let (_mac, phone, _server) = mac_and_phone();
    assert_eq!(rows(&phone), vec![("s1".to_string(), true)]);
    assert!(phone.due_scheduled_sends(DUE).unwrap().is_empty());
    assert_eq!(
        phone.next_outbox_due().unwrap(),
        Some(DUE + TAKEOVER_AFTER_MS)
    );
    assert_eq!(
        phone
            .due_scheduled_sends(DUE + TAKEOVER_AFTER_MS)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn the_owner_sends_on_time_and_the_other_device_forgets_it() {
    let (mac, phone, server) = mac_and_phone();
    assert!(is_send(&turn(&mac, "s1", DUE)));
    // The phone, asked at the same moment, may not.
    assert!(
        matches!(turn(&phone, "s1", DUE), Turn::Wait { until } if until == DUE + TAKEOVER_AFTER_MS)
    );
    mac.finish_scheduled_send(A, "s1", Outcome::Sent).unwrap();
    sync(&mac, &server, DUE + MIN);
    sync(&phone, &server, DUE + MIN);
    assert!(rows(&phone).is_empty());
    assert_eq!(turn(&phone, "s1", DUE + TAKEOVER_AFTER_MS), Turn::Gone);
}

#[test]
fn an_asleep_owner_is_taken_over_once_and_yields_when_it_wakes() {
    let (mac, phone, server) = mac_and_phone();
    let late = DUE + TAKEOVER_AFTER_MS + MIN;
    // The phone claims; it doesn't send before the server has its claim.
    assert!(matches!(
        turn(&phone, "s1", late),
        Turn::Claim { phase: 1, .. }
    ));
    assert!(matches!(turn(&phone, "s1", late), Turn::Wait { .. }));
    sync(&phone, &server, late);
    assert!(is_send(&turn(&phone, "s1", late + MIN)));

    // The Mac wakes before hearing of it: it is late, so it claims too…
    let woke = late + 2 * MIN;
    assert!(matches!(turn(&mac, "s1", woke), Turn::Claim { .. }));
    // …and its claim loses to the one the server stored.
    sync(&mac, &server, woke);
    assert!(matches!(turn(&mac, "s1", woke + MIN), Turn::Wait { .. }));

    phone.finish_scheduled_send(A, "s1", Outcome::Sent).unwrap();
    sync(&phone, &server, woke + MIN);
    sync(&mac, &server, woke + 2 * MIN);
    assert_eq!(turn(&mac, "s1", woke + 3 * MIN), Turn::Gone);
    assert!(rows(&mac).is_empty() && rows(&phone).is_empty());
}

#[test]
fn concurrent_claims_leave_exactly_one_sender_whichever_reaches_the_server_first() {
    for mac_first in [true, false] {
        let (mac, phone, server) = mac_and_phone();
        let late = DUE + TAKEOVER_AFTER_MS + MIN;
        // Both claim before either has synced.
        assert!(matches!(turn(&mac, "s1", late), Turn::Claim { .. }));
        assert!(matches!(turn(&phone, "s1", late), Turn::Claim { .. }));
        let (first, second) = if mac_first {
            (&mac, &phone)
        } else {
            (&phone, &mac)
        };
        sync(first, &server, late);
        sync(second, &server, late);
        // Each device asks again, several times, after more syncs.
        let mut senders = 0;
        for (i, d) in [&mac, &phone].into_iter().enumerate() {
            sync(d, &server, late + MIN);
            let sends = (0..3)
                .filter(|k| is_send(&turn(d, "s1", late + (2 + k) * MIN)))
                .count();
            if sends > 0 {
                senders += 1;
                assert_eq!(i == 0, mac_first, "the first to the server sends");
            }
        }
        assert_eq!(senders, 1, "mac_first = {mac_first}");
    }
}

#[test]
fn the_owner_late_without_ever_reaching_the_server_just_sends() {
    let (mac, server) = (device(), MemoryServer::new());
    schedule(&mac, "s1", "d1");
    // Never synced (iCloud not ready): nobody else can know of it.
    assert!(is_send(&turn(&mac, "s1", DUE + OWNER_FREE_MS + MIN)));
    // Once it is on the server, a late owner claims first.
    sync(&mac, &server, T0);
    assert!(matches!(
        turn(&mac, "s1", DUE + OWNER_FREE_MS + MIN),
        Turn::Claim { .. }
    ));
}

#[test]
fn mail_whose_send_cannot_be_checked_is_only_sent_by_its_owner() {
    let (mac, phone, server) = (
        device_with(AccountProvider::Imap),
        device_with(AccountProvider::Imap),
        MemoryServer::new(),
    );
    schedule(&mac, "s1", "d1");
    sync(&mac, &server, T0);
    sync(&phone, &server, T0);
    let much_later = DUE + 24 * 60 * MIN;
    assert!(matches!(turn(&phone, "s1", much_later), Turn::Wait { .. }));
    assert!(
        is_send(&turn(&mac, "s1", much_later)),
        "no claim to wait for"
    );
}

#[test]
fn with_sync_off_another_devices_schedule_never_sends_here() {
    let (_mac, phone, _server) = mac_and_phone();
    phone.set_sync_enabled(false).unwrap();
    assert!(matches!(
        turn(&phone, "s1", DUE + 10 * TAKEOVER_AFTER_MS),
        Turn::Wait { .. }
    ));
}

#[test]
fn a_cancel_on_either_device_reaches_the_other() {
    let (mac, phone, server) = mac_and_phone();
    assert!(phone.delete_scheduled_send_by_id("s1").unwrap());
    sync(&phone, &server, T0 + MIN);
    sync(&mac, &server, T0 + MIN);
    assert!(rows(&mac).is_empty());
    assert_eq!(turn(&mac, "s1", DUE), Turn::Gone);

    schedule(&phone, "s2", "d2");
    sync(&phone, &server, T0 + 2 * MIN);
    sync(&mac, &server, T0 + 2 * MIN);
    assert_eq!(rows(&mac), vec![("s2".to_string(), true)]);
    assert!(mac.delete_scheduled_sends_for_draft(A, "d2").unwrap() == 1);
    sync(&mac, &server, T0 + 3 * MIN);
    sync(&phone, &server, T0 + 3 * MIN);
    assert!(rows(&phone).is_empty());
}

#[test]
fn sent_beats_a_cancel_made_concurrently_so_it_is_never_resurrected() {
    let (mac, phone, server) = mac_and_phone();
    mac.finish_scheduled_send(A, "s1", Outcome::Sent).unwrap();
    // The phone, not yet told, cancels (a newer intent).
    phone.delete_scheduled_send_by_id("s1").unwrap();
    sync(&phone, &server, DUE + MIN);
    sync(&mac, &server, DUE + MIN);
    sync(&phone, &server, DUE + 2 * MIN);
    for d in [&mac, &phone] {
        let r = d.scheduled_send_record(A, "s1").unwrap().unwrap();
        let b: ScheduledSendBody = r.body_as().unwrap();
        assert_eq!(b.outcome, Some(Outcome::Sent));
    }
}

#[test]
fn rescheduling_cancels_the_old_schedule_everywhere() {
    let (mac, phone, server) = mac_and_phone();
    mac.replace_scheduled_sends(&ScheduledSend {
        id: "s9".into(),
        account_id: A.into(),
        draft_id: "d1".into(),
        send_at: DUE + 60 * MIN,
        created_at: T0,
        remind_after_ms: None,
        attempts: 0,
        last_error: None,
        elsewhere: false,
    })
    .unwrap();
    assert_eq!(rows(&mac), vec![("s9".to_string(), false)]);
    sync(&mac, &server, T0 + MIN);
    sync(&phone, &server, T0 + MIN);
    assert_eq!(rows(&phone), vec![("s9".to_string(), true)]);
}

#[test]
fn a_retry_keeps_the_owner_and_records_nothing() {
    let (_mac, phone, _server) = mac_and_phone();
    let mut row = phone.get_scheduled_send(A, "s1").unwrap().unwrap();
    let pending = phone.sync_pending_count().unwrap();
    row.attempts = 1;
    row.send_at = DUE + TAKEOVER_AFTER_MS + 2 * MIN;
    phone.upsert_scheduled_send(&row).unwrap();
    assert!(
        phone
            .get_scheduled_send(A, "s1")
            .unwrap()
            .unwrap()
            .elsewhere
    );
    assert_eq!(phone.sync_pending_count().unwrap(), pending);
}

#[test]
fn a_published_schedule_from_the_phones_own_outbox_is_shown_and_cancellable() {
    let (mac, phone, server) = (device(), device(), MemoryServer::new());
    let body = ScheduledSendBody {
        account_id: A.into(),
        draft_id: "d7".into(),
        send_at: DUE,
        created_at: T0,
        remind_after_ms: Some(MIN),
        owner: String::new(),
        claimed_by: None,
        claimed_at: None,
        outcome: None,
    };
    phone.publish_scheduled_send("o1", &body).unwrap();
    // The phone keeps it in its own outbox, not in this table.
    assert!(rows(&phone).is_empty());
    assert!(is_send(&turn(&phone, "o1", DUE)));
    sync(&phone, &server, T0);
    sync(&mac, &server, T0);
    assert_eq!(rows(&mac), vec![("o1".to_string(), true)]);
    // Cancelled on the Mac: the phone's outbox learns it at its turn.
    mac.delete_scheduled_send_by_id("o1").unwrap();
    sync(&mac, &server, T0 + MIN);
    sync(&phone, &server, T0 + MIN);
    assert_eq!(turn(&phone, "o1", DUE), Turn::Gone);
    assert!(phone
        .scheduled_send_record(A, "o1")
        .unwrap()
        .unwrap()
        .is_deleted());
}

#[test]
fn a_schedule_waits_for_its_account_and_appears_when_it_is_added() {
    let (mac, server) = (device(), MemoryServer::new());
    schedule(&mac, "s1", "d1");
    sync(&mac, &server, T0);
    let phone = Store::open_in_memory().unwrap();
    phone.set_sync_enabled(true).unwrap();
    sync(&phone, &server, T0);
    assert!(rows(&phone).is_empty());
    phone
        .upsert_account(&account(A, AccountProvider::Gmail))
        .unwrap();
    assert_eq!(rows(&phone), vec![("s1".to_string(), true)]);
}

// ---------- reminders ----------

fn remind(s: &Store, id: &str) {
    s.replace_reminders(&Reminder {
        id: id.into(),
        account_id: A.into(),
        thread_id: "t1".into(),
        sent_message_id: Some("m1".into()),
        sent_at: T0,
        remind_at: DUE,
        created_at: T0,
    })
    .unwrap();
}

#[test]
fn a_reminder_crosses_and_fires_once_after_a_fresh_pull() {
    let (mac, phone, server) = (device(), device(), MemoryServer::new());
    remind(&mac, "r1");
    sync(&mac, &server, T0);
    sync(&phone, &server, T0);
    assert_eq!(phone.list_reminders(None).unwrap().len(), 1);
    // Due, but the phone hasn't pulled since: it waits (and asks for one).
    assert!(matches!(
        phone.reminder_turn(A, "r1", DUE).unwrap(),
        Turn::Wait { .. }
    ));
    assert!(phone.due_reminders(DUE).unwrap().is_empty());
    sync(&phone, &server, DUE + MIN);
    assert_eq!(phone.reminder_turn(A, "r1", DUE + MIN).unwrap(), Turn::Send);
    phone.finish_reminder(A, "r1", Outcome::Fired).unwrap();
    sync(&phone, &server, DUE + 2 * MIN);
    sync(&mac, &server, DUE + 2 * MIN);
    assert!(mac.list_reminders(None).unwrap().is_empty());
    let r = mac
        .sync_records(Kind::Reminder)
        .unwrap()
        .into_iter()
        .find(|r| r.key == thread_key(A, "r1"))
        .unwrap();
    assert_eq!(
        r.body_as::<ReminderBody>().unwrap().outcome,
        Some(Outcome::Fired)
    );
}

#[test]
fn a_new_reminder_replaces_the_conversations_old_one_everywhere() {
    let (mac, phone, server) = (device(), device(), MemoryServer::new());
    remind(&mac, "r1");
    sync(&mac, &server, T0);
    sync(&phone, &server, T0);
    remind(&phone, "r2");
    sync(&phone, &server, T0 + MIN);
    sync(&mac, &server, T0 + MIN);
    let ids = |s: &Store| -> Vec<String> {
        s.list_reminders(None)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect()
    };
    assert_eq!(ids(&mac), ["r2"]);
    assert_eq!(ids(&phone), ["r2"]);
}

#[test]
fn without_sync_a_reminder_fires_as_before() {
    let s = device();
    s.set_sync_enabled(false).unwrap();
    remind(&s, "r1");
    assert_eq!(s.reminder_turn(A, "r1", DUE).unwrap(), Turn::Send);
    assert!(
        is_send(&turn(&s, "nope", DUE)),
        "no record: this device's own"
    );
}
