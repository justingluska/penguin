//! Pictures from a message, for the image viewer's Save / Copy / Share:
//! what the bytes really are (never the declared type), and a remote
//! picture fetched with the avatar fetcher's hardening (net.rs: HTTPS only,
//! public addresses only, no cookies or Referer, a size cap). Used by the
//! Mac's image viewer and attachment previews and by the iPhone's viewer.

use crate::net::{acceptable_url, FetchError, Net};

/// The largest picture Save / Copy handles (the attachment preview's limit).
pub const MAX_PICTURE_BYTES: usize = 20 * 1024 * 1024;

/// PNG, JPEG, GIF or WebP by their magic bytes (the types an attachment
/// preview draws).
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// [`sniff_image`] plus the other types a rendered body embeds (BMP, ICO,
/// AVIF).
pub fn sniff_picture(bytes: &[u8]) -> Option<&'static str> {
    sniff_image(bytes).or_else(|| {
        if bytes.starts_with(b"BM") && bytes.len() > 14 {
            Some("image/bmp")
        } else if bytes.starts_with(&[0, 0, 1, 0]) && bytes.len() > 6 {
            Some("image/x-icon")
        } else if bytes.len() >= 12
            && &bytes[4..8] == b"ftyp"
            && matches!(&bytes[8..12], b"avif" | b"avis")
        {
            Some("image/avif")
        } else {
            None
        }
    })
}

/// Why a remote picture couldn't be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PictureError {
    /// Not an https URL to a public host name.
    NotAllowed,
    /// Offline, timeout, 5xx: `host` and why.
    Unreachable {
        host: String,
        error: String,
    },
    /// The host answered without the picture.
    Unavailable {
        host: String,
    },
    TooLarge,
    /// A data: URL whose base64 doesn't decode.
    Damaged,
    /// The bytes aren't a picture.
    NotAPicture,
}

impl std::fmt::Display for PictureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PictureError::NotAllowed => f.write_str("Not a picture Penguin can save"),
            PictureError::Unreachable { host, .. } => write!(f, "Couldn't reach {host}"),
            PictureError::Unavailable { host } => write!(f, "{host} didn't return the picture"),
            PictureError::TooLarge => f.write_str("That picture is too large to save"),
            PictureError::Damaged => f.write_str("That picture's data is damaged"),
            PictureError::NotAPicture => f.write_str("That isn't a picture Penguin can save"),
        }
    }
}

impl std::error::Error for PictureError {}

/// Bytes already in hand (a data: picture, a download): within the size
/// cap and really a picture. Returns the sniffed type.
pub fn check_picture(bytes: &[u8]) -> Result<&'static str, PictureError> {
    if bytes.len() > MAX_PICTURE_BYTES {
        return Err(PictureError::TooLarge);
    }
    sniff_picture(bytes).ok_or(PictureError::NotAPicture)
}

/// The image types penguin-render lets through as `data:` URLs.
const DATA_PREFIXES: &[&str] = &[
    "data:image/png;base64,",
    "data:image/jpeg;base64,",
    "data:image/gif;base64,",
    "data:image/webp;base64,",
    "data:image/bmp;base64,",
    "data:image/avif;base64,",
    "data:image/x-icon;base64,",
];

/// A body picture embedded as a `data:` URL (an inline `cid:` part the
/// renderer embedded), decoded but not yet sniffed. None when `url` isn't
/// one of the data: image types the renderer produces.
pub fn data_picture(url: &str) -> Option<Result<Vec<u8>, PictureError>> {
    use base64::Engine;
    let url = url.trim();
    let prefix = DATA_PREFIXES
        .iter()
        .find(|p| url.len() >= p.len() && url[..p.len()].eq_ignore_ascii_case(p))?;
    let b64 = &url[prefix.len()..];
    if b64.len() > MAX_PICTURE_BYTES / 3 * 4 + 4 {
        return Some(Err(PictureError::TooLarge));
    }
    Some(
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| PictureError::Damaged),
    )
}

/// A picture shown in a message body, by its `src`: an embedded data: URL
/// (decoded) or a remote https one the user let load (fetched again with
/// [`fetch_remote_picture`]). Always sniffed.
pub async fn message_picture(
    net: &dyn Net,
    url: &str,
) -> Result<(Vec<u8>, &'static str), PictureError> {
    match data_picture(url) {
        Some(bytes) => {
            let bytes = bytes?;
            let mime = check_picture(&bytes)?;
            Ok((bytes, mime))
        }
        None => fetch_remote_picture(net, url).await,
    }
}

