//! The record model and the merge rule.
//!
//! A [`Record`] is one piece of synced state, named by its [`Kind`] and a
//! `key` (for thread state, [`thread_key`] of account and thread). Its
//! version is three stamps deep:
//!
//! - `gen`: when the user intent behind this version was made (snooze it,
//!   unsnooze it, dismiss it, change a setting). Every user edit starts a
//!   new generation stamped with the device's [`Clock`](crate::Clock).
//! - `phase`: how far that intent has progressed. Only system events move
//!   it, and only forward: a snooze is 0 while waiting, 1 once it ended by
//!   itself (the thread was trashed, or a reply brought it back), 2 once a
//!   device woke it. Plain values (settings, dismissals) stay at 0.
//! - `at`: when this exact version was written; the last tie-break.
//!
//! [`merge`] keeps the version with the larger `(gen, phase, at)`. A newer
//! intent beats anything about an older one (re-snoozing on an offline
//! phone beats the Mac waking the old snooze), and within one intent a
//! later phase beats an earlier one (a woken snooze can't be un-woken by a
//! device that hadn't heard yet). Deletes are records too (`body: None`,
//! a tombstone with its own `gen`), so "unsnoozed" and "snoozed again"
//! compete by the same rule. Because the order is total and every device
//! applies the same rule, merging is commutative, associative and
//! idempotent: devices converge whatever order records arrive in.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::hlc::Hlc;

/// What a record holds. Serialized by its tag, which is also part of the
/// record's name. A kind from a newer app reads as `Unknown` and is kept
/// as is, never projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// A snoozed conversation: [`SnoozeBody`], key = [`thread_key`].
    Snooze,
    /// A conversation hidden from Follow up: [`DismissalBody`], key =
    /// [`thread_key`].
    FollowUpDismissal,
    /// Remind me if nobody replies (outbox reminders):
    /// [`crate::outbox::ReminderBody`], key = [`thread_key`] of account and
    /// reminder id. Any device may fire one once it has pulled past its
    /// time; firing is a terminal phase.
    Reminder,
    /// A send-later schedule: [`crate::outbox::ScheduledSendBody`], key =
    /// [`thread_key`] of account and schedule id. Exactly one device sends
    /// it (see [`crate::outbox`]).
    ScheduledSend,
    /// One top-level setting (key = its JSON name; see [`crate::settings`]).
    Setting,
    /// One snippet (key = its id).
    Snippet,
    /// One signature (key = its id).
    Signature,
    #[serde(other)]
    Unknown,
}

impl Kind {
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Snooze => "snooze",
            Kind::FollowUpDismissal => "followUpDismissal",
            Kind::Reminder => "reminder",
            Kind::ScheduledSend => "scheduledSend",
            Kind::Setting => "setting",
            Kind::Snippet => "snippet",
            Kind::Signature => "signature",
            Kind::Unknown => "unknown",
        }
    }
}

/// One piece of synced state. See the module docs for the stamps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub kind: Kind,
    pub key: String,
    pub gen: Hlc,
    #[serde(default)]
    pub phase: u8,
    pub at: Hlc,
    /// None: deleted (a tombstone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// Which side [`merge`] kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    Local,
    Remote,
    /// The same version on both sides.
    Same,
}

impl Record {
    /// A new user intent: `gen` and `at` both `stamp`, phase 0.
    pub fn intent(kind: Kind, key: impl Into<String>, stamp: Hlc, body: Option<Value>) -> Record {
        Record {
            kind,
            key: key.into(),
            gen: stamp.clone(),
            phase: 0,
            at: stamp,
            body,
        }
    }

    /// A record that only carries over what a device already had when it
    /// joined sync (its settings file before the first sync): `gen` is the
    /// zero stamp of `stamp`'s device, so any real edit, made on any device
    /// at any time, wins over it, and two such records are decided by `at`
    /// like any other tie.
    pub fn seeded(kind: Kind, key: impl Into<String>, stamp: Hlc, body: Option<Value>) -> Record {
        Record {
            kind,
            key: key.into(),
            gen: Hlc {
                wall: 0,
                count: 0,
                device: stamp.device.clone(),
            },
            phase: 0,
            at: stamp,
            body,
        }
    }

    /// The next phase of this record's intent (same `gen`), written at
    /// `stamp`. Phases never go backwards: a lower `phase` keeps the
    /// current one.
    pub fn advanced(&self, phase: u8, stamp: Hlc, body: Option<Value>) -> Record {
        Record {
            kind: self.kind,
            key: self.key.clone(),
            gen: self.gen.clone(),
            phase: phase.max(self.phase),
            at: stamp,
            body,
        }
    }

    pub fn name(&self) -> String {
        record_name(self.kind, &self.key)
    }

    pub fn is_deleted(&self) -> bool {
        self.body.is_none()
    }

