//! IMAP's part of the sync cursor (`SyncCursor.provider_state`, docs/PROVIDERS-IMPL.md §5):
//!
//! ```json
//! {"v":1,"folders":{"INBOX":{"uidvalidity":7,"uidnext":4012,"highestmodseq":9001,
//!   "fillBelow":3800,"olderBelow":null}}}
//! ```
//!
//! Per folder: the UIDVALIDITY the rest is valid for, the incremental anchor
//! (`uidnext`: everything below it has been seen or belongs to a backfill
//! pass), the CONDSTORE position, and each backfill pass's progress as "the
//! lowest UID still to look at, exclusive" (`None` = that pass is done for
//! the folder).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FolderState {
    pub uidvalidity: u32,
    /// Incremental anchor: UIDs ≥ this are new mail.
    pub uidnext: u32,
    pub highestmodseq: Option<u64>,
    /// Window fill: UIDs below this are still to be looked at (None: done).
    pub fill_below: Option<u32>,
    /// Older-mail pass: likewise (None: not started or done; see `older_done`).
    pub older_below: Option<u32>,
    pub older_done: bool,
    /// Unix ms of the last full flag scan (servers without CONDSTORE).
    pub last_flag_scan_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImapCursor {
    pub v: u32,
    /// Wire folder name → state.
    pub folders: BTreeMap<String, FolderState>,
}

impl Default for ImapCursor {
    fn default() -> Self {
        ImapCursor {
            v: VERSION,
            folders: BTreeMap::new(),
        }
    }
}

impl ImapCursor {
    /// Read `provider_state` ("" = nothing yet).
    pub fn parse(state: &str) -> Result<ImapCursor> {
        if state.trim().is_empty() {
            return Ok(ImapCursor::default());
        }
        let c: ImapCursor = serde_json::from_str(state)
            .map_err(|e| Error::Other(format!("IMAP sync state is unreadable: {e}")))?;
        if c.v > VERSION {
            return Err(Error::Other(format!(
                "IMAP sync state v{} is newer than this build",
                c.v
            )));
        }
        Ok(c)
    }

    pub fn to_state(&self) -> String {
        serde_json::to_string(self).expect("plain struct serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_pins_the_shape() {
        assert_eq!(ImapCursor::parse("").unwrap(), ImapCursor::default());
        let mut c = ImapCursor::default();
        c.folders.insert(
            "INBOX".into(),
            FolderState {
                uidvalidity: 7,
                uidnext: 4012,
                highestmodseq: Some(9001),
                fill_below: Some(3800),
                ..FolderState::default()
            },
        );
        let s = c.to_state();
        assert!(s.starts_with(r#"{"v":1,"folders":{"INBOX":{"uidvalidity":7,"uidnext":4012,"highestmodseq":9001,"fillBelow":3800"#), "{s}");
        assert_eq!(ImapCursor::parse(&s).unwrap(), c);
        assert!(ImapCursor::parse(r#"{"v":2}"#).is_err());
        assert!(ImapCursor::parse("{bad").is_err());
        // Unknown fields from a newer minor change are ignored.
        let c2 = ImapCursor::parse(r#"{"v":1,"folders":{"A":{"uidvalidity":1,"x":2}}}"#).unwrap();
        assert_eq!(c2.folders["A"].uidvalidity, 1);
        assert!(s.len() < 64 * 1024);
    }
}
