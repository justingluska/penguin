//! The user's own Microsoft Entra app registration: only its Application
//! (client) ID, kept in `<config dir>/microsoft-oauth-client.json` beside the
//! Google client. It's a public client (no secret, PKCE), so the ID is
//! configuration, not a credential, and doesn't go in the Keychain.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE: &str = "microsoft-oauth-client.json";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    client_id: String,
}

pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

/// The saved client ID; None when missing or unreadable (the setup screen
/// then asks for it again).
pub fn load(config_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path(config_dir)).ok()?;
    let stored: Stored = serde_json::from_str(&text).ok()?;
    normalize_client_id(&stored.client_id).ok()
}

/// Save the client ID, or remove the file for None.
pub fn save(config_dir: &Path, client_id: Option<&str>) -> std::io::Result<()> {
    let file = path(config_dir);
    match client_id {
        None => match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
        Some(id) => {
            std::fs::create_dir_all(config_dir)?;
            let json = serde_json::to_vec_pretty(&Stored {
                client_id: id.to_string(),
            })
            .map_err(std::io::Error::other)?;
            crate::avatars::cache::write_atomic(&file, &json)
        }
    }
}

pub use penguin_onboard::normalize_client_id;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microsoft_client_id_round_trips_through_the_config_dir() {
        let dir = std::env::temp_dir().join(format!("penguin-ms-client-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), None);
        save(&dir, Some("1b2c3d4e-0000-1111-2222-333344445555")).unwrap();
        assert_eq!(
            load(&dir).as_deref(),
            Some("1b2c3d4e-0000-1111-2222-333344445555")
        );
        save(&dir, None).unwrap();
        assert_eq!(load(&dir), None);
        save(&dir, None).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
