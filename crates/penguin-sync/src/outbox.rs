//! Send later and "remind me if no reply" across devices
//! (docs/CROSS-DEVICE-STATE.md, "The outbox").
//!
//! **A scheduled send is sent by exactly one device.**
//! - The device that scheduled it is its `owner` and sends it on time,
//!   without asking anyone, for [`OWNER_FREE_MS`] after it is due.
//! - Every other device shows it, can cancel it (a tombstone, a new
//!   generation), and may **take over** once it is [`TAKEOVER_AFTER_MS`]
//!   overdue and still not sent: the owner is asleep, offline or gone.
//! - Taking over is a **claim**: the next phase of the schedule's record,
//!   naming the claimer. A device sends a claimed schedule only once the
//!   server has stored *its* claim. CloudKit stores one of two concurrent
//!   saves and returns the other as a conflict; [`claim_winner`] makes the
//!   conflicting claim yield to the stored one, so two devices can never
//!   both hold a confirmed claim.
//! - Past [`OWNER_FREE_MS`] the owner claims too, so a late owner and a
//!   device taking over go through the same gate. The gap between the two
//!   windows (five minutes) absorbs clock skew and a send in flight.
//! - Sending (or giving up) writes a terminal phase, which beats every
//!   other version, a newer generation included: a sent message is never
//!   brought back to be sent again.
//! - Taking over needs a provider whose send consumes a server draft
//!   (Gmail's `drafts.send`; a second send of the same draft is a 404), and
//!   the draft's id. Mail whose send can't be checked that way (IMAP,
//!   SMTP) is only ever sent by its owner. For Gmail the draft is the
//!   backstop: even an owner that comes back after its claim couldn't be
//!   confirmed for [`CLAIM_STALE_MS`] can't send twice.
//!
//! **Reminders** follow the snooze pattern: any device may fire a due
//! reminder (an idempotent INBOX + UNREAD), but only after a pull that
//! completed after it was due, so a reminder cancelled or moved elsewhere
//! isn't fired here; firing is a terminal phase.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::record::{Kind, Record, Winner};

const MIN: i64 = 60_000;

/// How long past due the owner sends without claiming.
pub const OWNER_FREE_MS: i64 = 10 * MIN;
/// How long past due another device may take a schedule over.
pub const TAKEOVER_AFTER_MS: i64 = 15 * MIN;
/// How soon a device waiting for its claim to be stored looks again.
pub const CLAIM_POLL_MS: i64 = 30_000;
/// A claim not followed by a send this long is abandoned: another device
/// may claim again, and an owner whose own claim never reached the server
/// sends anyway (its Gmail draft prevents a second copy).
pub const CLAIM_STALE_MS: i64 = 30 * MIN;
/// How long a device that may not send (another device's mail it can't
/// take over, or sync off) waits before looking again.
pub const IDLE_RECHECK_MS: i64 = 60 * MIN;
/// How soon a reminder waiting for a fresh pull looks again.
pub const PULL_WAIT_MS: i64 = 60_000;

/// The phase of a sent, fired or abandoned outbox record. Beats all else.
pub const PHASE_TERMINAL: u8 = 255;
/// Claims are phases 1…MAX_CLAIM_PHASE (a stale claim is claimed again
/// one phase up).
pub const MAX_CLAIM_PHASE: u8 = 200;

/// How an outbox record ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// The message went out.
    Sent,
    /// The draft was gone when it was due (sent or deleted elsewhere).
    Cancelled,
    /// The provider refused it for good.
    Failed,
    /// The reminder brought the conversation back.
    Fired,
    /// Someone replied, so the reminder had nothing to do.
    Answered,
}

/// Body of a [`Kind::ScheduledSend`] record. No subject, recipients or
/// text: those are in the draft, which the provider syncs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledSendBody {
    pub account_id: String,
    /// The provider's draft id; empty when the owner keeps the message
    /// only on itself (then nobody else can send it).
    #[serde(default)]
    pub draft_id: String,
    /// When the user asked for it to go (unix ms).
    pub send_at: i64,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remind_after_ms: Option<i64>,
    /// The device that scheduled it (`sync_state.device`).
    pub owner: String,
    /// The device whose claim this is (claimed phases only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<i64>,
    /// Terminal phase only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}

impl ScheduledSendBody {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("plain struct")
    }
}

/// Body of a [`Kind::Reminder`] record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderBody {
    pub account_id: String,
    pub thread_id: String,
    /// Only replies after this count (unix ms).
    pub sent_at: i64,
    pub remind_at: i64,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_message_id: Option<String>,
    /// Terminal phase only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}

impl ReminderBody {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("plain struct")
    }
}

pub fn is_outbox(kind: Kind) -> bool {
    matches!(kind, Kind::ScheduledSend | Kind::Reminder)
}

/// Sent, fired or given up (deleted by the user is a tombstone, not this).
pub fn is_terminal(r: &Record) -> bool {
    is_outbox(r.kind) && r.phase == PHASE_TERMINAL
}

fn claimer(r: &Record) -> Option<String> {
    r.body_as::<ScheduledSendBody>().and_then(|b| b.claimed_by)
}

