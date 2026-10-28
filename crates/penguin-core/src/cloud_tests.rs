//! The CloudKit transport's host side, end to end: a Mac and an iPhone
//! (each a real Store plus a settings file) exchanging records through a
//! fake shim that does what the Swift CKSyncEngine shim does (fetch pages
//! → `apply` → `fetched`; pending names → `outgoing` → conditional saves →
//! `saved` / `conflict`), over penguin-sync's in-memory CloudKit-like
//! server.

use std::sync::{Arc, Mutex};

use penguin_sync::memory::{MemoryServer, MemoryTransport};
use penguin_sync::{
    thread_key, CloudAccount, CloudProblem, Kind, Outgoing, PushOutcome, RemoteRecord, Transport,
};
use serde_json::{json, Value};

use super::*;
use crate::types::*;

const A: &str = "ada@penguin.example";
const HOUR: i64 = 3_600_000;
const T0: i64 = 1_760_000_000_000;

/// The app's settings file: JSON behind a lock, like SettingsState.
struct FileSettings {
    value: Mutex<Value>,
    defaults: Value,
}

struct Shared(Arc<FileSettings>);

impl SettingsFile for Shared {
    fn current(&self) -> Value {
        self.0.value.lock().unwrap().clone()
    }
    fn defaults(&self) -> Value {
        self.0.defaults.clone()
    }
    fn apply_synced(&self, records: &[penguin_sync::Record]) {
        let mut cur = self.0.value.lock().unwrap();
        // This "app" can't read a non-numeric undoSendSeconds.
        apply_settings_records(&mut cur, records, |v| {
            v.get("undoSendSeconds").is_none_or(Value::is_number)
        });
    }
}

struct Device {
    host: CloudHost,
    settings: Arc<FileSettings>,
    transport: MemoryTransport,
    token: Option<String>,
}

fn wire(r: &RemoteRecord) -> WireRecord {
    WireRecord {
        name: r.record.name(),
        payload: serde_json::to_string(&r.record).unwrap(),
        tag: r.version.tag.clone(),
        modified_ms: r.version.modified_ms,
    }
}

impl Device {
    fn new(server: &MemoryServer, defaults: Value, current: Value, partial: bool) -> Device {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_account(&Account {
                id: A.into(),
                email: A.into(),
                color: "#123456".into(),
                added_at: 1,
                ..Account::default()
            })
            .unwrap();
        let settings = Arc::new(FileSettings {
            value: Mutex::new(current),
            defaults,
        });
        let host = CloudHost::new(store, Box::new(Shared(settings.clone())), partial);
        host.set_available(None).unwrap();
        host.report_account(CloudAccount::Available).unwrap();
        Device {
            host,
            settings,
            transport: server.transport(),
            token: None,
        }
    }

    fn store(&self) -> &Store {
        self.host.store()
    }

    fn settings(&self) -> Value {
        self.settings.value.lock().unwrap().clone()
    }

    /// What the app does on a settings write: record, then write the file.
    fn edit(&self, now: i64, f: impl FnOnce(&mut Value)) {
        let old = self.settings();
        let mut new = old.clone();
        f(&mut new);
        self.host.settings_write(&old, &mut new, now).unwrap();
        *self.settings.value.lock().unwrap() = new;
    }

    /// CKSyncEngine's fetch: every page of zone changes, then "done".
    fn fetch(&mut self, now: i64) {
        loop {
            let page = self.transport.pull(self.token.as_deref()).unwrap();
            let records: Vec<WireRecord> = page.records.iter().map(wire).collect();
            self.host.apply(&records, &page.deleted, now).unwrap();
            self.token = Some(page.token);
            if !page.more {
                break;
            }
        }
        self.host.fetched(now, now).unwrap();
    }

    /// CKSyncEngine's send: the pending records, saved conditionally on
    /// their base tag; a conflict comes back with the server's record.
    fn send(&mut self, now: i64) {
        for _ in 0..4 {
            let names = self.host.pending_names(200).unwrap();
            if names.is_empty() {
                return;
            }
            let out = self.host.outgoing(&names).unwrap();
            let batch: Vec<Outgoing> = out
                .iter()
                .map(|o| Outgoing {
                    record: serde_json::from_str(&o.payload).unwrap(),
                    base_tag: o.base_tag.clone(),
                })
                .collect();
            let results = self.transport.push(&batch).unwrap();
            let mut saved = Vec::new();
            for (o, r) in out.iter().zip(results) {
                match r {
                    PushOutcome::Saved(v) => saved.push(WireRecord {
                        name: o.name.clone(),
                        payload: o.payload.clone(),
                        tag: v.tag,
                        modified_ms: v.modified_ms,
                    }),
                    PushOutcome::Conflict(server) => {
                        self.host.conflict(&wire(&server), now).unwrap();
                    }
                }
            }
            self.host.saved(&saved, now).unwrap();
        }
    }

