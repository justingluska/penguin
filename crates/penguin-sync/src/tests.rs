use serde_json::json;

use crate::memory::{stamp, MemoryServer, MemoryStore};
use crate::record::{merge, thread_key, Kind, Record, SnoozeBody, SnoozeState, Winner};
use crate::settings;
use crate::{sync_once, Clock, Error, Hlc, LocalStore, MAX_SKEW_MS};

const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const T0: i64 = 1_800_000_000_000;

fn key() -> String {
    thread_key("Ada@Example.example", "t1")
}

fn snoozed(wake: i64, at: i64) -> Option<serde_json::Value> {
    Some(SnoozeBody::snoozed(wake, at).to_value())
}

fn state(r: &Record) -> Option<SnoozeState> {
    r.body_as::<SnoozeBody>().map(|b| b.state)
}

fn sync(store: &MemoryStore, server: &MemoryServer, now: i64) {
    server.set_now(now);
    sync_once(store, &mut server.transport(), &|| now).expect("sync");
}

// ---------- clocks ----------

#[test]
fn tick_never_goes_backwards() {
    let mut c = Clock::new("a", 0, 0);
    let a = c.tick(T0);
    let b = c.tick(T0 - 5_000); // wall clock stepped back
    let d = c.tick(T0);
    assert!(a < b && b < d);
    assert_eq!(b.wall, T0);
    assert_eq!(c.tick(T0 + 1).count, 0);
}

#[test]
fn observing_a_newer_stamp_moves_past_it() {
    // A device whose clock is an hour behind still stamps after what it saw.
    let mut slow = Clock::new("slow", 0, 0);
    let seen = stamp(T0, 7, "fast");
    slow.observe(&seen, T0 - HOUR);
    let next = slow.tick(T0 - HOUR);
    assert!(next > seen, "{next:?} must sort after {seen:?}");
}

#[test]
fn a_broken_future_clock_drags_others_only_to_the_server_time() {
    // A device a month fast writes; the server stores it at T0.
    let server = MemoryServer::new();
    let (broken, phone) = (MemoryStore::new("broken"), MemoryStore::new("phone"));
    broken.edit(
        Kind::Setting,
        "followUpDays",
        Some(json!(2)),
        T0 + 30 * 24 * HOUR,
    );
    sync(&broken, &server, T0);
    sync(&phone, &server, T0);
    assert!(
        phone.clock().wall <= T0 + MAX_SKEW_MS,
        "{:?}",
        phone.clock()
    );
}

#[test]
fn clamping_caps_future_stamps_at_the_server_time() {
    let h = stamp(T0 + 24 * HOUR, 3, "a");
    assert_eq!(h.clamped(Some(T0)).wall, T0);
    assert_eq!(h.clamped(None), h);
    // Within the bound it is left alone.
    let near = stamp(T0 + MAX_SKEW_MS, 0, "a");
    assert_eq!(near.clamped(Some(T0)), near);
}

