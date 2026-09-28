//! The `avatar:` URI scheme: serves cached avatar PNGs to the webview.
//!
//! Why a protocol rather than data: URLs from a command: list rows are
//! virtualized and remount constantly. A stable, content-addressed URL lets
//! WebKit keep the decoded image in its memory cache across remounts
//! (`Cache-Control: immutable`), keeps IPC payloads tiny (a URL per sender
//! instead of ~10 KB of base64), and the app CSP only has to allow this one
//! scheme in `img-src`, not remote hosts.
//!
//! The only accepted path is `/<64 lowercase hex>.png`; the name is checked
//! character by character before it is joined to the cache dir, so no
//! request can name any other file.

use std::borrow::Cow;

use tauri::http::{header, Request, Response, StatusCode};

use super::cache::{valid_image_hash, Cache};

pub const SCHEME: &str = "avatar";

/// The image hash named by a request path, if the path is exactly
/// `/<hash>.png`.
pub fn parse_path(path: &str) -> Option<&str> {
    let name = path.strip_prefix('/')?;
    let hash = name.strip_suffix(".png")?;
    valid_image_hash(hash).then_some(hash)
}

pub fn not_found() -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Cow::Borrowed(&[][..]))
        .expect("static response")
}

pub fn respond(cache: &Cache, request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    if request.method() != tauri::http::Method::GET {
        return not_found();
    }
    let Some(path) = parse_path(request.uri().path()).and_then(|h| cache.image_path(h)) else {
        return not_found();
    };
    match std::fs::read(&path) {
        Ok(bytes) if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/png")
            .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .body(Cow::Owned(bytes))
            .unwrap_or_else(|_| not_found()),
        _ => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn only_bare_hash_paths_parse() {
        assert_eq!(parse_path(&format!("/{H}.png")), Some(H));
        for bad in [
            format!("{H}.png"),
            format!("/{H}"),
            format!("/{H}.png/"),
            format!("/img/{H}.png"),
            format!("/../{H}.png"),
            format!("/{H}.png/../index.json"),
            format!("//{H}.png"),
            format!("/{}.png", H.to_uppercase()),
            format!("/{}.png", &H[..63]),
            "/../index.json".into(),
            "/..%2Findex.json".into(),
            "/%2e%2e/%2e%2e/etc/passwd".into(),
            "/index.json".into(),
            "/contacts/abc.json".into(),
            "/".into(),
            String::new(),
            format!("/{H}.png%00.json"),
            format!("/{H}.PNG"),
        ] {
            assert_eq!(parse_path(&bad), None, "{bad}");
        }
    }

    #[test]
    fn serves_cached_png_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("penguin-avatar-proto-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::new(dir.clone());
        let png = b"\x89PNG\r\n\x1a\nrest";
        let hash = cache.put_image("icon-x", png, 0, 100, None).unwrap();
        cache.flush().unwrap();
        let get = |uri: &str| {
            let req = Request::builder().uri(uri).body(Vec::new()).unwrap();
            respond(&cache, &req)
        };
        let ok = get(&format!("avatar://localhost/{hash}.png"));
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(ok.headers()[header::CONTENT_TYPE], "image/png");
        assert!(ok.headers()[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable"));
        assert_eq!(ok.body().as_ref(), png);
        // The index sits next to img/ and must not be reachable.
        for uri in [
            "avatar://localhost/../index.json".to_string(),
            "avatar://localhost/index.json".to_string(),
            format!("avatar://localhost/img/{hash}.png"),
            format!("avatar://localhost/{}.png", "0".repeat(64)),
        ] {
            assert_eq!(get(&uri).status(), StatusCode::NOT_FOUND, "{uri}");
        }
        // Non-PNG content under a valid name (tampered cache) isn't served.
        let fake = "f".repeat(64);
        std::fs::write(
            dir.join("img").join(format!("{fake}.png")),
            b"<svg onload=x>",
        )
        .unwrap();
        assert_eq!(
            get(&format!("avatar://localhost/{fake}.png")).status(),
            StatusCode::NOT_FOUND
        );
        let post = Request::builder()
            .method("POST")
            .uri(format!("avatar://localhost/{hash}.png"))
            .body(Vec::new())
            .unwrap();
        assert_eq!(respond(&cache, &post).status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(dir);
    }
}
