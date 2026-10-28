use serde_json::json;

use super::*;
use crate::memory::{stamp, MemoryServer, MemoryStore};
use crate::record::{merge, thread_key, Record, Winner};
use crate::sync_once;

const T0: i64 = 1_800_000_000_000;
const DUE: i64 = T0 + 60 * MIN;

fn body(owner: &str, draft: &str) -> ScheduledSendBody {
    ScheduledSendBody {
        account_id: "ada@example.example".into(),
        draft_id: draft.into(),
        send_at: DUE,
        created_at: T0,
        remind_after_ms: None,
        owner: owner.into(),
        claimed_by: None,
        claimed_at: None,
        outcome: None,
    }
}

fn scheduled(owner: &str) -> Record {
    Record::intent(
        Kind::ScheduledSend,
        thread_key("ada@example.example", "s1"),
        stamp(T0, 0, owner),
        Some(body(owner, "d1").to_value()),
    )
}

fn claimed(base: &Record, by: &str, phase: u8, at: i64) -> Record {
    let mut b: ScheduledSendBody = base.body_as().unwrap();
    b.claimed_by = Some(by.into());
    b.claimed_at = Some(at);
    base.advanced(phase, stamp(at, 0, by), Some(b.to_value()))
}

const STORED: OnServer = OnServer {
    stored: true,
    ever: true,
};
const SENT_BEFORE: OnServer = OnServer {
    stored: false,
    ever: true,
};
const NEVER: OnServer = OnServer {
    stored: false,
    ever: false,
};

#[test]
fn the_owner_sends_on_time_without_asking() {
    let r = scheduled("mac");
    assert_eq!(send_turn(&r, STORED, "mac", DUE, true, true), Turn::Send);
    assert_eq!(
        send_turn(&r, STORED, "mac", DUE + OWNER_FREE_MS - 1, true, true),
        Turn::Send
    );
}

#[test]
fn a_late_owner_claims_unless_nobody_else_could_have_sent_it() {
    let r = scheduled("mac");
    let late = DUE + OWNER_FREE_MS;
    assert!(matches!(
        send_turn(&r, SENT_BEFORE, "mac", late, true, true),
        Turn::Claim { phase: 1, .. }
    ));
    // Never reached the server, can't be taken over, or sync is off.
    assert_eq!(send_turn(&r, NEVER, "mac", late, true, true), Turn::Send);
    assert_eq!(send_turn(&r, STORED, "mac", late, false, true), Turn::Send);
    assert_eq!(send_turn(&r, STORED, "mac", late, true, false), Turn::Send);
}

#[test]
fn another_device_waits_for_the_takeover_time_then_claims() {
    let r = scheduled("mac");
    assert_eq!(
        send_turn(&r, STORED, "phone", DUE, true, true),
        Turn::Wait {
            until: DUE + TAKEOVER_AFTER_MS
        }
    );
    match send_turn(&r, STORED, "phone", DUE + TAKEOVER_AFTER_MS, true, true) {
        Turn::Claim { phase, body, .. } => {
            assert_eq!(phase, 1);
            let b: ScheduledSendBody = serde_json::from_value(body).unwrap();
            assert_eq!(b.claimed_by.as_deref(), Some("phone"));
            assert_eq!(b.owner, "mac");
        }
        t => panic!("{t:?}"),
    }
}

#[test]
fn another_device_never_sends_what_it_cannot_take_over() {
    let r = scheduled("mac");
    let much_later = DUE + 100 * TAKEOVER_AFTER_MS;
    assert!(matches!(
        send_turn(&r, STORED, "phone", much_later, false, true),
        Turn::Wait { .. }
    ));
    assert!(matches!(
        send_turn(&r, STORED, "phone", much_later, true, false),
        Turn::Wait { .. }
    ));
    let no_draft = Record::intent(
        Kind::ScheduledSend,
        r.key.clone(),
        stamp(T0, 0, "mac"),
        Some(body("mac", "").to_value()),
    );
    assert!(matches!(
        send_turn(&no_draft, STORED, "phone", much_later, true, true),
        Turn::Wait { .. }
    ));
}

#[test]
fn a_claim_sends_only_once_the_server_stored_it() {
    let at = DUE + TAKEOVER_AFTER_MS;
    let c = claimed(&scheduled("mac"), "phone", 1, at);
    assert!(matches!(
        send_turn(&c, SENT_BEFORE, "phone", at, true, true),
        Turn::Wait { .. }
    ));
    assert_eq!(send_turn(&c, STORED, "phone", at, true, true), Turn::Send);
    // Everyone else waits for it, then may claim again once it's stale.
    assert!(matches!(
        send_turn(&c, STORED, "mac", at + 1, true, true),
        Turn::Wait { until } if until == at + CLAIM_STALE_MS
    ));
    assert!(matches!(
        send_turn(&c, STORED, "mac", at + CLAIM_STALE_MS, true, true),
        Turn::Claim { phase: 2, .. }
    ));
}

#[test]
fn an_owner_whose_claim_never_reaches_the_server_sends_once_it_is_stale() {
    let at = DUE + OWNER_FREE_MS;
    let c = claimed(&scheduled("mac"), "mac", 1, at);
    assert!(matches!(
        send_turn(&c, SENT_BEFORE, "mac", at + 1, true, true),
        Turn::Wait { .. }
    ));
    assert_eq!(
        send_turn(&c, SENT_BEFORE, "mac", at + CLAIM_STALE_MS, true, true),
        Turn::Send
    );
    // Another device's unconfirmed claim never sends.
    let p = claimed(&scheduled("mac"), "phone", 1, at);
    assert!(matches!(
        send_turn(
            &p,
            SENT_BEFORE,
            "phone",
            at + 10 * CLAIM_STALE_MS,
            true,
            true
        ),
        Turn::Wait { .. }
    ));
}