    fn sync(&mut self, server: &MemoryServer, now: i64) {
        server.set_now(now);
        self.fetch(now);
        self.send(now);
    }
}

fn mac_defaults() -> Value {
    json!({
        "theme": "system",
        "density": "compact",
        "followUpDays": 3,
        "undoSendSeconds": 10,
        "smartViews": {"shown": [], "counts": [], "custom": []},
        "snippets": [{"id": "welcome", "body": "Hi {first_name},"}],
    })
}

/// The phone keeps a subset, smart views without the Mac's extras.
fn phone_defaults() -> Value {
    json!({
        "theme": "system",
        "density": "compact",
        "undoSendSeconds": 10,
        "smartViews": {"shown": ["receipts", "travel"]},
    })
}

fn pair(server: &MemoryServer) -> (Device, Device) {
    (
        Device::new(server, mac_defaults(), mac_defaults(), false),
        Device::new(server, phone_defaults(), phone_defaults(), true),
    )
}

#[test]
fn settings_cross_both_ways_and_the_phone_keeps_what_it_lacks() {
    let server = MemoryServer::new();
    let (mut mac, mut phone) = pair(&server);
    mac.sync(&server, T0);
    phone.sync(&server, T0 + 1);

    mac.edit(T0 + 10, |s| {
        s["undoSendSeconds"] = json!(20);
        s["smartViews"]["custom"] = json!([{"id": "vip", "name": "VIP", "query": "from:vip"}]);
        s["smartViews"]["shown"] = json!(["custom:vip"]);
        s["density"] = json!("comfortable");
    });
    mac.sync(&server, T0 + 20);
    phone.sync(&server, T0 + 30);
    let p = phone.settings();
    assert_eq!(p["undoSendSeconds"], 20);
    assert_eq!(p["density"], "compact", "density stays on each device");

    // The phone changes which views it lists; the Mac's pinned search and
    // counts survive, though the phone has no field for them.
    phone.edit(T0 + 40, |s| s["smartViews"] = json!({"shown": ["travel"]}));
    phone.sync(&server, T0 + 50);
    mac.sync(&server, T0 + 60);
    let m = mac.settings();
    assert_eq!(m["smartViews"]["shown"], json!(["travel"]));
    assert_eq!(m["smartViews"]["custom"][0]["id"], "vip");
    assert_eq!(m["density"], "comfortable");
    assert_eq!(phone.host.status().unwrap().pending, 0);
    assert_eq!(mac.host.status().unwrap().pending, 0);
}

#[test]
fn a_snippet_edited_on_the_phone_keeps_the_files_only_the_mac_has() {
    let server = MemoryServer::new();
    let (mut mac, mut phone) = pair(&server);
    mac.edit(T0, |s| {
        s["snippets"] = json!([{
            "id": "s1", "title": "Deck", "body": "Attached.",
            "attachments": [{"id": "ab12", "filename": "deck.pdf"}],
        }]);
    });
    mac.sync(&server, T0 + 1);
    phone.sync(&server, T0 + 2);
    // The phone's snippets have no files.
    phone.settings.value.lock().unwrap()["snippets"] =
        json!([{"id": "s1", "title": "Deck", "body": "Attached."}]);
    phone.edit(T0 + 10, |s| {
        s["snippets"] = json!([{"id": "s1", "title": "The deck", "body": "Attached."}])
    });
    phone.sync(&server, T0 + 11);
    mac.sync(&server, T0 + 12);
    let s = &mac.settings()["snippets"][0];
    assert_eq!(s["title"], "The deck");
    assert_eq!(s["attachments"][0]["filename"], "deck.pdf");
}

