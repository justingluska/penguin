//! Which settings sync, and how a change to `settings.json` becomes
//! records (and records become settings again).
//!
//! Settings are the app's JSON object (camelCase keys, src-tauri
//! `Settings`). Each synced top-level key is one [`Kind::Setting`] record
//! whose body is the key's value, so two devices changing different
//! settings never conflict, and the same setting resolves last-writer-wins.
//! Snippets and signatures are lists people grow on both devices, so each
//! item (by its `id`) is its own record ([`Kind::Snippet`],
//! [`Kind::Signature`]): adding one on the phone and another on the Mac
//! keeps both.
//!
//! Everything else stays on the device: what depends on this screen or
//! keyboard (density, fonts and sizes, Floe mode, shortcut hints), what this
//! machine runs (agents/MCP, on-device models, the Gmail budget, the sync
//! window, calendar sync, spelling), and one-off state (welcome, pending
//! setups, "You"). And a device can keep any synced setting to itself:
//! keys listed in its own [`KEEP_HERE`] list are neither recorded nor
//! overwritten (Settings' "Only on this Mac / iPhone", e.g. a phone that
//! follows the system's light and dark while the Mac stays dark). See
//! docs/CROSS-DEVICE-STATE.md for the table.

use serde_json::{Map, Value};

use crate::record::Kind;

/// Top-level settings synced whole, one record each.
pub const SYNCED_SETTINGS: &[&str] = &[
    "inboxSplits",
    "getToZero",
    "remoteImages",
    "trustedImageSenders",
    "blockTrackingPixels",
    "stripLinkTracking",
    "requestReadReceipts",
    "profiles",
    "hiddenFromAll",
    "accountOrder",
    "followUpDays",
    "undoSendSeconds",
    "sendLaterHour",
    "instantReplies",
    "writeWithAi",
    "signatureDefaults",
    "signatureInsert",
    "signatureSeparator",
    "lockReplyAccount",
    "unsubscribeButton",
    "smartViews",
    // New-mail notifications: on or off, per account, known senders only.
    // (Whether the system lets Penguin notify is each device's own.)
    "notifications",
    // Split Inbox on or off (the key keeps its old name).
    "inboxTabs",
    "getToZeroButton",
    "zeroCelebration",
    // Swipes. The iPhone's own Reply Later and Snooze read as the Mac's
    // default there, and a phone keeps its own defaults until one is set.
    "swipeRight",
    "swipeLeft",
    "swipeLeftLong",
    // Appearance: the theme (light, dark or the system's; see KEEP_HERE),
    // accent, sidebar theme, dark shade, corners, list style, dark email
    // bodies, conversation order.
    "theme",
    "accentColor",
    "matchAccent",
    "sidebarTheme",
    "darkShade",
    "corners",
    "listStyle",
    "darkEmailBodies",
    "threadLatestFirst",
    // Sender photos: which sources, and where they show.
    "senderPhotos",
    "avatarPlacement",
];

/// A device's own list of synced keys it keeps to itself (a top-level
/// array of key names in its settings file; never synced). A key listed
/// here is neither recorded when it changes nor replaced by another
/// device's value; taking it off the list takes the synced value again
/// ([`unkept`]).
pub const KEEP_HERE: &str = "keepHere";

/// Lists synced per item (by the item's `"id"`): (settings key, kind).
pub const SYNCED_LISTS: &[(&str, Kind)] = &[
    ("snippets", Kind::Snippet),
    ("signatures", Kind::Signature),
];

/// Whether `settings` keeps `key` on this device (its [`KEEP_HERE`] list).
pub fn kept_here(settings: &Value, key: &str) -> bool {
    settings
        .get(KEEP_HERE)
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|k| k.as_str() == Some(key)))
}

/// Synced keys `new` no longer keeps to itself but `old` did: they should
/// take the synced value again.
pub fn unkept(old: &Value, new: &Value) -> Vec<String> {
    SYNCED_SETTINGS
        .iter()
        .filter(|k| kept_here(old, k) && !kept_here(new, k))
        .map(|k| k.to_string())
        .collect()
}