/// Two different claims on the same schedule at the same phase: the one
/// the server stored wins (the earlier one if both were). None: not that
/// case, or the server hasn't decided (neither is stored): the normal
/// order applies, and neither device sends until its claim is stored.
pub fn claim_winner(
    local: (&Record, Option<i64>),
    remote: (&Record, Option<i64>),
) -> Option<Winner> {
    let (l, r) = (local.0, remote.0);
    if l.kind != Kind::ScheduledSend
        || r.kind != Kind::ScheduledSend
        || l.gen != r.gen
        || l.phase != r.phase
        || l.phase == 0
        || l.phase == PHASE_TERMINAL
    {
        return None;
    }
    let (lc, rc) = (claimer(l), claimer(r));
    if lc.is_none() || rc.is_none() || lc == rc {
        return None;
    }
    match (local.1, remote.1) {
        (Some(_), None) => Some(Winner::Local),
        (None, Some(_)) => Some(Winner::Remote),
        (Some(a), Some(b)) if a != b => Some(if a < b { Winner::Local } else { Winner::Remote }),
        _ => None,
    }
}

/// Where this device's copy of a record stands with the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OnServer {
    /// The server stored exactly this version (nothing left to send).
    pub stored: bool,
    /// The server has had some version of it (so other devices may know).
    pub ever: bool,
}

/// What a device should do with a due outbox record.
#[derive(Debug, Clone, PartialEq)]
pub enum Turn {
    /// Send it (fire it) now.
    Send,
    /// Write this claim (the record's next phase), then wait for the
    /// server to store it.
    Claim { phase: u8, body: Value, until: i64 },
    /// Not yet, or not this device: look again at `until`.
    Wait { until: i64 },
    /// Sent, fired, given up or cancelled: drop the local copy.
    Gone,
}

/// Whose turn it is to send the schedule `rec` at `now`. `me` is this
/// device; `takeover` whether another device could send it too (a provider
/// whose send consumes the draft, and a draft id); `sync_on` whether
/// records move at all (off: only the owner ever sends).
pub fn send_turn(
    rec: &Record,
    on_server: OnServer,
    me: &str,
    now: i64,
    takeover: bool,
    sync_on: bool,
) -> Turn {
    if rec.is_deleted() || is_terminal(rec) {
        return Turn::Gone;
    }
    let Some(body) = rec.body_as::<ScheduledSendBody>() else {
        // A newer app's format: leave it to that app.
        return Turn::Wait {
            until: now + IDLE_RECHECK_MS,
        };
    };
    let mine = body.owner == me;
    let takeover = takeover && !body.draft_id.is_empty();
    if !sync_on {
        return if mine {
            Turn::Send
        } else {
            Turn::Wait {
                until: now + IDLE_RECHECK_MS,
            }
        };
    }
    let claim = |phase: u8| {
        let mut b = body.clone();
        b.claimed_by = Some(me.to_string());
        b.claimed_at = Some(now);
        Turn::Claim {
            phase,
            body: b.to_value(),
            until: now + CLAIM_POLL_MS,
        }
    };
    if rec.phase > 0 {
        let at = body.claimed_at.unwrap_or(now);
        let stale = now - at >= CLAIM_STALE_MS;
        if body.claimed_by.as_deref() == Some(me) {
            return if on_server.stored || (mine && stale) {
                Turn::Send
            } else {
                Turn::Wait {
                    until: now + CLAIM_POLL_MS,
                }
            };
        }
        return if stale && (mine || takeover) && rec.phase < MAX_CLAIM_PHASE {
            claim(rec.phase + 1)
        } else {
            Turn::Wait {
                until: (at + CLAIM_STALE_MS).max(now + CLAIM_POLL_MS),
            }
        };
    }
    if mine {
        // Nobody else can know of it, or nobody else could send it.
        if now < body.send_at + OWNER_FREE_MS || !on_server.ever || !takeover {
            return Turn::Send;
        }
        return claim(1);
    }
    if !takeover {
        return Turn::Wait {
            until: now + IDLE_RECHECK_MS,
        };
    }
    if now >= body.send_at + TAKEOVER_AFTER_MS {
        claim(1)
    } else {
        Turn::Wait {
            until: body.send_at + TAKEOVER_AFTER_MS,
        }
    }
}

/// Whether to fire the reminder `rec` at `now`: `pulled_after_due` says a
/// pull that started after it was due has completed (always true with
/// sync off).
pub fn reminder_turn(rec: &Record, now: i64, pulled_after_due: bool) -> Turn {
    if rec.is_deleted() || is_terminal(rec) {
        return Turn::Gone;
    }
    if pulled_after_due {
        Turn::Send
    } else {
        Turn::Wait {
            until: now + PULL_WAIT_MS,
        }
    }
}

/// The terminal body for `rec` (its body with `outcome`).
pub fn finished_body(rec: &Record, outcome: Outcome) -> Option<Value> {
    let mut v = rec.body.clone()?;
    if let Some(o) = v.as_object_mut() {
        o.insert(
            "outcome".into(),
            serde_json::to_value(outcome).expect("plain enum"),
        );
    }
    Some(v)
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
