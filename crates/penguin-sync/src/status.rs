//! What Settings says about sync ("Sync with iCloud"), the same on the Mac
//! and the iPhone. The transport (the CloudKit shim) reports what it sees
//! ([`Report`]); the store adds what it knows (last sync, pending records);
//! [`CloudStatus`] is the two together.

use serde::{Deserialize, Serialize};

/// The device's iCloud account, as CloudKit reports it (`CKAccountStatus`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CloudAccount {
    /// Not asked yet (sync off, or the transport is still starting).
    #[default]
    Unknown,
    Available,
    /// Not signed in to iCloud.
    NoAccount,
    /// Parental controls or a device profile block iCloud for this app.
    Restricted,
    /// Signed in, but iCloud can't be used right now (a password or terms
    /// to confirm in System Settings).
    TemporarilyUnavailable,
}

/// Something the user may need to act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CloudProblem {
    /// iCloud storage is full; changes wait until there is room.
    QuotaExceeded,
    /// The app's iCloud container isn't ready for this build (its schema
    /// isn't deployed, or the build's entitlements don't match).
    NotSetUp { detail: String },
    /// The user deleted Penguin's data from iCloud (Settings → iCloud):
    /// sync turned itself off; turning it on uploads this device's copy.
    DataDeleted,
    /// Anything else, with CloudKit's error code for the log.
    Other { code: i64, detail: String },
}

/// What the transport last reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub account: CloudAccount,
    pub problem: Option<CloudProblem>,
    /// Last time records were sent (unix ms).
    pub last_send_at: Option<i64>,
}

/// Settings → Sync with iCloud.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudStatus {
    /// This build can sync on this device (it carries the iCloud
    /// entitlement and the OS has CKSyncEngine). A self-build without its
    /// own container can't.
    pub available: bool,
    /// Why not, when `available` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// The user's switch: on unless turned off.
    pub enabled: bool,
    /// Records are moving: available, enabled and an iCloud account.
    pub active: bool,
    pub account: CloudAccount,
    /// Last complete fetch or send (unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<i64>,
    /// Local changes not on iCloud yet.
    pub pending: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<CloudProblem>,
}