/// One change to record: (kind, key, new body; None = removed).
pub type Change = (Kind, String, Option<Value>);

/// The records a settings write produces: every synced key whose value
/// changed between `old` and `new`, and every list item added, changed or
/// removed. Unsynced keys are ignored.
pub fn changes(old: &Value, new: &Value) -> Vec<Change> {
    let empty = Map::new();
    let o = old.as_object().unwrap_or(&empty);
    let n = new.as_object().unwrap_or(&empty);
    let mut out = Vec::new();
    for key in SYNCED_SETTINGS {
        // Kept to this device, or just let go of (it takes the synced
        // value then; see `unkept`).
        if kept_here(new, key) || kept_here(old, key) {
            continue;
        }
        let (a, b) = (o.get(*key), n.get(*key));
        if a != b {
            out.push((Kind::Setting, key.to_string(), b.cloned()));
        }
    }
    for (key, kind) in SYNCED_LISTS {
        let a = items(o.get(*key));
        let b = items(n.get(*key));
        for (id, item) in &b {
            if a.iter().find(|(x, _)| x == id).map(|(_, v)| v) != Some(item) {
                out.push((*kind, id.clone(), Some((*item).clone())));
            }
        }
        for (id, _) in &a {
            if !b.iter().any(|(x, _)| x == id) {
                out.push((*kind, id.clone(), None));
            }
        }
    }
    out
}