#[test]
fn a_joining_device_takes_synced_settings_and_carries_over_only_the_rest() {
    let server = MemoryServer::new();
    let mut mac_now = mac_defaults();
    mac_now["followUpDays"] = json!(7);
    mac_now["snippets"] = json!([{"id": "u1", "body": "Thanks!"}]);
    let mut mac = Device::new(&server, mac_defaults(), mac_now, false);
    mac.sync(&server, T0);
    // Seeded on the first sync: the changed key, the user's snippet and the
    // default snippet the user deleted.
    assert_eq!(
        mac.store().sync_settings_records().unwrap().len(),
        3,
        "{:?}",
        mac.store().sync_settings_records().unwrap()
    );

    // A second Mac set up later with different choices of its own.
    let mut other_now = mac_defaults();
    other_now["followUpDays"] = json!(2);
    other_now["undoSendSeconds"] = json!(30);
    let mut other = Device::new(&server, mac_defaults(), other_now, false);
    other.sync(&server, T0 + 100);
    let o = other.settings();
    assert_eq!(o["followUpDays"], 7, "settings already synced win");
    assert_eq!(o["snippets"], json!([{"id": "u1", "body": "Thanks!"}]));
    assert_eq!(
        o["undoSendSeconds"], 30,
        "its own choice where nothing was synced"
    );
    mac.sync(&server, T0 + 200);
    assert_eq!(mac.settings()["undoSendSeconds"], 30);

    // Seeding happens once: a later sync doesn't re-carry anything.
    assert!(other.store().sync_settings_seeded().unwrap());

    // Any real edit beats the seeds, even an older one made offline.
    other.edit(T0 + 150, |s| s["followUpDays"] = json!(4));
    other.sync(&server, T0 + 300);
    mac.sync(&server, T0 + 400);
    assert_eq!(mac.settings()["followUpDays"], 4);
}

#[test]
fn concurrent_edits_conflict_and_the_later_one_wins_on_both() {
    let server = MemoryServer::new();
    let (mut mac, mut phone) = pair(&server);
    mac.sync(&server, T0);
    phone.sync(&server, T0);
    mac.edit(T0 + 10, |s| s["undoSendSeconds"] = json!(5));
    phone.edit(T0 + 20, |s| s["undoSendSeconds"] = json!(30));
    // The Mac sends first; the phone's save conflicts, merges, wins, resends.
    server.set_now(T0 + 30);
    mac.send(T0 + 30);
    server.set_now(T0 + 31);
    phone.send(T0 + 31);
    mac.sync(&server, T0 + 40);
    phone.sync(&server, T0 + 41);
    assert_eq!(mac.settings()["undoSendSeconds"], 30);
    assert_eq!(phone.settings()["undoSendSeconds"], 30);
    assert_eq!(mac.host.status().unwrap().pending, 0);
}

fn message(id: &str, thread: &str) -> Message {
    Message {
        account_id: A.into(),
        id: id.into(),
        thread_id: thread.into(),
        date: T0,
        from: Address {
            name: None,
            email: "cy@quill.example".into(),
        },
        to: vec![Address {
            name: None,
            email: A.into(),
        }],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: "Lunch".into(),
        snippet: "lunch?".into(),
        body_text: "lunch?".into(),
        body_html: None,
        label_ids: vec!["INBOX".into()],
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        sender_authenticated: false,
        list_unsubscribe: None,
        list_unsubscribe_post: None,
    }
}

#[test]
fn a_snooze_crosses_through_the_host_and_its_wake_waits_for_a_fetch() {
    let server = MemoryServer::new();
    let (mut mac, mut phone) = pair(&server);
    for d in [&mac, &phone] {
        d.store().upsert_messages(&[message("m1", "t1")]).unwrap();
    }
    mac.store()
        .snooze_threads(&[(A.into(), "t1".into())], T0 + HOUR, T0)
        .unwrap();
    mac.store()
        .modify_thread_labels(A, "t1", &[], &["INBOX".to_string()])
        .unwrap();
    mac.sync(&server, T0 + 1);
    phone.sync(&server, T0 + 2);
    let z = phone
        .store()
        .get_snooze(A, "t1")
        .unwrap()
        .expect("snoozed on the phone");
    assert_eq!(z.wake_at, T0 + HOUR);
    assert!(phone
        .store()
        .sync_records(Kind::Snooze)
        .unwrap()
        .iter()
        .any(|r| r.key == thread_key(A, "t1") && !r.is_deleted()));

    // At the wake time the Mac may not tell Gmail before a fetch that
    // started after it; the host's `fetched` is that fetch.
    mac.store().wake_snooze(A, "t1", T0 + HOUR).unwrap();
    assert!(!mac.store().snooze_wake_committable(A, "t1").unwrap());
    server.set_now(T0 + HOUR + 1);
    mac.fetch(T0 + HOUR + 1);
    assert!(mac.store().snooze_wake_committable(A, "t1").unwrap());
}

