//! Every fetched avatar ends up as a small PNG we encoded ourselves, so the
//! webview only ever decodes our output, never the sender's bytes.
//!
//! - Raster images (PNG, JPEG, GIF, WebP, ICO, BMP) are sniffed by content,
//!   decoded under strict size limits, resized to at most [`SIZE`] px and
//!   re-encoded.
//! - SVG (BIMI logos) is never passed on as SVG. It is rasterized with resvg
//!   after a preflight that rejects compressed, oversized or DTD/entity
//!   documents. usvg does not run scripts, render `foreignObject`, or fetch
//!   anything; on top of that it is built without text/font support, and its
//!   `<image>` resolver is replaced by one that refuses everything (the
//!   default one reads local files named by `href`).

use std::io::Cursor;

use image::{imageops::FilterType, DynamicImage, ImageFormat, ImageReader, Limits};

/// Stored edge length. The largest avatar in the UI is 56 CSS px; 128 covers
/// it at 2x.
pub const SIZE: u32 = 128;
/// Raster inputs above this are refused before decoding.
pub const MAX_RASTER_BYTES: usize = 2 * 1024 * 1024;
/// BIMI's SVG Tiny PS profile caps logos at 32 KB; allow some slack.
pub const MAX_SVG_BYTES: usize = 64 * 1024;
const MAX_DIMENSION: u32 = 4096;
/// Favicons smaller than this look like mush at avatar size.
pub const MIN_ICON_EDGE: u32 = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum ImageError {
    TooLarge,
    TooSmall,
    Unsupported,
    Invalid(String),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::TooLarge => f.write_str("image too large"),
            ImageError::TooSmall => f.write_str("image too small"),
            ImageError::Unsupported => f.write_str("unsupported image type"),
            ImageError::Invalid(e) => write!(f, "invalid image: {e}"),
        }
    }
}

/// Whether the bytes look like SVG (after an optional BOM and whitespace).
pub fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    t.starts_with("<svg") || (t.starts_with("<?xml") && text.contains("<svg"))
}

/// Decode any supported raster (or SVG) and return our PNG.
pub fn normalize(bytes: &[u8], min_edge: u32) -> Result<Vec<u8>, ImageError> {
    if looks_like_svg(bytes) {
        return rasterize_svg(bytes);
    }
    normalize_raster(bytes, min_edge)
}

pub fn normalize_raster(bytes: &[u8], min_edge: u32) -> Result<Vec<u8>, ImageError> {
    if bytes.len() > MAX_RASTER_BYTES {
        return Err(ImageError::TooLarge);
    }
    let format = image::guess_format(bytes).map_err(|_| ImageError::Unsupported)?;
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::WebP
            | ImageFormat::Ico
            | ImageFormat::Bmp
    ) {
        return Err(ImageError::Unsupported);
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| ImageError::Invalid(e.to_string()))?;
    if img.width().min(img.height()) < min_edge {
        return Err(ImageError::TooSmall);
    }
    encode(square(img))
}

/// Center-crop to a square (photos) and shrink to SIZE.
fn square(img: DynamicImage) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let edge = w.min(h);
    let img = if w != h {
        img.crop_imm((w - edge) / 2, (h - edge) / 2, edge, edge)
    } else {
        img
    };
    if edge > SIZE {
        img.resize_exact(SIZE, SIZE, FilterType::Lanczos3)
    } else {
        img
    }
}

fn encode(img: DynamicImage) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    img.to_rgba8()
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| ImageError::Invalid(e.to_string()))?;
    Ok(out)
}

/// Reject SVG we won't even hand to the parser: compressed (svgz bombs),
/// oversized, not UTF-8, or carrying a DTD (entity expansion).
fn svg_preflight(bytes: &[u8]) -> Result<&str, ImageError> {
    if bytes.len() > MAX_SVG_BYTES {
        return Err(ImageError::TooLarge);
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(ImageError::Unsupported);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ImageError::Invalid("not UTF-8".into()))?;
    let lower = text.to_ascii_lowercase();
    if lower.contains("<!doctype") || lower.contains("<!entity") {
        return Err(ImageError::Invalid("DTDs are not allowed".into()));
    }
    Ok(text)
}

