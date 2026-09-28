//! On-demand fetches for the "Message details" view: every header (for
//! authentication and transport info the sync doesn't store) and the raw
//! RFC 822 source ("Show original"). Both are user-initiated, so they go out
//! at interactive priority through the same paced client as everything else;
//! nothing here runs during sync.

use serde::Deserialize;

use crate::api::{Cost, GmailClient, Priority, LIGHT_GET};
use crate::convert::b64url_decode;
use crate::{Error, Result};

/// All of a message's headers, in the order Gmail returns them (topmost
/// first), plus Gmail's size estimate for the whole message.
pub use penguin_provider::MessageMetadata;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct MetadataWire {
    size_estimate: Option<u64>,
    payload: Option<PayloadWire>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PayloadWire {
    headers: Vec<HeaderWire>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HeaderWire {
    name: String,
    value: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawWire {
    raw: Option<String>,
}

impl GmailClient {
    /// messages.get format=metadata (every header), ~20 units.
    pub async fn get_message_metadata(&self, id: &str) -> Result<Option<MessageMetadata>> {
        let path = format!("/messages/{id}");
        let wire: MetadataWire = match self
            .get_json(
                &path,
                &[("format", "metadata".to_string())],
                LIGHT_GET,
                Priority::Interactive,
            )
            .await
        {
            Ok(w) => w,
            Err(Error::Http { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(Some(MessageMetadata {
            headers: wire
                .payload
                .map(|p| p.headers.into_iter().map(|h| (h.name, h.value)).collect())
                .unwrap_or_default(),
            size_estimate: wire.size_estimate,
        }))
    }

    /// messages.get format=raw: the RFC 822 source, decoded. Charged like a
    /// full get (learned, ~60 units).
    pub async fn get_message_raw(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let path = format!("/messages/{id}");
        let wire: RawWire = match self
            .get_json(
                &path,
                &[("format", "raw".to_string())],
                Cost::Get,
                Priority::Interactive,
            )
            .await
        {
            Ok(w) => w,
            Err(Error::Http { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        let raw = wire
            .raw
            .ok_or_else(|| Error::Other(format!("message {id}: format=raw returned no source")))?;
        b64url_decode(&raw)
            .map(Some)
            .ok_or_else(|| Error::Other(format!("message {id}: raw source is not valid base64url")))
    }
}
