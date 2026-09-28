//! Graph's part of the sync cursor (`SyncCursor.provider_state`), as JSON:
//! `{"v":1,"folders":{"<folder id>":{"deltaLink":"…","next":"…"}}}`.
//!
//! Delta works one folder at a time, so each synced folder has its own
//! `deltaLink` (where the next incremental round starts) and, while a
//! round is being paged through, `next` (the page to resume from after a
//! restart). A folder without a `deltaLink` hasn't finished its first
//! (backfill) round. `plain` records that Graph refused the newest-first
//! delta for that folder, so it's paged in Graph's own order.

use std::collections::BTreeMap;

use penguin_provider::{Error, Result};
use serde::{Deserialize, Serialize};

pub const CURSOR_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FolderCursor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub plain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphCursor {
    pub v: u32,
    pub folders: BTreeMap<String, FolderCursor>,
}

impl Default for GraphCursor {
    fn default() -> Self {
        GraphCursor {
            v: CURSOR_VERSION,
            folders: BTreeMap::new(),
        }
    }
}

impl GraphCursor {
    /// Read `provider_state` ("" = nothing yet).
    pub fn parse(state: &str) -> Result<GraphCursor> {
        if state.trim().is_empty() {
            return Ok(GraphCursor::default());
        }
        let c: GraphCursor = serde_json::from_str(state)
            .map_err(|e| Error::Other(format!("Microsoft sync state is unreadable: {e}")))?;
        if c.v > CURSOR_VERSION {
            return Err(Error::Other(format!(
                "Microsoft sync state v{} is newer than this build",
                c.v
            )));
        }
        Ok(c)
    }

    pub fn to_state(&self) -> String {
        if self.folders.is_empty() {
            return String::new();
        }
        serde_json::to_string(self).expect("plain struct serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_stays_small() {
        let mut c = GraphCursor::default();
        assert_eq!(c.to_state(), "");
        assert_eq!(GraphCursor::parse("").unwrap(), c);
        c.folders.insert(
            "F-inbox".into(),
            FolderCursor {
                delta_link: Some("https://graph.example/v1.0/me/mailFolders('F-inbox')/messages/delta?$deltatoken=abc".into()),
                next: None,
                plain: false,
            },
        );
        c.folders.insert(
            "F-sent".into(),
            FolderCursor {
                delta_link: None,
                next: Some("https://graph.example/next?$skiptoken=1".into()),
                plain: true,
            },
        );
        let s = c.to_state();
        assert!(
            s.starts_with(r#"{"v":1,"folders":{"F-inbox":{"deltaLink":"#),
            "{s}"
        );
        assert!(!s.contains("\"next\":null") && s.contains("\"plain\":true"));
        assert_eq!(GraphCursor::parse(&s).unwrap(), c);
        assert!(GraphCursor::parse(r#"{"v":2,"folders":{}}"#).is_err());
        assert!(GraphCursor::parse("{oops").is_err());
    }
}