/// The picture at `url` (https, a public host) and its sniffed type.
pub async fn fetch_remote_picture(
    net: &dyn Net,
    url: &str,
) -> Result<(Vec<u8>, &'static str), PictureError> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| PictureError::NotAllowed)?;
    if parsed.scheme() != "https" || !acceptable_url(&parsed) {
        return Err(PictureError::NotAllowed);
    }
    let host = parsed.host_str().unwrap_or("").to_string();
    let bytes = match net.get(parsed.as_str(), MAX_PICTURE_BYTES, None).await {
        Ok(bytes) => bytes,
        Err(FetchError::Transient(error)) => {
            tracing::warn!(host = %host, error = %error, "message picture fetch failed");
            return Err(PictureError::Unreachable { host, error });
        }
        Err(_) => {
            tracing::warn!(host = %host, "message picture unavailable");
            return Err(PictureError::Unavailable { host });
        }
    };
    let mime = check_picture(&bytes)?;
    Ok((bytes, mime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::BoxFut;

    struct OneFile(Result<Vec<u8>, FetchError>);

    impl Net for OneFile {
        fn txt<'a>(&'a self, _name: &'a str) -> BoxFut<'a, Result<Vec<String>, String>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn get<'a>(
            &'a self,
            _url: &'a str,
            _max: usize,
            _bearer: Option<&'a str>,
        ) -> BoxFut<'a, Result<Vec<u8>, FetchError>> {
            let r = self.0.clone();
            Box::pin(async move { r })
        }
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0000000000";

    #[tokio::test]
    async fn only_public_https_pictures_are_fetched_and_they_must_sniff() {
        let ok = OneFile(Ok(PNG.to_vec()));
        assert_eq!(
            fetch_remote_picture(&ok, "https://cdn.shop.example/a.png")
                .await
                .unwrap()
                .1,
            "image/png"
        );
        for bad in [
            "http://cdn.shop.example/a.png",
            "https://127.0.0.1/a.png",
            "https://localhost/a.png",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert_eq!(
                fetch_remote_picture(&ok, bad).await.unwrap_err(),
                PictureError::NotAllowed,
                "{bad}"
            );
        }
        let html = OneFile(Ok(b"<html>".to_vec()));
        assert_eq!(
            fetch_remote_picture(&html, "https://cdn.shop.example/a.png")
                .await
                .unwrap_err(),
            PictureError::NotAPicture
        );
        let offline = OneFile(Err(FetchError::Transient("offline".into())));
        assert!(matches!(
            fetch_remote_picture(&offline, "https://cdn.shop.example/a.png").await,
            Err(PictureError::Unreachable { .. })
        ));
        let gone = OneFile(Err(FetchError::NotFound));
        assert_eq!(
            fetch_remote_picture(&gone, "https://cdn.shop.example/a.png")
                .await
                .unwrap_err()
                .to_string(),
            "cdn.shop.example didn't return the picture"
        );
    }

    #[tokio::test]
    async fn data_pictures_decode_and_must_sniff() {
        use base64::Engine;
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let none = OneFile(Err(FetchError::NotFound));
        let url = format!("data:image/png;base64,{}", b64(PNG));
        assert_eq!(
            message_picture(&none, &url).await.unwrap(),
            (PNG.to_vec(), "image/png")
        );
        let html = format!("data:image/png;base64,{}", b64(b"<script>x</script>"));
        assert_eq!(
            message_picture(&none, &html).await.unwrap_err(),
            PictureError::NotAPicture
        );
        assert_eq!(
            data_picture("data:image/png;base64,***"),
            Some(Err(PictureError::Damaged))
        );
        for not_ours in [
            "data:image/svg+xml;base64,PHN2Zz4=",
            "data:text/html;base64,PHNjcmlwdD4=",
            "data:image/png,rawbytes",
        ] {
            assert_eq!(data_picture(not_ours), None, "{not_ours}");
            assert_eq!(
                message_picture(&none, not_ours).await.unwrap_err(),
                PictureError::NotAllowed
            );
        }
    }

    #[test]
    fn pictures_are_sniffed_not_trusted() {
        assert_eq!(sniff_picture(PNG), Some("image/png"));
        assert_eq!(sniff_picture(b"GIF89a....."), Some("image/gif"));
        assert_eq!(sniff_picture(b"BM0000000000000000"), Some("image/bmp"));
        assert_eq!(sniff_image(b"BM0000000000000000"), None);
        assert_eq!(sniff_picture(b"%PDF-1.7"), None);
        assert_eq!(
            check_picture(&vec![0u8; MAX_PICTURE_BYTES + 1]),
            Err(PictureError::TooLarge)
        );
    }
}