    /// The merge order once stored at `server_ms` (see [`Hlc::clamped`]).
    pub fn order(&self, server_ms: Option<i64>) -> (Hlc, u8, Hlc) {
        (
            self.gen.clamped(server_ms),
            self.phase,
            self.at.clamped(server_ms),
        )
    }

    /// The body as `T` (None for a tombstone or a body this build can't
    /// read).
    pub fn body_as<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        self.body
            .as_ref()
            .and_then(|b| serde_json::from_value(b.clone()).ok())
    }
}

/// Merge a local version with a remote one, each with the time the server
/// stored it (None: not stored yet). Larger `(gen, phase, at)` wins; equal
/// orders with different bodies (impossible unless a device reused a
/// stamp) fall back to comparing the bodies' JSON, so both sides still
/// pick the same one.
///
/// Outbox records ([`Kind::ScheduledSend`], [`Kind::Reminder`]) add two
/// rules (see [`crate::outbox`]): a terminal phase (sent, fired, given up)
/// beats everything, a newer generation included, so nothing can bring a
/// sent message back to be sent again; and two claims to send the same
/// schedule at the same phase are decided by the server, not the clocks:
/// the one the server stored wins (CloudKit stores only one of two
/// concurrent saves; the other comes back as a conflict and must yield).
pub fn merge(local: (&Record, Option<i64>), remote: (&Record, Option<i64>)) -> Winner {
    if crate::outbox::is_outbox(local.0.kind) && local.0.kind == remote.0.kind {
        let (lt, rt) = (
            crate::outbox::is_terminal(local.0),
            crate::outbox::is_terminal(remote.0),
        );
        if lt != rt {
            return if lt { Winner::Local } else { Winner::Remote };
        }
        if let Some(w) = crate::outbox::claim_winner(local, remote) {
            return w;
        }
    }
    let l = local.0.order(local.1);
    let r = remote.0.order(remote.1);
    match l.cmp(&r) {
        std::cmp::Ordering::Greater => Winner::Local,
        std::cmp::Ordering::Less => Winner::Remote,
        std::cmp::Ordering::Equal => {
            if local.0.body == remote.0.body && local.0.kind == remote.0.kind {
                return Winner::Same;
            }
            let lb = serde_json::to_string(&local.0.body).unwrap_or_default();
            let rb = serde_json::to_string(&remote.0.body).unwrap_or_default();
            if lb >= rb {
                Winner::Local
            } else {
                Winner::Remote
            }
        }
    }
}

/// The record's name on the server: the kind, then a hash of kind and key,
/// so account addresses and thread ids are not visible in record names
/// (bodies are encrypted fields; names are not).
pub fn record_name(kind: Kind, key: &str) -> String {
    let mut h = Sha256::new();
    h.update(kind.tag().as_bytes());
    h.update([0u8]);
    h.update(key.as_bytes());
    let digest = h.finalize();
    let hex: String = digest[..20].iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{hex}", kind.tag())
}

/// Separator between account and thread in a thread record's key.
const SEP: char = '\u{1f}';

/// Key for per-conversation state. Account ids are lowercased addresses,
/// the same on every device; thread ids are the provider's (Gmail's thread
/// id, Graph's conversationId; IMAP's are computed locally and may differ
/// between devices, see the design doc).
pub fn thread_key(account_id: &str, thread_id: &str) -> String {
    format!("{}{SEP}{thread_id}", account_id.to_lowercase())
}

/// The (account, thread) of a [`thread_key`].
pub fn split_thread_key(key: &str) -> Option<(&str, &str)> {
    key.split_once(SEP)
}

/// Where a snooze is. The phase is derived from it ([`SnoozeState::phase`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnoozeState {
    /// Waiting for `wake_at`.
    Snoozed,
    /// Ended by itself before waking (a reply brought it back to the inbox,
    /// or the thread was trashed, marked spam or deleted).
    Ended,
    /// A device woke it and told the provider.
    Woken,
}

impl SnoozeState {
    pub fn phase(self) -> u8 {
        match self {
            SnoozeState::Snoozed => 0,
            SnoozeState::Ended => 1,
            SnoozeState::Woken => 2,
        }
    }
}

/// Body of a [`Kind::Snooze`] record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozeBody {
    pub state: SnoozeState,
    /// Unix ms it wakes.
    pub wake_at: i64,
    /// Unix ms it was snoozed (wall time on the device that snoozed it).
    pub snoozed_at: i64,
    /// Unix ms it woke or ended (Woken / Ended only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

impl SnoozeBody {
    pub fn snoozed(wake_at: i64, snoozed_at: i64) -> SnoozeBody {
        SnoozeBody {
            state: SnoozeState::Snoozed,
            wake_at,
            snoozed_at,
            ended_at: None,
        }
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("plain struct")
    }
}

/// Body of a [`Kind::FollowUpDismissal`] record: the date of the sent
/// message that was being waited on (the dismissal holds until something
/// newer is sent in the thread).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DismissalBody {
    pub sent_at: i64,
}

impl DismissalBody {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("plain struct")
    }
}