#[test]
fn queued_records_wake_the_transport_once_the_write_commits() {
    let server = MemoryServer::new();
    let (mac, _) = pair(&server);
    let heard = Arc::new(Mutex::new(0));
    let h = heard.clone();
    let store = mac.store().clone();
    let reader = store.clone();
    mac.store().set_sync_listener(Some(Arc::new(move || {
        // The writer is free again: reading from the listener can't deadlock.
        let _ = reader.sync_pending_count().unwrap();
        *h.lock().unwrap() += 1;
    })));
    store
        .snooze_threads(&[(A.into(), "t1".into())], T0 + HOUR, T0)
        .unwrap();
    assert_eq!(*heard.lock().unwrap(), 1);
    // Writes that queue nothing stay quiet.
    store.set_sync_wanted(true).unwrap();
    store.set_sync_engine_state(Some(b"state")).unwrap();
    assert_eq!(*heard.lock().unwrap(), 1);
    mac.host
        .record_settings_change(
            &json!({"undoSendSeconds": 10}),
            &json!({"undoSendSeconds": 5}),
            T0,
        )
        .unwrap();
    assert_eq!(*heard.lock().unwrap(), 2);
    store.set_sync_listener(None);
}

#[test]
fn the_switch_and_the_account_decide_whether_records_move() {
    let server = MemoryServer::new();
    let (mac, _) = pair(&server);
    let s = mac.host.status().unwrap();
    assert!(s.available && s.enabled && s.active);
    assert!(mac.store().sync_enabled().unwrap());

    // No iCloud account: the device works alone, the wake gate opens.
    mac.host.report_account(CloudAccount::NoAccount).unwrap();
    assert!(!mac.store().sync_enabled().unwrap());
    assert!(mac.store().sync_pull_fresh(i64::MAX).unwrap());
    let s = mac.host.status().unwrap();
    assert_eq!((s.active, s.account), (false, CloudAccount::NoAccount));

    mac.host.report_account(CloudAccount::Available).unwrap();
    assert!(mac.store().sync_enabled().unwrap());
    // Off by the user: kept off across restarts (it's in the database).
    mac.host.set_wanted(false).unwrap();
    assert!(!mac.store().sync_enabled().unwrap());
    assert!(!mac.store().sync_wanted().unwrap());
    let s = mac.host.status().unwrap();
    assert_eq!(
        (s.enabled, s.active, s.account),
        (false, false, CloudAccount::Unknown)
    );
    assert!(!mac.host.should_run().unwrap());

    // A build without the entitlement can't run at all.
    mac.host.set_wanted(true).unwrap();
    mac.host.set_available(Some(NOT_BUILT.into())).unwrap();
    let s = mac.host.status().unwrap();
    assert!(!s.available && !s.active);
    assert_eq!(s.unavailable_reason.as_deref(), Some(NOT_BUILT));
}

#[test]
fn starting_over_sends_everything_to_the_new_server() {
    let server = MemoryServer::new();
    let (mut mac, _) = pair(&server);
    mac.edit(T0, |s| s["undoSendSeconds"] = json!(5));
    mac.store()
        .snooze_threads(&[(A.into(), "t1".into())], T0 + HOUR, T0)
        .unwrap();
    mac.sync(&server, T0 + 1);
    let stored = server.len();
    assert!(stored >= 2);
    assert_eq!(mac.host.status().unwrap().pending, 0);

    // Another iCloud account: an empty zone. Everything goes up again.
    let fresh = MemoryServer::new();
    mac.transport = fresh.transport();
    mac.token = None;
    mac.host.reset(ResetReason::SwitchedAccounts).unwrap();
    assert_eq!(mac.host.status().unwrap().pending as usize, stored);
    assert_eq!(mac.store().sync_engine_state().unwrap(), None);
    mac.sync(&fresh, T0 + 2);
    assert_eq!(fresh.len(), stored);
    assert!(mac.host.wanted().unwrap());

    // Deleted from iCloud settings: sync turns itself off and says why.
    mac.host.reset(ResetReason::Purged).unwrap();
    let s = mac.host.status().unwrap();
    assert!(!s.enabled && !s.active);
    assert_eq!(s.problem, Some(CloudProblem::DataDeleted));
    mac.host.set_wanted(true).unwrap();
    assert_eq!(mac.host.status().unwrap().problem, None);
}