pub fn rasterize_svg(bytes: &[u8]) -> Result<Vec<u8>, ImageError> {
    let text = svg_preflight(bytes)?;
    let opts = resvg::usvg::Options {
        resources_dir: None,
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree =
        resvg::usvg::Tree::from_str(text, &opts).map_err(|e| ImageError::Invalid(e.to_string()))?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    if !(w > 0.0 && h > 0.0) {
        return Err(ImageError::Invalid("empty SVG".into()));
    }
    // Fit inside SIZE×SIZE, centered: BIMI logos are square, others letterbox.
    let scale = (SIZE as f32 / w).min(SIZE as f32 / h);
    let dx = (SIZE as f32 - w * scale) / 2.0;
    let dy = (SIZE as f32 - h * scale) / 2.0;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(SIZE, SIZE)
        .ok_or_else(|| ImageError::Invalid("pixmap".into()))?;
    let transform = resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    if pixmap.pixels().iter().all(|p| p.alpha() == 0) {
        return Err(ImageError::Invalid("SVG rendered nothing".into()));
    }
    pixmap
        .encode_png()
        .map_err(|e| ImageError::Invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    fn decode(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory_with_format(png, ImageFormat::Png)
            .unwrap()
            .to_rgba8()
    }

    fn has_color(img: &image::RgbaImage, want: [u8; 3]) -> bool {
        img.pixels().any(|p| {
            p[3] > 0
                && (p[0] as i16 - want[0] as i16).abs() < 40
                && (p[1] as i16 - want[1] as i16).abs() < 40
                && (p[2] as i16 - want[2] as i16).abs() < 40
        })
    }

    #[test]
    fn raster_is_squared_resized_and_reencoded() {
        let out = normalize(&png(300, 200, [10, 200, 30, 255]), MIN_ICON_EDGE).unwrap();
        let img = decode(&out);
        assert_eq!((img.width(), img.height()), (SIZE, SIZE));
        assert!(out.starts_with(b"\x89PNG"));
    }

    #[test]
    fn small_icons_are_refused_and_junk_is_unsupported() {
        assert_eq!(
            normalize(&png(16, 16, [0, 0, 0, 255]), MIN_ICON_EDGE),
            Err(ImageError::TooSmall)
        );
        assert_eq!(
            normalize(b"<html><body>not an icon</body></html>", 0),
            Err(ImageError::Unsupported)
        );
        assert_eq!(
            normalize(&vec![0u8; MAX_RASTER_BYTES + 1], 0),
            Err(ImageError::TooLarge)
        );
    }

    #[test]
    fn huge_declared_dimensions_are_refused() {
        // A tiny PNG header claiming 60000×60000: decoding must stop at the limit.
        let mut bytes = png(1, 1, [0, 0, 0, 255]);
        bytes[16..20].copy_from_slice(&60000u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&60000u32.to_be_bytes());
        assert!(normalize(&bytes, 0).is_err());
    }

    const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" version="1.2" baseProfile="tiny-ps" viewBox="0 0 100 100"><title>Shop</title><rect width="100" height="100" fill="#1e40af"/><circle cx="50" cy="50" r="30" fill="#ffffff"/></svg>"##;

    #[test]
    fn plain_bimi_logo_rasterizes() {
        let out = rasterize_svg(LOGO.as_bytes()).unwrap();
        let img = decode(&out);
        assert_eq!((img.width(), img.height()), (SIZE, SIZE));
        assert!(has_color(&img, [0x1e, 0x40, 0xaf]));
    }

    /// Hostile SVGs: whatever happens, the output is our own PNG (or an
    /// error), nothing is read from disk, and nothing hangs.
    #[test]
    fn malicious_svg_corpus() {
        // A bright red PNG on disk that an <image href> would pull in.
        let dir = std::env::temp_dir().join(format!("penguin-avatar-svg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let red = dir.join("secret.png");
        std::fs::write(&red, png(64, 64, [255, 0, 0, 255])).unwrap();
        let red_path = red.display().to_string();
        let red_data = format!(
            "data:image/png;base64,{}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                png(64, 64, [255, 0, 0, 255])
            )
        );
        let green_bg = r##"<rect width="100" height="100" fill="#00a000"/>"##;
        let wrap = |inner: &str| {
            format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 100 100">{green_bg}{inner}</svg>"##
            )
        };
        let corpus: Vec<String> = vec![
            wrap(r#"<script>alert(1)</script>"#),
            wrap(r#"<script xlink:href="https://evil.example/x.js"/>"#),
            wrap(
                r#"<foreignObject width="100" height="100"><iframe xmlns="http://www.w3.org/1999/xhtml" src="https://evil.example"/></foreignObject>"#,
            ),
            wrap(
                r#"<a href="javascript:alert(1)"><circle r="10" onload="alert(1)" onclick="alert(2)"/></a>"#,
            ),
            wrap(
                r#"<animate attributeName="href" to="javascript:alert(1)"/><set attributeName="onload" to="alert(1)"/>"#,
            ),
            wrap(&format!(
                r#"<image href="{red_path}" width="100" height="100"/>"#
            )),
            wrap(&format!(
                r#"<image xlink:href="file://{red_path}" width="100" height="100"/>"#
            )),
            wrap(r#"<image href="secret.png" width="100" height="100"/>"#),
            wrap(&format!(
                r#"<image href="{red_data}" width="100" height="100"/>"#
            )),
            wrap(r#"<image href="https://evil.example/pixel.png" width="100" height="100"/>"#),
            wrap(r#"<use href="https://evil.example/sprite.svg#a"/><use href="/etc/passwd#x"/>"#),
            wrap(
                r#"<style>@import url(https://evil.example/x.css); circle { fill: url(https://evil.example/p) }</style><circle r="5"/>"#,
            ),
            wrap(r#"<text x="10" y="50">Hello</text>"#),
            wrap(r##"<g id="a"><use href="#a"/></g><use href="#a"/>"##),
            wrap(
                r#"<filter id="f"><feImage href="/etc/hosts"/><feGaussianBlur stdDeviation="1000000"/></filter><rect width="100" height="100" filter="url(#f)"/>"#,
            ),
            wrap(
                r#"<pattern id="p" width="0.0001" height="0.0001"><rect width="1" height="1"/></pattern><rect width="100" height="100" fill="url(#p)"/>"#,
            ),
            format!(
                r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">]>{}"#,
                wrap("<text>&c;&c;&c;</text>")
            ),
            format!(
                r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY x SYSTEM "file:///etc/passwd">]>{}"#,
                wrap("<text>&x;</text>")
            ),
        ];
        let mut rendered = 0;
        for (i, svg) in corpus.iter().enumerate() {
            // Refusing is always fine; whatever renders must be clean.
            if let Ok(out) = rasterize_svg(svg.as_bytes()) {
                rendered += 1;
                assert!(out.starts_with(b"\x89PNG"), "case {i}");
                let img = decode(&out);
                assert_eq!((img.width(), img.height()), (SIZE, SIZE), "case {i}");
                assert!(
                    has_color(&img, [0, 0xa0, 0]),
                    "case {i} lost the benign part"
                );
                assert!(
                    !has_color(&img, [255, 0, 0]),
                    "case {i} pulled in an external image"
                );
            }
        }
        // The corpus exercises the renderer, not just the preflight.
        assert!(rendered >= 12, "only {rendered} cases rendered");
        // DTD documents never reach the parser.
        assert!(rasterize_svg(corpus[16].as_bytes()).is_err());
        assert!(rasterize_svg(corpus[17].as_bytes()).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn svgz_oversized_and_non_utf8_svg_are_refused() {
        let mut gz = vec![0x1f, 0x8b, 0x08];
        gz.extend_from_slice(&[0; 32]);
        assert_eq!(rasterize_svg(&gz), Err(ImageError::Unsupported));
        let big = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg">{}</svg>"#,
            "<!-- pad -->".repeat(MAX_SVG_BYTES / 10)
        );
        assert_eq!(rasterize_svg(big.as_bytes()), Err(ImageError::TooLarge));
        assert!(rasterize_svg(b"<svg \xff\xfe>").is_err());
    }

    #[test]
    fn svg_is_detected_before_raster_sniffing() {
        assert!(looks_like_svg(LOGO.as_bytes()));
        assert!(looks_like_svg(
            format!("\u{feff}<?xml version=\"1.0\"?>\n{LOGO}").as_bytes()
        ));
        assert!(!looks_like_svg(&png(2, 2, [0, 0, 0, 255])));
        assert!(normalize(LOGO.as_bytes(), MIN_ICON_EDGE).is_ok());
    }
}
