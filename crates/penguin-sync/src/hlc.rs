//! Hybrid logical clocks (Kulkarni et al., "Logical Physical Clocks",
//! OPODIS 2014): a timestamp that stays close to wall time but never goes
//! backwards and is always past anything the device has seen. An edit made
//! after seeing another device's edit therefore sorts after it even when
//! this device's clock is behind.
//!
//! Skew: a device whose clock runs far ahead would otherwise win every
//! conflict until real time caught up, and drag every device that saw its
//! stamps into the future with it. [`Hlc::clamped`] stops both: it caps a
//! stamp at the time the server stored the record (CloudKit's
//! `modificationDate`, the same for every device) plus [`MAX_SKEW_MS`].
//! Merges compare clamped stamps, and devices only [`Clock::observe`]
//! clamped stamps, so a fast clock loses its head start once its record is
//! stored, and a slow clock (behind by any amount) still stamps its next
//! edit after everything it has seen.

use serde::{Deserialize, Serialize};

/// How far past the server's (or this device's) time a stamp may be.
pub const MAX_SKEW_MS: i64 = 10 * 60_000;

/// One timestamp. Ordered by wall time, then counter, then device, so two
/// devices never produce equal stamps.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hlc {
    /// Unix ms (logical: at least the wall clock, maybe a little ahead).
    #[serde(rename = "w")]
    pub wall: i64,
    /// Orders events within one `wall` millisecond.
    #[serde(rename = "c")]
    pub count: u32,
    /// The device that made it (a random id per install).
    #[serde(rename = "d")]
    pub device: String,
}

impl Hlc {
    /// The zero stamp, before everything.
    pub fn zero() -> Hlc {
        Hlc {
            wall: 0,
            count: 0,
            device: String::new(),
        }
    }

    /// This stamp as ordered once the server stored it at `server_ms`: a
    /// wall time more than [`MAX_SKEW_MS`] past the server's is capped at
    /// the server's time. `None` (not stored yet) leaves it alone.
    pub fn clamped(&self, server_ms: Option<i64>) -> Hlc {
        match server_ms {
            Some(s) if self.wall > s.saturating_add(MAX_SKEW_MS) => Hlc {
                wall: s,
                count: self.count,
                device: self.device.clone(),
            },
            _ => self.clone(),
        }
    }
}

/// A device's clock. Its state (`wall`, `count`) is persisted with the
/// records so stamps keep increasing across launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clock {
    pub wall: i64,
    pub count: u32,
    pub device: String,
}

impl Clock {
    pub fn new(device: impl Into<String>, wall: i64, count: u32) -> Clock {
        Clock {
            wall,
            count,
            device: device.into(),
        }
    }

    /// A stamp for a local event at wall time `now_ms`.
    pub fn tick(&mut self, now_ms: i64) -> Hlc {
        if now_ms > self.wall {
            self.wall = now_ms;
            self.count = 0;
        } else {
            self.count = self.count.saturating_add(1);
        }
        self.stamp()
    }

    /// Take in a stamp from another device (received at wall time
    /// `now_ms`), so this device's next stamp sorts after it. Pass the
    /// stamp as stored, [`Hlc::clamped`] to the server's time: that is what
    /// bounds a broken clock, so this follows it however far ahead it is
    /// (this device's own clock may be the one that's behind).
    pub fn observe(&mut self, remote: &Hlc, now_ms: i64) {
        let remote_wall = remote.wall;
        let wall = self.wall.max(remote_wall).max(now_ms);
        let count = if wall == self.wall && wall == remote_wall {
            self.count.max(remote.count).saturating_add(1)
        } else if wall == self.wall {
            self.count.saturating_add(1)
        } else if wall == remote_wall {
            remote.count.saturating_add(1)
        } else {
            0
        };
        self.wall = wall;
        self.count = count;
    }

    fn stamp(&self) -> Hlc {
        Hlc {
            wall: self.wall,
            count: self.count,
            device: self.device.clone(),
        }
    }
}