#[test]
fn records_the_server_lost_go_up_again_as_new() {
    let server = MemoryServer::new();
    let (mut mac, _) = pair(&server);
    mac.edit(T0, |s| s["undoSendSeconds"] = json!(5));
    mac.sync(&server, T0 + 1);
    let name = penguin_sync::record_name(Kind::Setting, "undoSendSeconds");
    assert!(mac.host.outgoing(&[name.clone()]).unwrap().is_empty());
    mac.host.clear_base(&[name.clone()]).unwrap();
    let out = mac.host.outgoing(&[name]).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].base_tag, None);
}

#[test]
fn the_settings_file_is_brought_back_in_line_at_launch() {
    let server = MemoryServer::new();
    let (mut mac, mut phone) = pair(&server);
    phone.edit(T0, |s| s["undoSendSeconds"] = json!(5));
    phone.sync(&server, T0 + 1);
    mac.sync(&server, T0 + 2);
    // The app quit between merging a record and writing the file.
    mac.settings.value.lock().unwrap()["undoSendSeconds"] = json!(10);
    mac.host.reconcile_settings().unwrap();
    assert_eq!(mac.settings()["undoSendSeconds"], 5);
}

#[test]
fn a_value_this_app_cannot_read_is_skipped_and_the_rest_still_lands() {
    let server = MemoryServer::new();
    let (mut mac, mut phone) = pair(&server);
    // A newer app's value the phone's build can't read, beside one it can.
    mac.edit(T0, |s| {
        s["undoSendSeconds"] = json!("twenty");
        s["theme"] = json!("dark");
        s["smartViews"]["shown"] = json!(["bills"]);
    });
    mac.sync(&server, T0 + 1);
    phone.sync(&server, T0 + 2);
    let p = phone.settings();
    assert_eq!(p["undoSendSeconds"], 10);
    assert_eq!(p["smartViews"]["shown"], json!(["bills"]));
}

#[test]
fn unreadable_and_misnamed_records_are_left_alone() {
    let server = MemoryServer::new();
    let (mac, _) = pair(&server);
    let good = penguin_sync::Record::intent(
        Kind::Setting,
        "undoSendSeconds",
        penguin_sync::memory::stamp(T0, 0, "x"),
        Some(json!(5)),
    );
    let bad = vec![
        WireRecord {
            name: "setting-00".into(),
            payload: "{not json".into(),
            tag: "t".into(),
            modified_ms: T0,
        },
        WireRecord {
            name: "setting-00".into(),
            payload: serde_json::to_string(&good).unwrap(),
            tag: "t".into(),
            modified_ms: T0,
        },
    ];
    mac.host.apply(&bad, &[], T0).unwrap();
    assert!(mac.store().sync_settings_records().unwrap().is_empty());
    assert_eq!(mac.settings()["undoSendSeconds"], 10);
}

#[test]
fn the_theme_syncs_unless_a_device_keeps_it_and_letting_go_takes_the_synced_one() {
    let server = MemoryServer::new();
    let (mut mac, mut phone) = pair(&server);
    mac.sync(&server, T0);
    phone.sync(&server, T0 + 1);
    // The phone keeps following the system; the Mac goes dark.
    phone.edit(T0 + 2, |s| s["keepHere"] = json!(["theme"]));
    mac.edit(T0 + 10, |s| s["theme"] = json!("dark"));
    mac.sync(&server, T0 + 20);
    phone.sync(&server, T0 + 30);
    assert_eq!(phone.settings()["theme"], "system");
    // Changing it on the phone stays on the phone.
    phone.edit(T0 + 40, |s| s["theme"] = json!("light"));
    phone.sync(&server, T0 + 50);
    mac.sync(&server, T0 + 60);
    assert_eq!(mac.settings()["theme"], "dark");
    // Letting go: the phone takes the Mac's dark, and sends nothing.
    let pending = phone.host.status().unwrap().pending;
    phone.edit(T0 + 70, |s| s["keepHere"] = json!([]));
    assert_eq!(phone.settings()["theme"], "dark");
    assert_eq!(phone.host.status().unwrap().pending, pending);
}
