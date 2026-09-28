//! Settings → You: the user's own photo. The UI crops a picked image to a
//! square and sends it here, or asks for a signed-in account's Google profile
//! photo. Either way the bytes are decoded under strict limits and re-encoded
//! as a 256 px PNG we wrote ourselves, stored as `<data dir>/me/<sha256>.png`;
//! `Settings.me.photo` holds that id. Only the current file is kept.
//!
//! The Google photo needs no extra scope: `people/me` (photos) works with the
//! `profile` scope every account already granted. When the People API is off
//! in the user's Cloud project we fall back to the OpenID userinfo `picture`.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use image::{imageops::FilterType, ImageFormat, ImageReader, Limits};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::State;

use crate::avatars::net::{FetchError, HttpNet, Net};
use crate::error::{CmdError, CmdResult};
use crate::settings::Settings;
use crate::state::{blocking, AppState};

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Stored edge; the largest "you" avatar is 64 CSS px (the person card).
pub const SIZE: u32 = 256;
/// Upload cap. The UI sends a cropped PNG of at most 1024 px (well under).
const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8192;
const MIN_EDGE: u32 = 32;

pub(crate) const PEOPLE_ME: &str = "https://people.googleapis.com/v1/people/me?personFields=photos";
const USERINFO: &str = "https://openidconnect.googleapis.com/v1/userinfo";

fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("me")
}

fn file_for(data_dir: &Path, id: &str) -> PathBuf {
    dir(data_dir).join(format!("{id}.png"))
}

/// Decode (PNG, JPEG, WebP, GIF; sniffed), center-crop to a square, shrink
/// to [`SIZE`] and re-encode as PNG.
pub fn normalize(bytes: &[u8]) -> CmdResult<Vec<u8>> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(CmdError::invalid("That image is too large (8 MB max)"));
    }
    let format = image::guess_format(bytes)
        .ok()
        .filter(|f| {
            matches!(
                f,
                ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
            )
        })
        .ok_or_else(|| CmdError::invalid("Use a PNG, JPEG, WebP or GIF image"))?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| CmdError::invalid(format!("Couldn't read that image: {e}")))?;
    let (w, h) = (img.width(), img.height());
    let edge = w.min(h);
    if edge < MIN_EDGE {
        return Err(CmdError::invalid("That image is too small"));
    }
    let img = img.crop_imm((w - edge) / 2, (h - edge) / 2, edge, edge);
    let img = if edge > SIZE {
        img.resize_exact(SIZE, SIZE, FilterType::Lanczos3)
    } else {
        img
    };
    let mut out = Vec::new();
    img.to_rgba8()
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| CmdError::other(e.to_string()))?;
    Ok(out)
}

/// Write `png` (already normalized) under its hash, point settings at it and
/// delete every other stored photo.
fn store(state: &AppState, png: Vec<u8>) -> CmdResult<Settings> {
    let id = hex(&Sha256::digest(&png));
    let d = dir(&state.paths.data_dir);
    std::fs::create_dir_all(&d)?;
    let path = file_for(&state.paths.data_dir, &id);
    let tmp = d.join(format!("{id}.tmp"));
    std::fs::write(&tmp, &png)?;
    std::fs::rename(&tmp, &path)?;
    let (saved, _) = state.settings.set_me_photo(Some(id.clone()))?;
    prune(&state.paths.data_dir, Some(&id));
    Ok(saved)
}