/// Apply one synced change to a settings object. Returns whether it
/// changed anything. Kinds and keys this build doesn't sync are ignored,
/// so a newer device's settings never land in an unknown key.
pub fn apply(settings: &mut Value, kind: Kind, key: &str, body: Option<&Value>) -> bool {
    if settings.as_object().is_none() {
        return false;
    }
    if kind == Kind::Setting {
        if !SYNCED_SETTINGS.contains(&key) || kept_here(settings, key) {
            return false;
        }
        let Some(obj) = settings.as_object_mut() else {
            return false;
        };
        return match body {
            Some(v) => obj.insert(key.to_string(), v.clone()).as_ref() != Some(v),
            // A synced setting is never removed, only reset by the app's
            // defaults on read.
            None => obj.remove(key).is_some(),
        };
    }
    let Some((list_key, _)) = SYNCED_LISTS.iter().find(|(_, k)| *k == kind) else {
        return false;
    };
    let Some(obj) = settings.as_object_mut() else {
        return false;
    };
    let list = obj
        .entry(list_key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(arr) = list.as_array_mut() else {
        return false;
    };
    let pos = arr
        .iter()
        .position(|it| it.get("id").and_then(Value::as_str) == Some(key));
    match (pos, body) {
        (Some(i), Some(v)) => {
            if &arr[i] == v {
                return false;
            }
            arr[i] = v.clone();
            true
        }
        (None, Some(v)) => {
            arr.push(v.clone());
            true
        }
        (Some(i), None) => {
            arr.remove(i);
            true
        }
        (None, None) => false,
    }
}

/// Whether records of `kind` live in the settings file (and are projected
/// there by the app, not by the store).
pub fn is_settings_kind(kind: Kind) -> bool {
    kind == Kind::Setting || SYNCED_LISTS.iter().any(|(_, k)| *k == kind)
}

/// The body to record for a setting when this device keeps only part of
/// it: `ours` laid over the last synced `base`, object by object, so the
/// fields this device doesn't have survive its edits. The iPhone keeps a
/// subset of the Mac's settings (`smartViews` without the Mac's pinned
/// searches and sidebar counts, for one); it records every change with
/// this. Arrays and plain values are replaced as a whole.
pub fn overlay(base: Option<&Value>, ours: &Value) -> Value {
    match (base, ours) {
        (Some(Value::Object(b)), Value::Object(o)) => {
            let mut out = b.clone();
            for (k, v) in o {
                let merged = overlay(b.get(k), v);
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        _ => ours.clone(),
    }
}

/// What a device that is joining sync carries over from its settings
/// file: every synced key and list item where `current` differs from this
/// app's `defaults` (a default deleted is a removal). Recorded with
/// [`crate::Record::seeded`] after the first complete pull, and only for
/// what has no record yet, so a device joining later takes the settings
/// already synced instead of overwriting them with its own.
pub fn seed_changes(current: &Value, defaults: &Value) -> Vec<Change> {
    changes(defaults, current)
}

/// Apply every record of a settings kind to `settings` (startup reconcile:
/// the file is brought back in line with the records, which are written
/// first). Returns whether anything changed.
pub fn apply_records(settings: &mut Value, records: &[crate::Record]) -> bool {
    let mut changed = false;
    for r in records {
        if is_settings_kind(r.kind) {
            changed |= apply(settings, r.kind, &r.key, r.body.as_ref());
        }
    }
    changed
}

/// Settings that name accounts. Each device only shows (and so only edits)
/// the accounts signed in on it; the others belong to another device.
const ACCOUNT_LISTS: &[&str] = &["accountOrder", "hiddenFromAll"];

/// Keep what a write couldn't have meant to drop: entries in `old` for
/// accounts this device doesn't have (`known` says which it has) stay in
/// `new`, so editing the account order on the Mac doesn't lose a
/// phone-only account's place, and editing a profile here doesn't drop
/// its members from the other device. Covers `accountOrder`,
/// `hiddenFromAll`, `signatureDefaults` and `notifications.accounts` (by
/// account) and each profile's
/// `accountIds` (profiles deleted in `new` stay deleted). Run on every
/// settings write, before recording it.
pub fn keep_other_devices_accounts(old: &Value, new: &mut Value, known: &dyn Fn(&str) -> bool) {
    for key in ACCOUNT_LISTS {
        if let (Some(o), Some(n)) = (old.get(*key), new.get_mut(*key)) {
            keep_foreign_ids(o, n, known);
        }
    }
    if let (Some(Value::Object(o)), Some(Value::Object(n))) = (
        old.get("signatureDefaults"),
        new.get_mut("signatureDefaults"),
    ) {
        for (account, sig) in o {
            if !known(account) && !n.contains_key(account) {
                n.insert(account.clone(), sig.clone());
            }
        }
    }
    if let (Some(Value::Object(o)), Some(Value::Object(n))) = (
        old.pointer("/notifications/accounts"),
        new.pointer_mut("/notifications/accounts"),
    ) {
        for (account, on) in o {
            if !known(account) && !n.contains_key(account) {
                n.insert(account.clone(), on.clone());
            }
        }
    }
    if let (Some(Value::Array(o)), Some(Value::Array(n))) =
        (old.get("profiles"), new.get_mut("profiles"))
    {
        for profile in n.iter_mut() {
            let id = profile.get("id").and_then(Value::as_str);
            let Some(before) = o
                .iter()
                .find(|p| id.is_some() && p.get("id").and_then(Value::as_str) == id)
            else {
                continue;
            };
            if let (Some(b), Some(m)) = (before.get("accountIds"), profile.get_mut("accountIds")) {
                keep_foreign_ids(b, m, known);
            }
        }
    }
}

/// `new` (a list of account ids) gets back every id of `old` this device
/// doesn't know, at its old position (or the end).
fn keep_foreign_ids(old: &Value, new: &mut Value, known: &dyn Fn(&str) -> bool) {
    let (Some(o), Some(n)) = (old.as_array(), new.as_array_mut()) else {
        return;
    };
    for (i, id) in o.iter().enumerate() {
        let Some(s) = id.as_str() else { continue };
        if !known(s) && !n.iter().any(|x| x.as_str() == Some(s)) {
            let at = i.min(n.len());
            n.insert(at, id.clone());
        }
    }
}

fn items(v: Option<&Value>) -> Vec<(String, &Value)> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|it| Some((it.get("id")?.as_str()?.to_string(), it)))
                .collect()
        })
        .unwrap_or_default()
}
