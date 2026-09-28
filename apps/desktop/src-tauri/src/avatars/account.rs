//! Each signed-in account's own Google profile photo, for the places that
//! list accounts (sidebar, switcher, Settings → Accounts, compose From,
//! profiles).
//!
//! Same source as Settings → You → "Use Google profile photo" (me.rs):
//! `people/me` with the `profile` scope every account already granted, the
//! OpenID userinfo `picture` as fallback. Stored like a sender avatar: a
//! 128 px PNG we encoded ourselves in the avatar cache, served through the
//! `avatar:` scheme, with a record keyed by the account's address. A hit or
//! a miss holds for a day, so the network is asked about once a day per
//! account; the UI asks once per account per session on top of that.
//!
//! A failed refresh (offline, reconnect needed) keeps showing the last photo
//! and leaves the record stale so the next session tries again.
//!
//! Other providers: the photo comes from `MailProvider::profile_photo` when
//! the account's capabilities include `profilePhoto` (Microsoft Graph's
//! `/me/photo`), through the same cache; otherwise there is none.

use std::sync::Arc;

use tauri::State;

use super::cache::{source_key, Cache, Lookup, SourceKind};
use super::image::{normalize_raster, MAX_RASTER_BYTES, MIN_ICON_EDGE, SIZE};
use super::net::{HttpNet, Net};
use super::{image_url, now_secs, Avatars};
use crate::error::CmdResult;
use crate::me::{fetch_error, google_photo_url, sized, GooglePhoto};
use crate::state::{blocking, AppState};

fn key(email: &str) -> String {
    source_key(SourceKind::Account, &email.trim().to_ascii_lowercase())
}

/// The cached answer for an account. A record whose image file is gone
/// (the OS may wipe the cache dir) counts as missing.
pub(super) fn cached(cache: &Cache, email: &str, now: u64) -> Lookup {
    let gone = |h: &Option<String>| h.as_deref().is_some_and(|h| cache.image_path(h).is_none());
    match cache.lookup(&key(email), now) {
        Lookup::Fresh(h) | Lookup::Stale(h) if gone(&h) => Lookup::Missing,
        other => other,
    }
}

/// The photo's raw bytes, or None when the account only has Google's
/// generated letter avatar.
pub(super) async fn fetch_photo(net: &dyn Net, token: &str) -> CmdResult<Option<Vec<u8>>> {
    let url = match google_photo_url(net, token).await? {
        GooglePhoto::Url(u) => sized(&u, SIZE),
        GooglePhoto::NoPhoto => return Ok(None),
    };
    net.get(&url, MAX_RASTER_BYTES, None)
        .await
        .map(Some)
        .map_err(|e| fetch_error("Google profile photo", e))
}

/// Record a fetch's outcome and return the image hash to show. Blocking.
/// `stale` is the previous image, kept on screen when the fetch failed.
pub(super) fn settle(
    cache: &Cache,
    email: &str,
    fetched: CmdResult<Option<Vec<u8>>>,
    stale: Option<String>,
    now: u64,
) -> CmdResult<Option<String>> {
    let (key, kind) = (key(email), SourceKind::Account);
    let hash = match fetched {
        Ok(Some(bytes)) => match normalize_raster(&bytes, MIN_ICON_EDGE) {
            Ok(png) => Some(cache.put_image(&key, &png, now, kind.hit_ttl(), None)?),
            Err(e) => {
                tracing::warn!(error = %e, "account photo unreadable; using the monogram");
                cache.put_miss(&key, now, kind.miss_ttl(), None);
                None
            }
        },
        Ok(None) => {
            cache.put_miss(&key, now, kind.miss_ttl(), None);
            None
        }
        Err(e) => {
            return match stale {
                Some(h) => {
                    tracing::info!(error = %e, "account photo refresh failed; keeping the cached one");
                    Ok(Some(h))
                }
                None => Err(e),
            }
        }
    };
    cache.flush()?;
    Ok(hash)
}

type AppStateRef<'a> = State<'a, Arc<AppState>>;
type AvatarsRef<'a> = State<'a, Arc<Avatars>>;

/// The account's own profile photo as an `avatar:` URL, or null when it has
/// none (the UI keeps its monogram). Answers from the cache when fresh;
/// otherwise asks Google, so the UI calls it off the render path.
#[tauri::command]
pub async fn account_photo(
    state: AppStateRef<'_>,
    avatars: AvatarsRef<'_>,
    account_id: String,
) -> CmdResult<Option<String>> {
    let account = state.account(&account_id).await?;
    let now = now_secs();
    let (av, email) = (avatars.inner().clone(), account.email.clone());
    let stale = match blocking(move || Ok(cached(&av.resolver.cache, &email, now))).await? {
        Lookup::Fresh(hash) => return Ok(hash.map(|h| image_url(&h))),
        Lookup::Stale(hash) => hash,
        Lookup::Missing => None,
    };
    let fetched = async {
        match account.provider {
            penguin_core::AccountProvider::Gmail => {
                let token = state.services()?.auth.access_token(&account.email).await?;
                fetch_photo(&HttpNet::new(), &token).await
            }
            _ if !account.capabilities.profile_photo => Ok(None),
            _ => Ok(state.provider(&account.id).await?.profile_photo().await?),
        }
    }
    .await;
    let (av, email) = (avatars.inner().clone(), account.email.clone());
    let hash = blocking(move || settle(&av.resolver.cache, &email, fetched, stale, now)).await?;
    Ok(hash.map(|h| image_url(&h)))
}