#[test]
fn stamps_serialize_compactly() {
    let s = serde_json::to_string(&stamp(5, 1, "d")).unwrap();
    assert_eq!(s, r#"{"w":5,"c":1,"d":"d"}"#);
}

// ---------- merge ----------

#[test]
fn newer_intent_wins() {
    let old = Record::intent(
        Kind::Snooze,
        key(),
        stamp(T0, 0, "a"),
        snoozed(T0 + HOUR, T0),
    );
    let new = Record::intent(
        Kind::Snooze,
        key(),
        stamp(T0 + 1, 0, "b"),
        snoozed(T0 + 2 * HOUR, T0),
    );
    assert_eq!(merge((&old, None), (&new, None)), Winner::Remote);
    assert_eq!(merge((&new, None), (&old, None)), Winner::Local);
    assert_eq!(merge((&new, None), (&new, Some(T0))), Winner::Same);
}

#[test]
fn a_later_phase_of_the_same_intent_wins_whatever_its_stamp() {
    let snooze = Record::intent(
        Kind::Snooze,
        key(),
        stamp(T0, 0, "a"),
        snoozed(T0 + HOUR, T0),
    );
    // Woken by a device whose clock is behind: `at` is older than `gen`.
    let woken = snooze.advanced(2, stamp(T0 - 10 * MIN, 0, "b"), None);
    assert_eq!(merge((&snooze, None), (&woken, None)), Winner::Remote);
}

#[test]
fn phases_never_go_backwards() {
    let r = Record::intent(Kind::Snooze, key(), stamp(T0, 0, "a"), None);
    assert_eq!(
        r.advanced(2, stamp(T0, 1, "a"), None)
            .advanced(1, stamp(T0, 2, "a"), None)
            .phase,
        2
    );
}

#[test]
fn equal_orders_with_different_bodies_pick_the_same_side_everywhere() {
    let s = stamp(T0, 0, "a");
    let x = Record::intent(Kind::Setting, "followUpDays", s.clone(), Some(json!(3)));
    let y = Record::intent(Kind::Setting, "followUpDays", s, Some(json!(5)));
    let one = merge((&x, None), (&y, None));
    let two = merge((&y, None), (&x, None));
    assert_ne!(one, Winner::Same);
    // x as local vs y as remote picks the same record as y local vs x remote.
    let pick = |w: Winner, l: &Record, r: &Record| {
        if w == Winner::Local {
            l.clone()
        } else {
            r.clone()
        }
    };
    assert_eq!(pick(one, &x, &y), pick(two, &y, &x));
}

/// Folding any permutation of the same versions gives the same result.
#[test]
fn merge_is_order_independent() {
    let base = Record::intent(
        Kind::Snooze,
        key(),
        stamp(T0, 0, "a"),
        snoozed(T0 + HOUR, T0),
    );
    let versions = [
        (base.clone(), Some(T0)),
        (base.advanced(1, stamp(T0 + 5, 0, "b"), None), Some(T0 + 6)),
        (base.advanced(2, stamp(T0 + 4, 0, "c"), None), None),
        (
            Record::intent(Kind::Snooze, key(), stamp(T0 + 3, 0, "b"), None),
            Some(T0 + 3),
        ),
        (
            Record::intent(
                Kind::Snooze,
                key(),
                stamp(T0 + 2, 1, "c"),
                snoozed(T0 + 9, T0),
            ),
            None,
        ),
        // From a clock a day fast, stored at T0 + 1: clamped below the others.
        (
            Record::intent(Kind::Snooze, key(), stamp(T0 + 24 * HOUR, 0, "x"), None),
            Some(T0 + 1),
        ),
    ];
    let fold = |order: &[usize]| {
        let mut acc = versions[order[0]].clone();
        for &i in &order[1..] {
            if merge((&acc.0, acc.1), (&versions[i].0, versions[i].1)) == Winner::Remote {
                acc = versions[i].clone();
            }
        }
        acc.0
    };
    let expected = fold(&[0, 1, 2, 3, 4, 5]);
    let mut idx: Vec<usize> = (0..versions.len()).collect();
    // All 720 permutations (Heap's algorithm).
    let mut c = vec![0; idx.len()];
    let mut i = 0;
    while i < idx.len() {
        if c[i] < i {
            if i % 2 == 0 {
                idx.swap(0, i);
            } else {
                idx.swap(c[i], i);
            }
            assert_eq!(fold(&idx), expected, "order {idx:?}");
            c[i] += 1;
            i = 0;
        } else {
            c[i] = 0;
            i += 1;
        }
    }
    // The newest intent (gen T0+3, the unsnooze) wins: the far-future one
    // was clamped to T0+1.
    assert!(expected.is_deleted());
    assert_eq!(expected.gen.device, "b");
}

#[test]
fn record_names_hide_the_key_and_are_stable() {
    let n = crate::record_name(Kind::Snooze, &key());
    assert!(n.starts_with("snooze-"));
    assert!(!n.contains("Ada") && !n.contains("t1"));
    assert_eq!(n.len(), "snooze-".len() + 40);
    assert_eq!(
        n,
        crate::record_name(Kind::Snooze, &thread_key("ada@example.example", "t1"))
    );
    assert_ne!(n, crate::record_name(Kind::FollowUpDismissal, &key()));
}

#[test]
fn unknown_kinds_from_newer_apps_still_parse() {
    let r: Record = serde_json::from_str(
        r#"{"kind":"pinnedNote","key":"k","gen":{"w":1,"c":0,"d":"a"},"at":{"w":1,"c":0,"d":"a"},"body":{"x":1}}"#,
    )
    .unwrap();
    assert_eq!(r.kind, Kind::Unknown);
    assert_eq!(r.phase, 0);
}

// ---------- two devices through the server ----------

#[test]
fn a_snooze_reaches_the_other_device() {
    let server = MemoryServer::new();
    let (mac, phone) = (MemoryStore::new("mac"), MemoryStore::new("phone"));
    mac.edit(Kind::Snooze, &key(), snoozed(T0 + HOUR, T0), T0);
    sync(&mac, &server, T0 + 1);
    sync(&phone, &server, T0 + 2);
    let got = phone.get(Kind::Snooze, &key()).expect("synced");
    assert_eq!(got.body_as::<SnoozeBody>().unwrap().wake_at, T0 + HOUR);
    assert!(phone.pending(10).unwrap().is_empty());
    assert_eq!(phone.last_pull(), Some(T0 + 2));
    // The phone's clock moved past what it saw.
    assert!(phone.clock().wall >= T0);
}

#[test]
fn concurrent_snooze_edits_converge_on_the_later_one() {
    let server = MemoryServer::new();
    let (mac, phone) = (MemoryStore::new("mac"), MemoryStore::new("phone"));
    mac.edit(Kind::Snooze, &key(), snoozed(T0 + HOUR, T0), T0);
    sync(&mac, &server, T0);
    sync(&phone, &server, T0);
    // Both change it while apart: the phone later in time.
    mac.edit(
        Kind::Snooze,
        &key(),
        snoozed(T0 + 3 * HOUR, T0 + MIN),
        T0 + MIN,
    );
    phone.edit(
        Kind::Snooze,
        &key(),
        snoozed(T0 + 5 * HOUR, T0 + 2 * MIN),
        T0 + 2 * MIN,
    );
    sync(&mac, &server, T0 + 3 * MIN);
    // The phone's push conflicts with the Mac's, merges, and wins.
    server.set_now(T0 + 4 * MIN);
    let report = sync_once(&phone, &mut server.transport(), &|| T0 + 4 * MIN).unwrap();
    assert_eq!(report.took, 0, "the Mac's older edit lost on pull");
    sync(&mac, &server, T0 + 5 * MIN);
    for d in [&mac, &phone] {
        assert_eq!(
            d.get(Kind::Snooze, &key())
                .unwrap()
                .body_as::<SnoozeBody>()
                .unwrap()
                .wake_at,
            T0 + 5 * HOUR
        );
    }
    assert_eq!(mac.snapshot(), phone.snapshot());
}

#[test]
fn a_real_mid_pass_conflict_is_merged() {
    // Drive the conflict directly: push with a stale base tag.
    let server = MemoryServer::new();
    let (mac, phone) = (MemoryStore::new("mac"), MemoryStore::new("phone"));
    mac.edit(Kind::Setting, "sendLaterHour", Some(json!(8)), T0);
    sync(&mac, &server, T0);
    sync(&phone, &server, T0);
    phone.edit(Kind::Setting, "sendLaterHour", Some(json!(9)), T0 + 2 * MIN);
    let stale = phone.pending(10).unwrap();
    mac.edit(Kind::Setting, "sendLaterHour", Some(json!(7)), T0 + MIN);
    sync(&mac, &server, T0 + MIN);
    use crate::Transport;
    let out = server.transport().push(&stale).unwrap();
    let crate::PushOutcome::Conflict(theirs) = &out[0] else {
        panic!("expected a conflict, got {out:?}");
    };
    // The phone's edit is newer: it keeps it and re-queues on the new base.
    assert_eq!(
        phone.apply_remote(theirs, T0 + 3 * MIN).unwrap(),
        crate::Applied::Kept
    );
    let again = phone.pending(10).unwrap();
    assert_eq!(
        again[0].base_tag.as_deref(),
        Some(theirs.version.tag.as_str())
    );
    let out = server.transport().push(&again).unwrap();
    assert!(matches!(out[0], crate::PushOutcome::Saved(_)));
}

#[test]
fn offline_resnooze_beats_another_devices_wake_of_the_old_snooze() {
    let server = MemoryServer::new();
    let (mac, phone) = (MemoryStore::new("mac"), MemoryStore::new("phone"));
    mac.edit(Kind::Snooze, &key(), snoozed(T0 + HOUR, T0), T0);
    sync(&mac, &server, T0);
    sync(&phone, &server, T0);
    // Offline phone re-snoozes to tomorrow just before the wake...
    phone.edit(
        Kind::Snooze,
        &key(),
        snoozed(T0 + 24 * HOUR, T0 + HOUR - MIN),
        T0 + HOUR - MIN,
    );
    // ...the Mac wakes the old snooze on time and syncs.
    mac.advance(
        Kind::Snooze,
        &key(),
        SnoozeState::Woken.phase(),
        None,
        T0 + HOUR,
    );
    sync(&mac, &server, T0 + HOUR);
    // The phone comes back: its re-snooze is a newer intent and wins.
    sync(&phone, &server, T0 + 2 * HOUR);
    sync(&mac, &server, T0 + 2 * HOUR);
    for d in [&mac, &phone] {
        let r = d.get(Kind::Snooze, &key()).unwrap();
        assert_eq!(state(&r), Some(SnoozeState::Snoozed));
        assert_eq!(r.body_as::<SnoozeBody>().unwrap().wake_at, T0 + 24 * HOUR);
    }
}

#[test]
fn unsnooze_beats_a_concurrent_wake_and_a_wake_beats_a_stale_snooze_copy() {
    let server = MemoryServer::new();
    let (mac, phone) = (MemoryStore::new("mac"), MemoryStore::new("phone"));
    mac.edit(Kind::Snooze, &key(), snoozed(T0 + HOUR, T0), T0);
    sync(&mac, &server, T0);
    sync(&phone, &server, T0);
    // The Mac wakes it; the phone (not synced yet) still has it snoozed and
    // must take the wake, not push its old copy back.
    mac.advance(
        Kind::Snooze,
        &key(),
        SnoozeState::Woken.phase(),
        Some(json!({"state":"woken","wakeAt":T0 + HOUR,"snoozedAt":T0,"endedAt":T0 + HOUR})),
        T0 + HOUR,
    );
    sync(&mac, &server, T0 + HOUR);
    sync(&phone, &server, T0 + HOUR + 1);
    assert_eq!(
        state(&phone.get(Kind::Snooze, &key()).unwrap()),
        Some(SnoozeState::Woken)
    );
    // Unsnoozing (a user intent, new generation) beats the wake.
    phone.edit(Kind::Snooze, &key(), None, T0 + HOUR + 2);
    sync(&phone, &server, T0 + HOUR + 3);
    sync(&mac, &server, T0 + HOUR + 4);
    assert!(mac.get(Kind::Snooze, &key()).unwrap().is_deleted());
}

#[test]
fn delete_and_concurrent_edit_resolve_by_intent_time() {
    let server = MemoryServer::new();
    let (mac, phone) = (MemoryStore::new("mac"), MemoryStore::new("phone"));
    mac.edit(
        Kind::FollowUpDismissal,
        &key(),
        Some(json!({"sentAt": T0})),
        T0,
    );
    sync(&mac, &server, T0);
    sync(&phone, &server, T0);
    // Mac un-dismisses (tombstone) at +2; phone re-dismisses at +1.
    mac.edit(Kind::FollowUpDismissal, &key(), None, T0 + 2 * MIN);
    phone.edit(
        Kind::FollowUpDismissal,
        &key(),
        Some(json!({"sentAt": T0 + 5})),
        T0 + MIN,
    );
    sync(&phone, &server, T0 + 3 * MIN);
    sync(&mac, &server, T0 + 4 * MIN);
    sync(&phone, &server, T0 + 5 * MIN);
    assert!(mac
        .get(Kind::FollowUpDismissal, &key())
        .unwrap()
        .is_deleted());
    assert_eq!(mac.snapshot(), phone.snapshot());
}

#[test]
fn a_fast_clock_loses_its_head_start_once_stored() {
    let server = MemoryServer::new();
    let (fast, phone) = (MemoryStore::new("fast"), MemoryStore::new("phone"));
    // This Mac's clock is a day ahead. It writes at real time T0.
    fast.edit(
        Kind::Setting,
        "followUpDays",
        Some(json!(2)),
        T0 + 24 * HOUR,
    );
    sync(&fast, &server, T0);
    // An hour later (real time) the phone changes it. Without the server
    // clamp the fast Mac's stamp would win for another 23 hours.
    sync(&phone, &server, T0 + HOUR);
    phone.edit(Kind::Setting, "followUpDays", Some(json!(9)), T0 + HOUR);
    sync(&phone, &server, T0 + HOUR);
    sync(&fast, &server, T0 + HOUR);
    for d in [&fast, &phone] {
        assert_eq!(
            d.get(Kind::Setting, "followUpDays").unwrap().body,
            Some(json!(9))
        );
    }
}

#[test]
fn offline_changes_wait_and_go_out_later() {
    let server = MemoryServer::new();
    let mac = MemoryStore::new("mac");
    mac.edit(Kind::Snooze, &key(), snoozed(T0 + HOUR, T0), T0);
    let mut t = server.transport();
    t.offline = true;
    let err = sync_once(&mac, &mut t, &|| T0).unwrap_err();
    assert!(matches!(err, Error::Unavailable(_)));
    assert_eq!(mac.pending(10).unwrap().len(), 1);
    assert_eq!(mac.last_pull(), None, "a failed pass is not a fresh pull");
    t.offline = false;
    sync_once(&mac, &mut t, &|| T0 + 1).unwrap();
    assert!(mac.pending(10).unwrap().is_empty());
    assert_eq!(server.len(), 1);
}

#[test]
fn pulls_page_through_the_feed() {
    let server = MemoryServer::new();
    server.set_page_size(2);
    let (mac, phone) = (MemoryStore::new("mac"), MemoryStore::new("phone"));
    for i in 0..5 {
        mac.edit(
            Kind::Snooze,
            &thread_key("a@x.example", &format!("t{i}")),
            snoozed(T0 + HOUR, T0),
            T0,
        );
    }
    sync(&mac, &server, T0);
    let r = sync_once(&phone, &mut server.transport(), &|| T0).unwrap();
    assert_eq!(r.pulled, 5);
    assert_eq!(phone.snapshot().len(), 5);
}

#[test]
fn collected_tombstones_are_forgotten_unless_edited_since() {
    let server = MemoryServer::new();
    let (mac, phone) = (MemoryStore::new("mac"), MemoryStore::new("phone"));
    mac.edit(Kind::Snooze, &key(), snoozed(T0 + HOUR, T0), T0);
    mac.edit(Kind::Snooze, "other", snoozed(T0 + HOUR, T0), T0);
    sync(&mac, &server, T0);
    sync(&phone, &server, T0);
    mac.edit(Kind::Snooze, &key(), None, T0 + 1);
    mac.edit(Kind::Snooze, "other", None, T0 + 1);
    sync(&mac, &server, T0 + 1);
    sync(&phone, &server, T0 + 2);
    // The phone re-snoozes "other" before hearing of the collection.
    phone.edit(
        Kind::Snooze,
        "other",
        snoozed(T0 + 9 * HOUR, T0 + 3),
        T0 + 3,
    );
    assert_eq!(server.collect_tombstones(T0 + 100), 2);
    sync(&phone, &server, T0 + 200);
    assert!(
        phone.get(Kind::Snooze, &key()).is_none(),
        "collected tombstone forgotten"
    );
    let other = phone.get(Kind::Snooze, "other").expect("unsent edit kept");
    assert!(!other.is_deleted());
    // And it was pushed again as a new record.
    assert!(server.get(&other.name()).is_some());
}

#[test]
fn hlc_type_is_ordered_by_wall_then_count_then_device() {
    let mut v = vec![
        stamp(2, 0, "a"),
        stamp(1, 5, "z"),
        stamp(1, 5, "b"),
        stamp(1, 0, "z"),
    ];
    v.sort();
    assert_eq!(
        v,
        vec![
            stamp(1, 0, "z"),
            stamp(1, 5, "b"),
            stamp(1, 5, "z"),
            stamp(2, 0, "a")
        ]
    );
    assert!(Hlc::zero() < stamp(0, 0, "a"));
}

// ---------- settings ----------

#[test]
fn settings_changes_cover_synced_keys_and_list_items_only() {
    let old = json!({
        "density": "compact",
        "followUpDays": 3,
        "snippets": [{"id": "s1", "trigger": "hi", "title": "Hi", "body": "Hello"}, {"id": "s2", "trigger": "bye", "title": "Bye", "body": "Bye"}],
    });
    let new = json!({
        "density": "comfortable",
        "followUpDays": 5,
        "snippets": [{"id": "s1", "trigger": "hi", "title": "Hi", "body": "Hello there"}, {"id": "s3", "trigger": "ty", "title": "Thanks", "body": "Thanks"}],
    });
    let mut c = settings::changes(&old, &new);
    c.sort_by(|a, b| a.1.cmp(&b.1));
    let keys: Vec<(Kind, &str, bool)> = c
        .iter()
        .map(|(k, key, v)| (*k, key.as_str(), v.is_some()))
        .collect();
    assert_eq!(
        keys,
        vec![
            (Kind::Setting, "followUpDays", true),
            (Kind::Snippet, "s1", true),
            (Kind::Snippet, "s2", false),
            (Kind::Snippet, "s3", true),
        ]
    );
}

#[test]
fn settings_apply_round_trips_and_ignores_unsynced_keys() {
    let mut s = json!({"density": "compact", "snippets": [{"id": "s1", "body": "a"}]});
    assert!(settings::apply(
        &mut s,
        Kind::Setting,
        "followUpDays",
        Some(&json!(4))
    ));
    assert!(!settings::apply(
        &mut s,
        Kind::Setting,
        "followUpDays",
        Some(&json!(4))
    ));
    assert!(!settings::apply(
        &mut s,
        Kind::Setting,
        "density",
        Some(&json!("comfortable"))
    ));
    assert!(settings::apply(
        &mut s,
        Kind::Snippet,
        "s2",
        Some(&json!({"id": "s2", "body": "b"}))
    ));
    assert!(settings::apply(&mut s, Kind::Snippet, "s1", None));
    assert!(settings::apply(
        &mut s,
        Kind::Signature,
        "g1",
        Some(&json!({"id": "g1", "name": "Work", "html": "x"}))
    ));
    assert_eq!(s["density"], "compact");
    assert_eq!(s["followUpDays"], 4);
    assert_eq!(s["snippets"], json!([{"id": "s2", "body": "b"}]));
    assert_eq!(s["signatures"][0]["id"], "g1");
}

#[test]
fn a_seeded_record_loses_to_any_real_edit_and_seeds_tie_on_at() {
    let real = Record::intent(
        Kind::Setting,
        "followUpDays",
        stamp(5, 0, "mac"),
        Some(json!(3)),
    );
    // Seeded long after the edit, on another device: still loses.
    let seed = Record::seeded(
        Kind::Setting,
        "followUpDays",
        stamp(9_000, 0, "phone"),
        Some(json!(7)),
    );
    assert_eq!(seed.gen.wall, 0);
    assert_eq!(seed.gen.device, "phone");
    assert_eq!(merge((&seed, None), (&real, None)), Winner::Remote);
    assert_eq!(
        merge((&real, Some(10)), (&seed, Some(9_000))),
        Winner::Local
    );
    // Two seeds: the later write wins, on both sides.
    let other = Record::seeded(
        Kind::Setting,
        "followUpDays",
        stamp(8_000, 0, "mac"),
        Some(json!(2)),
    );
    assert_eq!(merge((&seed, None), (&other, None)), Winner::Local);
    assert_eq!(merge((&other, None), (&seed, None)), Winner::Remote);
}

#[test]
fn overlay_keeps_fields_this_device_does_not_have() {
    let mac = json!({
        "shown": ["receipts", "custom:a"],
        "counts": ["receipts"],
        "custom": [{"id": "a", "name": "A", "query": "from:a"}],
    });
    let phone = json!({"shown": ["travel"]});
    let out = settings::overlay(Some(&mac), &phone);
    assert_eq!(out["shown"], json!(["travel"]));
    assert_eq!(out["counts"], mac["counts"]);
    assert_eq!(out["custom"], mac["custom"]);
    // Nested objects merge; arrays and plain values replace.
    let base = json!({"a": {"x": 1, "y": 2}, "b": [1, 2]});
    let ours = json!({"a": {"y": 3}, "b": [9]});
    assert_eq!(
        settings::overlay(Some(&base), &ours),
        json!({"a": {"x": 1, "y": 3}, "b": [9]})
    );
    // Nothing synced yet, or not an object: ours as it is.
    assert_eq!(settings::overlay(None, &phone), phone);
    assert_eq!(settings::overlay(Some(&json!(4)), &json!(5)), json!(5));
}

#[test]
fn seeding_carries_only_what_differs_from_the_defaults() {
    let defaults = json!({
        "followUpDays": 3,
        "undoSendSeconds": 10,
        "density": "compact",
        "snippets": [{"id": "d1", "body": "default"}, {"id": "d2", "body": "other"}],
    });
    let current = json!({
        "followUpDays": 3,
        "undoSendSeconds": 20,
        "density": "comfortable",
        "snippets": [{"id": "d1", "body": "default"}, {"id": "u1", "body": "mine"}],
    });
    let mut c = settings::seed_changes(&current, &defaults);
    c.sort_by(|a, b| a.1.cmp(&b.1));
    let got: Vec<(Kind, &str, Option<serde_json::Value>)> = c
        .iter()
        .map(|(k, key, v)| (*k, key.as_str(), v.clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            (Kind::Snippet, "d2", None),
            (
                Kind::Snippet,
                "u1",
                Some(json!({"id": "u1", "body": "mine"}))
            ),
            (Kind::Setting, "undoSendSeconds", Some(json!(20))),
        ]
    );
}

#[test]
fn a_write_keeps_the_other_devices_accounts() {
    // The phone has ada, bo and cy; this Mac has only ada and bo.
    let known = |a: &str| a == "ada@x.example" || a == "bo@x.example";
    let old = json!({
        "accountOrder": ["cy@x.example", "ada@x.example", "bo@x.example"],
        "hiddenFromAll": ["cy@x.example", "bo@x.example"],
        "signatureDefaults": {"cy@x.example": "s2", "ada@x.example": "s1"},
        "notifications": {"enabled": true, "accounts": {"cy@x.example": false, "ada@x.example": false}},
        "profiles": [
            {"id": "work", "accountIds": ["ada@x.example", "cy@x.example"]},
            {"id": "home", "accountIds": ["cy@x.example"]},
        ],
    });
    // The Mac's UI only saw its own accounts, and the user: swapped the
    // order, unhid bo, cleared ada's signature, emptied Work, deleted Home.
    let mut new = json!({
        "accountOrder": ["bo@x.example", "ada@x.example"],
        "hiddenFromAll": [],
        "signatureDefaults": {},
        "notifications": {"enabled": true, "accounts": {}},
        "profiles": [{"id": "work", "accountIds": []}],
    });
    settings::keep_other_devices_accounts(&old, &mut new, &known);
    assert_eq!(
        new,
        json!({
            "accountOrder": ["cy@x.example", "bo@x.example", "ada@x.example"],
            "hiddenFromAll": ["cy@x.example"],
            "signatureDefaults": {"cy@x.example": "s2"},
            "notifications": {"enabled": true, "accounts": {"cy@x.example": false}},
            "profiles": [{"id": "work", "accountIds": ["cy@x.example"]}],
        })
    );
    // Nothing foreign: the write stands as it is.
    let mut same = json!({"accountOrder": ["bo@x.example"]});
    settings::keep_other_devices_accounts(
        &json!({"accountOrder": ["ada@x.example", "bo@x.example"]}),
        &mut same,
        &known,
    );
    assert_eq!(same, json!({"accountOrder": ["bo@x.example"]}));
}

#[test]
fn apply_records_brings_a_settings_file_in_line() {
    let mut s = json!({"followUpDays": 3, "snippets": [{"id": "s1", "body": "a"}]});
    let recs = vec![
        Record::intent(
            Kind::Setting,
            "followUpDays",
            stamp(1, 0, "m"),
            Some(json!(5)),
        ),
        Record::intent(Kind::Snippet, "s1", stamp(2, 0, "m"), None),
        Record::intent(
            Kind::Snippet,
            "s2",
            stamp(3, 0, "m"),
            Some(json!({"id": "s2", "body": "b"})),
        ),
        // Not a settings kind: ignored.
        Record::intent(
            Kind::FollowUpDismissal,
            "x",
            stamp(4, 0, "m"),
            Some(json!({"sentAt": 1})),
        ),
    ];
    assert!(settings::apply_records(&mut s, &recs));
    assert_eq!(
        s,
        json!({"followUpDays": 5, "snippets": [{"id": "s2", "body": "b"}]})
    );
    assert!(
        !settings::apply_records(&mut s, &recs),
        "applying again changes nothing"
    );
    assert!(settings::is_settings_kind(Kind::Signature));
    assert!(!settings::is_settings_kind(Kind::Snooze));
}

// ---------- wider settings sync ----------

#[test]
fn appearance_swipes_split_inbox_and_sender_photos_now_sync() {
    let old = json!({"theme": "system", "swipeLeft": "archive", "inboxTabs": false,
                     "senderPhotos": {"gravatar": false}, "density": "compact"});
    let new = json!({"theme": "dark", "swipeLeft": "snooze", "inboxTabs": true,
                     "senderPhotos": {"gravatar": true}, "density": "comfortable"});
    let mut keys: Vec<String> = settings::changes(&old, &new)
        .into_iter()
        .map(|(_, k, _)| k)
        .collect();
    keys.sort();
    // Density depends on the screen: it stays on each device.
    assert_eq!(keys, ["inboxTabs", "senderPhotos", "swipeLeft", "theme"]);
}

#[test]
fn a_key_kept_on_this_device_is_neither_recorded_nor_overwritten() {
    let old = json!({"theme": "dark", "accentColor": "blue", "keepHere": ["theme"]});
    let new = json!({"theme": "system", "accentColor": "pink", "keepHere": ["theme"]});
    let c = settings::changes(&old, &new);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].1, "accentColor");
    let mut s = new.clone();
    assert!(!settings::apply(&mut s, Kind::Setting, "theme", Some(&json!("light"))));
    assert_eq!(s["theme"], "system");
    assert!(settings::apply(&mut s, Kind::Setting, "accentColor", Some(&json!("green"))));
    // keepHere itself never syncs, and neither does seeding a kept key.
    assert!(!settings::apply(&mut s, Kind::Setting, "keepHere", Some(&json!([]))));
    assert!(settings::seed_changes(&new, &json!({"theme": "light"}))
        .iter()
        .all(|(_, k, _)| k != "theme" && k != "keepHere"));
}