fn prune(data_dir: &Path, keep: Option<&str>) {
    let Ok(entries) = std::fs::read_dir(dir(data_dir)) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if keep.is_some_and(|k| name == format!("{k}.png")) {
            continue;
        }
        if let Err(err) = std::fs::remove_file(e.path()) {
            tracing::warn!(error = %err, "could not remove an old profile photo");
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn saved_view(state: &Arc<AppState>, saved: Settings) -> CmdResult<Settings> {
    let saved = crate::commands::settings_view(state, saved).await?;
    state.emit_settings_changed(saved.clone());
    Ok(saved)
}

/// Settings → You → Choose photo: `png` is the UI's cropped square (base64).
#[tauri::command]
pub async fn set_me_photo(state: AppStateRef<'_>, png: String) -> CmdResult<Settings> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png.trim())
        .map_err(|_| CmdError::invalid("The photo wasn't valid base64"))?;
    let st = state.inner().clone();
    let saved = blocking(move || store(&st, normalize(&bytes)?)).await?;
    tracing::info!("profile photo set from a file");
    saved_view(state.inner(), saved).await
}

#[tauri::command]
pub async fn clear_me_photo(state: AppStateRef<'_>) -> CmdResult<Settings> {
    let st = state.inner().clone();
    let saved = blocking(move || {
        let (saved, _) = st.settings.set_me_photo(None)?;
        prune(&st.paths.data_dir, None);
        Ok(saved)
    })
    .await?;
    saved_view(state.inner(), saved).await
}

/// The stored photo as a `data:image/png` URL (null when none). The app CSP
/// allows data: images; the file is ours and at most a few dozen KB.
#[tauri::command]
pub async fn me_photo(state: AppStateRef<'_>) -> CmdResult<Option<String>> {
    let Some(id) = state.settings.get().me.photo else {
        return Ok(None);
    };
    let path = file_for(&state.paths.data_dir, &id);
    blocking(move || match std::fs::read(&path) {
        Ok(b) => Ok(Some(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b)
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    })
    .await
}

// ---------- Google profile photo ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct PeopleMe {
    photos: Vec<PeoplePhoto>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PeoplePhoto {
    url: String,
    default: bool,
    metadata: Option<PhotoMeta>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PhotoMeta {
    primary: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UserInfo {
    picture: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GooglePhoto {
    Url(String),
    /// Only Google's generated letter avatar.
    NoPhoto,
}

/// The primary non-default photo from a `people/me` response.
fn parse_people_me(json: &[u8]) -> Result<GooglePhoto, String> {
    let me: PeopleMe = serde_json::from_slice(json).map_err(|e| format!("People API: {e}"))?;
    let mut photos: Vec<&PeoplePhoto> = me.photos.iter().filter(|p| !p.url.is_empty()).collect();
    photos.sort_by_key(|p| !p.metadata.as_ref().is_some_and(|m| m.primary));
    match photos.first() {
        Some(p) if !p.default => Ok(GooglePhoto::Url(p.url.clone())),
        _ => Ok(GooglePhoto::NoPhoto),
    }
}

/// Google photo URLs end in a size option (`=s100`, `=s96-c`); ask for one
/// `size` px, cropped square.
pub(crate) fn sized(url: &str, size: u32) -> String {
    let base = match url.rfind('=') {
        Some(i) if url[i + 1..].starts_with('s') && !url[i..].contains('/') => &url[..i],
        _ => url,
    };
    format!("{base}=s{size}-c")
}

pub(crate) fn fetch_error(what: &str, e: FetchError) -> CmdError {
    match e {
        FetchError::Unauthorized => CmdError::new(
            crate::error::ErrorCode::NeedsReauth,
            "Google rejected the sign-in; reconnect this account",
        ),
        FetchError::Transient(m) => {
            CmdError::new(crate::error::ErrorCode::Network, format!("{what}: {m}"))
        }
        FetchError::NotFound | FetchError::Forbidden(_) => {
            CmdError::not_found(format!("{what}: not available"))
        }
    }
}

pub(crate) async fn google_photo_url(net: &dyn Net, token: &str) -> CmdResult<GooglePhoto> {
    match net.get(PEOPLE_ME, 256 * 1024, Some(token)).await {
        Ok(body) => return parse_people_me(&body).map_err(CmdError::other),
        // 403 = the People API is off in the Cloud project (or a policy);
        // the OpenID userinfo endpoint needs no API switch.
        Err(FetchError::Forbidden(_) | FetchError::NotFound) => {}
        Err(e) => return Err(fetch_error("Google profile", e)),
    }
    let body = net
        .get(USERINFO, 64 * 1024, Some(token))
        .await
        .map_err(|e| fetch_error("Google profile", e))?;
    let info: UserInfo =
        serde_json::from_slice(&body).map_err(|e| CmdError::other(e.to_string()))?;
    Ok(info
        .picture
        .filter(|p| !p.is_empty())
        .map_or(GooglePhoto::NoPhoto, GooglePhoto::Url))
}

/// Settings → You → "Use Google profile photo": the account's own photo.
#[tauri::command]
pub async fn set_me_photo_from_google(
    state: AppStateRef<'_>,
    account_id: String,
) -> CmdResult<Settings> {
    let account = state.account(&account_id).await?;
    if account.provider != penguin_core::AccountProvider::Gmail {
        return Err(CmdError::invalid(format!(
            "{} isn't a Google account",
            account.email
        )));
    }
    let services = state.services()?;
    let token = services.auth.access_token(&account.email).await?;
    let net = HttpNet::new();
    let url = match google_photo_url(&net, &token).await? {
        GooglePhoto::Url(u) => sized(&u, SIZE),
        GooglePhoto::NoPhoto => {
            return Err(CmdError::not_found(format!(
                "{} has no Google profile photo",
                account.email
            )))
        }
    };
    let bytes = net
        .get(&url, MAX_INPUT_BYTES, None)
        .await
        .map_err(|e| fetch_error("Google profile photo", e))?;
    let st = state.inner().clone();
    let saved = blocking(move || store(&st, normalize(&bytes)?)).await?;
    tracing::info!(account = %account.id, "profile photo set from Google");
    saved_view(state.inner(), saved).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn photos_are_squared_shrunk_and_reencoded() {
        let out = normalize(&png(900, 600)).unwrap();
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!((img.width(), img.height()), (SIZE, SIZE));
        assert_eq!(&out[..8], b"\x89PNG\r\n\x1a\n");
        let small = image::load_from_memory(&normalize(&png(100, 120)).unwrap()).unwrap();
        assert_eq!((small.width(), small.height()), (100, 100));
    }

    #[test]
    fn junk_tiny_and_svg_are_refused() {
        assert!(normalize(b"not an image").is_err());
        assert!(normalize(&png(10, 10)).is_err());
        assert!(normalize(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_err());
        assert!(normalize(&vec![0u8; MAX_INPUT_BYTES + 1]).is_err());
    }

    #[test]
    fn people_me_prefers_primary_and_skips_default_photos() {
        let json = br#"{"photos":[
            {"url":"https://lh3.googleusercontent.com/other=s100","metadata":{"primary":false}},
            {"url":"https://lh3.googleusercontent.com/a/me=s100","metadata":{"primary":true}}]}"#;
        assert_eq!(
            parse_people_me(json).unwrap(),
            GooglePhoto::Url("https://lh3.googleusercontent.com/a/me=s100".into())
        );
        let letter = br#"{"photos":[{"url":"https://lh3.googleusercontent.com/a/x=s100","default":true,"metadata":{"primary":true}}]}"#;
        assert_eq!(parse_people_me(letter).unwrap(), GooglePhoto::NoPhoto);
        assert_eq!(parse_people_me(b"{}").unwrap(), GooglePhoto::NoPhoto);
    }

    #[test]
    fn google_photo_urls_are_resized() {
        assert_eq!(
            sized("https://lh3.googleusercontent.com/a/ACg8=s96-c", SIZE),
            "https://lh3.googleusercontent.com/a/ACg8=s256-c"
        );
        assert_eq!(
            sized("https://lh3.googleusercontent.com/a-/AOh14", SIZE),
            "https://lh3.googleusercontent.com/a-/AOh14=s256-c"
        );
    }
}