#[test]
fn sent_and_cancelled_are_gone() {
    let r = scheduled("mac");
    let sent = r.advanced(
        PHASE_TERMINAL,
        stamp(DUE, 0, "mac"),
        finished_body(&r, Outcome::Sent),
    );
    assert_eq!(
        send_turn(&sent, STORED, "phone", DUE, true, true),
        Turn::Gone
    );
    let cancelled = Record::intent(r.kind, r.key.clone(), stamp(DUE, 1, "phone"), None);
    assert_eq!(
        send_turn(&cancelled, STORED, "mac", DUE, true, true),
        Turn::Gone
    );
    assert_eq!(
        sent.body_as::<ScheduledSendBody>().unwrap().outcome,
        Some(Outcome::Sent)
    );
}

#[test]
fn terminal_beats_a_newer_generation_both_ways() {
    let r = scheduled("mac");
    let sent = r.advanced(
        PHASE_TERMINAL,
        stamp(DUE, 0, "mac"),
        finished_body(&r, Outcome::Sent),
    );
    let newer = Record::intent(r.kind, r.key.clone(), stamp(DUE + MIN, 0, "phone"), None);
    assert_eq!(
        merge((&sent, Some(DUE)), (&newer, Some(DUE + MIN))),
        Winner::Local
    );
    assert_eq!(merge((&newer, None), (&sent, Some(DUE))), Winner::Remote);
}

#[test]
fn the_stored_claim_wins_over_a_concurrent_one_whatever_the_clocks() {
    let base = scheduled("mac");
    // The phone's clock is ahead: its claim has the later stamp.
    let mac = claimed(&base, "mac", 1, DUE + 20 * MIN);
    let phone = claimed(&base, "phone", 1, DUE + 25 * MIN);
    // The Mac's reached the server first.
    assert_eq!(
        merge((&phone, None), (&mac, Some(DUE + 20 * MIN))),
        Winner::Remote
    );
    assert_eq!(
        merge((&mac, Some(DUE + 20 * MIN)), (&phone, None)),
        Winner::Local
    );
    // Both stored (only possible across a re-claim): the earlier.
    assert_eq!(
        merge((&phone, Some(DUE + 30 * MIN)), (&mac, Some(DUE + 20 * MIN))),
        Winner::Remote
    );
    // A later phase (a re-claim of a stale claim) wins as usual.
    let reclaim = claimed(&mac, "phone", 2, DUE + 60 * MIN);
    assert_eq!(
        merge((&mac, Some(DUE + 20 * MIN)), (&reclaim, None)),
        Winner::Remote
    );
}

#[test]
fn two_devices_claiming_at_once_through_the_server_leave_one_claimer() {
    for mac_first in [true, false] {
        let server = MemoryServer::new();
        let (mac, phone) = (MemoryStore::new("mac"), MemoryStore::new("phone"));
        let key = thread_key("ada@example.example", "s1");
        mac.edit(
            Kind::ScheduledSend,
            &key,
            Some(body("mac", "d1").to_value()),
            T0,
        );
        for d in [&mac, &phone] {
            server.set_now(T0);
            sync_once(d, &mut server.transport(), &|| T0).unwrap();
        }
        let at = DUE + TAKEOVER_AFTER_MS;
        for (d, who) in [(&mac, "mac"), (&phone, "phone")] {
            let cur = d.get(Kind::ScheduledSend, &key).unwrap();
            let c = claimed(&cur, who, 1, at);
            d.advance(Kind::ScheduledSend, &key, 1, c.body.clone(), at);
        }
        let order = if mac_first {
            [&mac, &phone]
        } else {
            [&phone, &mac]
        };
        for d in order.iter().chain(order.iter()) {
            server.set_now(at);
            sync_once(*d, &mut server.transport(), &|| at).unwrap();
        }
        let winner = if mac_first { "mac" } else { "phone" };
        for d in [&mac, &phone] {
            let r = d.get(Kind::ScheduledSend, &key).unwrap();
            let b: ScheduledSendBody = r.body_as().unwrap();
            assert_eq!(
                b.claimed_by.as_deref(),
                Some(winner),
                "mac_first {mac_first}"
            );
        }
        let stored = server
            .get(&crate::record_name(Kind::ScheduledSend, &key))
            .unwrap();
        let b: ScheduledSendBody = stored.record.body_as().unwrap();
        assert_eq!(b.claimed_by.as_deref(), Some(winner));
    }
}

#[test]
fn reminders_fire_after_a_fresh_pull_and_end_for_good() {
    let r = Record::intent(
        Kind::Reminder,
        thread_key("ada@example.example", "r1"),
        stamp(T0, 0, "mac"),
        Some(
            json!({"accountId": "ada@example.example", "threadId": "t1", "sentAt": T0,
                    "remindAt": DUE, "createdAt": T0}),
        ),
    );
    assert!(
        matches!(reminder_turn(&r, DUE, false), Turn::Wait { until } if until == DUE + PULL_WAIT_MS)
    );
    assert_eq!(reminder_turn(&r, DUE, true), Turn::Send);
    let fired = r.advanced(
        PHASE_TERMINAL,
        stamp(DUE, 0, "phone"),
        finished_body(&r, Outcome::Fired),
    );
    assert_eq!(reminder_turn(&fired, DUE, true), Turn::Gone);
    assert!(is_terminal(&fired));
    assert_eq!(
        fired.body_as::<ReminderBody>().unwrap().outcome,
        Some(Outcome::Fired)
    );
}