#[test]
fn letting_go_of_a_kept_key_takes_the_synced_value_without_recording_it() {
    let old = json!({"theme": "system", "keepHere": ["theme"]});
    let new = json!({"theme": "system", "keepHere": []});
    assert_eq!(settings::unkept(&old, &new), ["theme"]);
    assert!(settings::changes(&old, &new).is_empty());
    assert!(settings::unkept(&new, &old).is_empty());
}

#[test]
fn the_phones_own_signatures_sync_and_keep_the_macs_extra_fields() {
    // The iPhone keeps id, name and html (PEN-88); the Mac may keep more.
    let mac_item = json!({"id": "g1", "name": "Work", "html": "<b>Ada</b>", "macOnly": 1});
    let phone_old = json!({"signatures": [{"id": "g1", "name": "Work", "html": "<b>Ada</b>"}]});
    let phone_new = json!({"signatures": [{"id": "g1", "name": "Work", "html": "<i>Ada</i>"},
                                          {"id": "g2", "name": "Home", "html": "A"}],
                           "signatureDefaults": {"ada@example.example": "g2"}});
    let c = settings::changes(&phone_old, &phone_new);
    let kinds: Vec<(Kind, &str)> = c.iter().map(|(k, key, _)| (*k, key.as_str())).collect();
    assert!(kinds.contains(&(Kind::Signature, "g1")));
    assert!(kinds.contains(&(Kind::Signature, "g2")));
    assert!(kinds.contains(&(Kind::Setting, "signatureDefaults")));
    let edited = c.iter().find(|(_, k, _)| k == "g1").unwrap().2.clone().unwrap();
    let merged = settings::overlay(Some(&mac_item), &edited);
    assert_eq!(merged["html"], "<i>Ada</i>");
    assert_eq!(merged["macOnly"], 1);
}
