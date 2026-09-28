//! penguin-render: turn untrusted email HTML into something safe to show.
//! OWNER: security/rendering agent.
//!
//! Output is rendered by the UI inside a sandboxed iframe (no allow-scripts)
//! with a deny-by-default CSP, so this sanitizer is one layer of defense, not
//! the only one. See docs/SECURITY.md for the whole pipeline.
//!
//! Pipeline for HTML bodies:
//! 1. `prescan` tokenizes the raw HTML once to collect `<style>` text, the
//!    `<body>` tag's colors, and which image URLs are tiny or hidden.
//! 2. ammonia parses the HTML with a real HTML5 tree builder and keeps only
//!    allowlisted tags and attributes. Its attribute callback sanitizes
//!    inline CSS, resolves every image URL (cid: → data:, tracker and remote
//!    blocking), and restricts link schemes.
//! 3. The result is wrapped in a complete document with a strict CSP meta,
//!    our base stylesheet and the sanitized email stylesheet.

mod compose;
mod css;
pub mod links;
mod prescan;
mod text;
pub mod trackers;
pub mod unsubscribe;

pub use compose::{
    cid_map, is_safe_cid, keep_cids, referenced_cids, sanitize_compose_html,
    sanitize_compose_html_with_images, sanitize_outgoing_html, sanitize_outgoing_html_with_images,
    sanitize_quoted_html, sanitize_quoted_html_with_images, CidMap,
};

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use ammonia::{Builder, Url, UrlRelative};
use serde::Serialize;

#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    /// Remote http(s) images are stripped (replaced by placeholders) unless true.
    pub allow_remote_images: bool,
    /// cid → data: URL for inline images already fetched.
    pub cid_map: HashMap<String, String>,
    /// Tracking pixels (known trackers, tiny or hidden images) are treated
    /// like any other remote image instead of always being removed: held
    /// back while remote images are, loaded when they load. False by
    /// default (Settings → Privacy "Block tracking pixels" on). They are
    /// still found and reported, with `TrackerStatus::Held` or `Loaded`.
    pub allow_tracking_pixels: bool,
    /// Remove tracking parameters (`links::strip_tracking_params`) from
    /// every link. False by default.
    pub strip_link_tracking: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedHtml {
    /// Full HTML document (with our base styles + CSP meta) ready for iframe srcdoc.
    pub html: String,
    pub blocked_remote_images: u32,
    /// Known tracking pixels / tracker hosts removed (even when images allowed).
    /// Counts distinct URLs; the sum of `trackers[].count` unless the list
    /// was capped.
    pub trackers_removed: u32,
    /// Every tracker found, one entry per host + path + kind + status, in
    /// document order. At most `MAX_TRACKER_ENTRIES`.
    pub trackers: Vec<TrackerRemoved>,
    /// Trackers found but not removed because `allow_tracking_pixels` was
    /// set (distinct URLs): held back with the other remote images, or
    /// loaded with them.
    pub trackers_allowed: u32,
    /// Links whose tracking parameters were removed (distinct URLs).
    pub links_cleaned: u32,
    /// The names of the parameters removed from links (never their values),
    /// lowercased, in first-seen order. At most `MAX_LINK_PARAMS`.
    pub link_params: Vec<String>,
    /// Links that go through a click-tracking redirect (distinct URLs);
    /// opening one tells the sender, whatever Penguin does.
    pub tracked_links: u32,
    /// Cleaned link → the link as the sender wrote it, for callers that
    /// must act on the original (the Unsubscribe button). Not serialized.
    #[serde(skip)]
    pub original_links: HashMap<String, String>,
}

impl RenderedHtml {
    /// A document with nothing blocked or found (plain text).
    fn text_only(html: String) -> Self {
        RenderedHtml {
            html,
            blocked_remote_images: 0,
            trackers_removed: 0,
            trackers: Vec::new(),
            trackers_allowed: 0,
            links_cleaned: 0,
            link_params: Vec::new(),
            tracked_links: 0,
            original_links: HashMap::new(),
        }
    }

    /// The link as the sender wrote it, for a link found in `html` (which
    /// may have had its tracking parameters removed).
    pub fn original_link(&self, link: &str) -> String {
        self.original_links
            .get(link)
            .cloned()
            .unwrap_or_else(|| link.to_string())
    }
}

/// Distinct link parameter names reported per message.
pub const MAX_LINK_PARAMS: usize = 30;

/// Entries past this many are only counted (a hostile message can carry
/// thousands of pixels).
pub const MAX_TRACKER_ENTRIES: usize = 50;
/// Paths longer than this are cut (with "…"): they're for recognizing the
/// tracker, and long ones are mostly per-recipient ids.
const MAX_TRACKER_PATH: usize = 80;

/// Why an image was treated as a tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackerKind {
    /// Matched the known-tracker list (`trackers::match_tracker`).
    KnownTracker,
    /// A tiny (3×3 px or smaller) or hidden `<img>` (`prescan`): an open-tracking
    /// pixel from a host that isn't on the list.
    HiddenImage,
}

/// What happened to a tracker found in a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackerStatus {
    /// Removed (tracking pixel blocking on, the default).
    Removed,
    /// Blocking is off; held back with the message's other remote images.
    Held,
    /// Blocking is off and remote images were allowed: it loaded.
    Loaded,
}

/// One tracker found in a message, for the privacy details in the UI.
/// Never carries the query string or fragment: those usually hold the
/// recipient's address or a per-recipient id. (The name predates `status`:
/// with blocking on, every tracker found is removed.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerRemoved {
    /// Lowercased host (`pixel.mailerlite.example`).
    pub host: String,
    /// URL path without query or fragment, capped at 80 characters.
    pub path: String,
    pub kind: TrackerKind,
    /// The company behind a known tracker, when the list names one.
    pub company: Option<String>,
    /// The list entry that matched (`host awstrack.me`, `path /wf/open`), or
    /// `hidden image` for `HiddenImage`.
    pub rule: String,
    /// At least one of the removed URLs had a query string.
    pub had_query: bool,
    /// Distinct URLs folded into this entry (same host, path and kind).
    pub count: u32,
    pub status: TrackerStatus,
}

/// Every document starts with one of these, so the UI can tell the two
/// kinds apart (plain text follows the app theme; HTML mail is shown on a
/// light canvas unless the UI's "Dark email bodies" darkens it) without
/// parsing.
pub const HTML_DOC_PREFIX: &str = "<!DOCTYPE html><html class=\"pg-html\">";
pub const TEXT_DOC_PREFIX: &str = "<!DOCTYPE html><html class=\"pg-text\">";

const TAGS: &[&str] = &[
    "a",
    "abbr",
    "acronym",
    "address",
    "article",
    "aside",
    "b",
    "bdi",
    "bdo",
    "big",
    "blockquote",
    "br",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "font",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "main",
    "mark",
    "nav",
    "ol",
    "p",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "section",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "wbr",
];

/// Removed together with everything inside them. Anything else not in
/// `TAGS` (form, button, font-face, o:p, …) is unwrapped: the tag goes,
/// its children stay.
const CLEAN_CONTENT_TAGS: &[&str] = &[
    "script",
    "style",
    "title",
    "noscript",
    "template",
    "iframe",
    "frame",
    "frameset",
    "object",
    "embed",
    "applet",
    "noembed",
    "noframes",
    "xmp",
    "plaintext",
    "textarea",
    "select",
    "option",
    "svg",
    "math",
    "head",
    "meta",
    "link",
    "base",
    "audio",
    "video",
    "canvas",
    "map",
    "portal",
    "fencedframe",
    "dialog",
];

const GENERIC_ATTRIBUTES: &[&str] = &[
    "align", "bgcolor", "border", "class", "dir", "height", "id", "lang", "style", "title",
    "valign", "width",
];

const TAG_ATTRIBUTES: &[(&str, &[&str])] = &[
    ("a", &["href", "name"]),
    ("img", &["src", "srcset", "alt", "hspace", "vspace"]),
    (
        "table",
        &[
            "cellpadding",
            "cellspacing",
            "background",
            "summary",
            "frame",
            "rules",
        ],
    ),
    (
        "td",
        &[
            "colspan",
            "rowspan",
            "nowrap",
            "background",
            "headers",
            "scope",
            "abbr",
        ],
    ),
    (
        "th",
        &[
            "colspan",
            "rowspan",
            "nowrap",
            "background",
            "headers",
            "scope",
            "abbr",
        ],
    ),
    ("tr", &["background"]),
    ("tbody", &["background"]),
    ("col", &["span"]),
    ("colgroup", &["span"]),
    ("font", &["color", "face", "size"]),
    ("ol", &["start", "type", "reversed"]),
    ("ul", &["type"]),
    ("li", &["value", "type"]),
    ("hr", &["size", "noshade", "color"]),
    ("time", &["datetime"]),
    ("details", &["open"]),
    ("bdo", &["dir"]),
];

/// Schemes that survive ammonia's URL check. The attribute callback then
/// narrows them per attribute (links: http/https/mailto; images:
/// https/http/cid/data:image).
const URL_SCHEMES: &[&str] = &["http", "https", "mailto", "cid", "data"];

const DATA_IMAGE_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/jpg",
    "image/gif",
    "image/webp",
    "image/bmp",
    "image/avif",
    "image/x-icon",
    "image/vnd.microsoft.icon",
];

/// A `data:image/…;base64,…` URL whose payload is plain base64, so it can be
/// placed in an attribute or a quoted CSS `url()` as-is.
fn safe_data_image(v: &str) -> Option<String> {
    let v = v.trim();
    let (head, payload) = v.split_once(',')?;
    let head = head.to_ascii_lowercase();
    let mime = head.strip_prefix("data:")?.strip_suffix(";base64")?;
    if !DATA_IMAGE_TYPES.contains(&mime) {
        return None;
    }
    let payload: String = payload
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    if payload.is_empty()
        || !payload
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    {
        return None;
    }
    Some(format!("data:{mime};base64,{payload}"))
}

/// Normalize a Content-ID for lookup: strip `<>`, percent-decode (RFC 2392
/// cid: URLs are percent-encoded), lowercase.
pub fn normalize_cid(cid: &str) -> String {
    let cid = cid.trim().trim_start_matches('<').trim_end_matches('>');
    let bytes = cid.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(b) = cid
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_lowercase()
}

/// Characters that would end a quoted CSS `url("…")` or be ambiguous in it
/// are percent-encoded; everything else in a parsed URL is already safe.
fn css_safe_url(u: &str) -> String {
    let mut out = String::with_capacity(u.len());
    for c in u.chars() {
        match c {
            '"' => out.push_str("%22"),
            '\'' => out.push_str("%27"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '\\' => out.push_str("%5C"),
            c if c.is_whitespace() || c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[derive(Default)]
struct Counts {
    blocked: HashSet<String>,
    trackers: HashSet<String>,
    trackers_removed: u32,
    trackers_allowed: u32,
    entries: Vec<TrackerRemoved>,
    cleaned: HashSet<String>,
    link_params: Vec<String>,
    tracked_links: HashSet<String>,
    original_links: HashMap<String, String>,
}

impl Counts {
    /// Count a tracker URL once, folding it into its host + path entry.
    fn tracker(&mut self, url: &Url, hit: Option<trackers::TrackerMatch>, status: TrackerStatus) {
        if !self.trackers.insert(url.as_str().to_string()) {
            return;
        }
        if status == TrackerStatus::Removed {
            self.trackers_removed += 1;
        } else {
            self.trackers_allowed += 1;
        }
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let path = cap_path(url.path());
        let kind = if hit.is_some() {
            TrackerKind::KnownTracker
        } else {
            TrackerKind::HiddenImage
        };
        let had_query = url.query().is_some_and(|q| !q.is_empty());
        if let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.host == host && e.path == path && e.kind == kind && e.status == status)
        {
            e.count += 1;
            e.had_query |= had_query;
            return;
        }
        if self.entries.len() >= MAX_TRACKER_ENTRIES {
            return;
        }
        let (company, rule) = match hit {
            Some(m) => (m.company.map(str::to_string), m.rule),
            None => (None, "hidden image".to_string()),
        };
        self.entries.push(TrackerRemoved {
            host,
            path,
            kind,
            company,
            rule,
            had_query,
            count: 1,
            status,
        });
    }

    /// Record a cleaned link: its removed parameter names, and the original
    /// for `RenderedHtml::original_links`.
    fn cleaned_link(&mut self, original: &str, cleaned: &links::Cleaned) {
        if self.cleaned.insert(original.to_string()) {
            for p in &cleaned.removed {
                if self.link_params.len() < MAX_LINK_PARAMS && !self.link_params.contains(p) {
                    self.link_params.push(p.clone());
                }
            }
        }
        self.original_links
            .entry(cleaned.url.clone())
            .or_insert_with(|| original.to_string());
    }
}

fn cap_path(path: &str) -> String {
    if path.chars().count() <= MAX_TRACKER_PATH {
        return path.to_string();
    }
    let mut out: String = path.chars().take(MAX_TRACKER_PATH - 1).collect();
    out.push('…');
    out
}

/// Decides the fate of every image URL in the message (img src/srcset,
/// `background` attributes, CSS `url()`).
struct ImagePolicy {
    allow_remote: bool,
    allow_trackers: bool,
    strip_links: bool,
    cid_map: HashMap<String, String>,
    tiny_srcs: HashSet<String>,
    counts: Mutex<Counts>,
}

impl ImagePolicy {
    fn new(opts: &RenderOptions, tiny_srcs: HashSet<String>) -> Self {
        ImagePolicy {
            allow_remote: opts.allow_remote_images,
            allow_trackers: opts.allow_tracking_pixels,
            strip_links: opts.strip_link_tracking,
            cid_map: opts
                .cid_map
                .iter()
                .map(|(k, v)| (normalize_cid(k), v.clone()))
                .collect(),
            tiny_srcs,
            counts: Mutex::new(Counts::default()),
        }
    }

    fn counts(&self) -> std::sync::MutexGuard<'_, Counts> {
        // A poisoned lock only means another thread panicked mid-insert;
        // the sets are still valid for counting.
        self.counts.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Returns the URL to emit, or `None` to drop it. `check_tiny` is true
    /// for `<img src>`, where the prescan's size hints apply.
    fn resolve(&self, raw: &str, check_tiny: bool) -> Option<String> {
        let raw = raw.trim();
        let lower = raw
            .get(..5)
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if lower.starts_with("data:") {
            return safe_data_image(raw);
        }
        if lower.starts_with("cid:") {
            let data = self.cid_map.get(&normalize_cid(&raw[4..]))?;
            return safe_data_image(data);
        }
        let url = Url::parse(raw).ok()?;
        if !matches!(url.scheme(), "http" | "https") || !is_public_host(&url) {
            return None;
        }
        let hit = trackers::match_tracker(&url);
        if hit.is_some() || (check_tiny && self.tiny_srcs.contains(raw)) {
            if !self.allow_trackers {
                self.counts().tracker(&url, hit, TrackerStatus::Removed);
                return None;
            }
            // Blocking is off: an ordinary remote image from here on.
            let status = if self.allow_remote {
                TrackerStatus::Loaded
            } else {
                TrackerStatus::Held
            };
            self.counts().tracker(&url, hit, status);
        }
        if !self.allow_remote {
            self.counts().blocked.insert(url.as_str().to_string());
            return None;
        }
        // Loaded images are upgraded to https: the frame's CSP only allows
        // https:, and plain http would leak the fetch on the network.
        let mut url = url;
        if url.scheme() == "http" {
            url.set_scheme("https").ok()?;
        }
        Some(css_safe_url(url.as_str()))
    }

    /// The `<a href>` to emit for one that passed `link_href_ok`:
    /// click-tracking redirects are counted, and with `strip_links` the
    /// tracking parameters are removed.
    fn link(&self, href: &str) -> String {
        let Ok(url) = Url::parse(href) else {
            return href.to_string();
        };
        if !matches!(url.scheme(), "http" | "https") {
            return href.to_string();
        }
        if links::is_tracked_link(&url) {
            self.counts().tracked_links.insert(url.as_str().to_string());
        }
        if !self.strip_links {
            return href.to_string();
        }
        match links::strip_tracking_params(&url) {
            Some(cleaned) => {
                self.counts().cleaned_link(href, &cleaned);
                cleaned.url
            }
            None => href.to_string(),
        }
    }

    /// `srcset`: keep only candidates that resolve, re-serialized.
    fn resolve_srcset(&self, value: &str) -> Option<String> {
        let mut kept = Vec::new();
        for candidate in value.split(',') {
            let mut parts = candidate.split_whitespace();
            let Some(url) = parts.next() else { continue };
            let descriptor = parts.next();
            if parts.next().is_some() {
                continue;
            }
            if url.to_ascii_lowercase().starts_with("data:") {
                // data: URLs contain commas, which the simple split breaks.
                continue;
            }
            let desc_ok = descriptor.is_none_or(|d| {
                d.len() > 1
                    && d[..d.len() - 1].parse::<f32>().is_ok()
                    && (d.ends_with('w') || d.ends_with('x'))
            });
            if !desc_ok {
                continue;
            }
            if let Some(u) = self.resolve(url, false) {
                kept.push(match descriptor {
                    Some(d) => format!("{u} {d}"),
                    None => u,
                });
            }
        }
        (!kept.is_empty()).then(|| kept.join(", "))
    }
}

/// Remote images may only come from the public internet. "Load images"
/// must not turn an email into requests to this machine or the LAN: the
/// app's own custom-protocol origins (`tauri.localhost`, `ipc.localhost`,
/// `avatar.localhost` on Windows), loopback and private services, or a
/// router's GET endpoints (CSRF). Hostnames can still resolve to private
/// addresses (DNS rebinding); the webview does that fetch, so this check
/// covers literal hosts only.
fn is_public_host(url: &Url) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr};
    fn v4_public(ip: Ipv4Addr) -> bool {
        let o = ip.octets();
        !(ip.is_private()
            || ip.is_loopback()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_broadcast()
            || ip.is_multicast()
            || ip.is_documentation()
            || o[0] == 0
            || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT
            || o[0] >= 240)
    }
    fn v6_public(ip: Ipv6Addr) -> bool {
        if let Some(v4) = ip.to_ipv4_mapped() {
            return v4_public(v4);
        }
        let seg0 = ip.segments()[0];
        !(ip.is_loopback()
            || ip.is_unspecified()
            || ip.is_multicast()
            || (seg0 & 0xfe00) == 0xfc00 // unique local
            || (seg0 & 0xffc0) == 0xfe80) // link local
    }
    match url.host() {
        Some(ammonia::url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            !(d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || d.ends_with(".internal")
                || d.ends_with(".home.arpa")
                || !d.contains('.'))
        }
        Some(ammonia::url::Host::Ipv4(ip)) => v4_public(ip),
        Some(ammonia::url::Host::Ipv6(ip)) => v6_public(ip),
        None => false,
    }
}

fn link_href_ok(v: &str) -> bool {
    Url::parse(v.trim()).is_ok_and(|u| match u.scheme() {
        "http" | "https" => u.host_str().is_some(),
        "mailto" => true,
        _ => false,
    })
}

fn escape_attr(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    text::escape(v, &mut out);
    out
}

/// A presentational color attribute (`bgcolor="#f4f4f4"`, `text="black"`).
fn simple_color(v: &str) -> Option<&str> {
    let v = v.trim();
    (!v.is_empty() && v.len() <= 32 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'#'))
        .then_some(v)
}

fn build_sanitizer(policy: Arc<ImagePolicy>) -> Builder<'static> {
    // Newsletters repeat the same inline styles on thousands of elements;
    // sanitizing each distinct one once was a fifth of a 2 MB render. The
    // result depends only on the value, and the policy only records URLs in
    // sets, so a repeat changes nothing but the time. One cache per render.
    let styles: Mutex<HashMap<String, Option<String>>> = Mutex::new(HashMap::new());
    let mut b = Builder::empty();
    b.tags(TAGS.iter().copied().collect())
        .clean_content_tags(CLEAN_CONTENT_TAGS.iter().copied().collect())
        .generic_attributes(GENERIC_ATTRIBUTES.iter().copied().collect())
        .tag_attributes(
            TAG_ATTRIBUTES
                .iter()
                .map(|(t, a)| (*t, a.iter().copied().collect()))
                .collect(),
        )
        .url_schemes(URL_SCHEMES.iter().copied().collect())
        .url_relative(UrlRelative::Deny)
        .link_rel(Some("noopener noreferrer"))
        .set_tag_attribute_value("a", "target", "_blank")
        .strip_comments(true)
        .attribute_filter(
            move |element, attribute, value| match (element, attribute) {
                ("a", "href") => {
                    let v = value.trim();
                    link_href_ok(v).then(|| Cow::Owned(policy.link(v)))
                }
                (_, "src") => policy.resolve(value, element == "img").map(Cow::Owned),
                (_, "srcset") => policy.resolve_srcset(value).map(Cow::Owned),
                (_, "background") => policy.resolve(value, false).map(Cow::Owned),
                (_, "style") => {
                    let mut styles = styles.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(done) = styles.get(value) {
                        return done.clone().map(Cow::Owned);
                    }
                    let css = css::sanitize_declarations(value, &|u| policy.resolve(u, false));
                    let css = (!css.is_empty()).then_some(css);
                    styles.insert(value.to_string(), css.clone());
                    css.map(Cow::Owned)
                }
                _ => Some(Cow::Borrowed(value)),
            },
        );
    b
}

fn csp(allow_remote_images: bool) -> String {
    format!(
        "default-src 'none'; img-src data:{}; style-src 'unsafe-inline'; font-src 'none'; \
         media-src 'none'; form-action 'none'; base-uri 'none'",
        if allow_remote_images { " https:" } else { "" }
    )
}

/// Base styles inside the frame. HTML mail renders on a light canvas by
/// default: most newsletters hardcode dark text and light backgrounds, and
/// inverting them (as some clients do) mangles brand colors and images.
/// The UI puts that canvas on a rounded card in dark mode.
///
/// The `data-pg-scheme=dark` rules are the dark canvas for a message that
/// supports one (see `DARK_SHEET_OPEN`); the UI sets the attribute when
/// "Dark email bodies" is on. `:where()` keeps their specificity at the
/// base rules' level, so the email's own styles still win.
const BASE_CSS: &str = "\
html{color-scheme:light;background:#fff;-webkit-text-size-adjust:100%;text-size-adjust:100%}\
body{margin:0;background:#fff;color:#1b1c1f;font:14px/1.5 -apple-system,BlinkMacSystemFont,\"Inter\",\"Segoe UI\",Helvetica,Arial,sans-serif;overflow-wrap:break-word;word-wrap:break-word}\
.pg-root{display:flow-root;box-sizing:border-box;padding:20px 24px;overflow-x:auto;overflow-y:hidden}\
img{max-width:100%;height:auto}\
table{max-width:100%}\
a{color:#0b63ce}\
pre{white-space:pre-wrap}\
blockquote{margin:0 0 0 4px;padding-left:12px;border-left:2px solid #d7d7d7;color:#555}\
html[data-pg-scheme=dark]{color-scheme:dark;background:#1c1c1e}\
:where(html[data-pg-scheme=dark]) body{background:#1c1c1e;color:#e4e4e5}\
:where(html[data-pg-scheme=dark]) a{color:#70b8ff}\
:where(html[data-pg-scheme=dark]) blockquote{border-left-color:#48484a;color:#aeaeb2}";

/// Opens the message's dark-mode sheet (`css::Stylesheets::dark`). It is
/// emitted, possibly empty, for every message that supports a dark canvas,
/// so its presence is the UI's signal; `media="not all"` keeps it off until
/// the UI sets `media="all"`. Keep in sync with MessageBody's darkBody.ts.
const DARK_SHEET_OPEN: &str = "<style class=\"pg-dark-css\" media=\"not all\">";

/// Plain text follows the app theme (`data-theme` is set on <html> by the UI)
/// on a transparent background so it blends into the thread view.
const TEXT_CSS: &str = "\
html{color-scheme:light;background:transparent;color:#191919;-webkit-text-size-adjust:100%}\
html[data-theme=dark]{color-scheme:dark;color:#e4e4e5}\
body{margin:0;background:transparent;font:14px/1.6 -apple-system,BlinkMacSystemFont,\"Inter\",\"Segoe UI\",Helvetica,Arial,sans-serif;overflow-wrap:anywhere}\
.pg-root{display:flow-root;white-space:pre-wrap;padding:2px 0}\
a{color:#006dcb}html[data-theme=dark] a{color:#70b8ff}\
.pg-quote{margin:6px 0;white-space:pre-wrap}\
.pg-quote>summary{display:inline-block;list-style:none;cursor:pointer;user-select:none;padding:0 7px;border-radius:6px;font-size:12px;line-height:16px;letter-spacing:1px;background:rgba(127,127,127,.16);color:inherit;opacity:.75}\
.pg-quote>summary::-webkit-details-marker{display:none}\
.pg-quote>summary:hover{opacity:1}\
.pg-quote blockquote{margin:6px 0 0;padding-left:12px;border-left:2px solid rgba(127,127,127,.35);opacity:.8}\
.pg-attribution{opacity:.8}\
.pg-simplified{white-space:normal;font-size:12px;opacity:.7;margin-bottom:12px}";

fn document(
    prefix: &str,
    allow_remote_images: bool,
    base_css: &str,
    email_css: &str,
    dark_css: Option<&str>,
    root_style: &str,
    body: &str,
) -> String {
    let dark_len = dark_css.map_or(0, str::len);
    let mut out =
        String::with_capacity(body.len() + base_css.len() + email_css.len() + dark_len + 600);
    out.push_str(prefix);
    out.push_str(
        "<head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"",
    );
    out.push_str(&csp(allow_remote_images));
    out.push_str("\"><meta name=\"referrer\" content=\"no-referrer\"><meta name=\"color-scheme\" content=\"light\"><style>");
    out.push_str(base_css);
    out.push_str("</style>");
    if !email_css.is_empty() {
        out.push_str("<style>");
        out.push_str(email_css);
        out.push_str("</style>");
    }
    if let Some(dark_css) = dark_css {
        out.push_str(DARK_SHEET_OPEN);
        out.push_str(dark_css);
        out.push_str("</style>");
    }
    out.push_str("</head><body><div class=\"pg-root\"");
    if !root_style.is_empty() {
        out.push_str(" style=\"");
        out.push_str(&escape_attr(root_style));
        out.push('"');
    }
    out.push('>');
    out.push_str(body);
    out.push_str("</div></body></html>");
    out
}

/// Past these, html5ever's tree construction goes quadratic (scope checks
/// walk the open-element stack; duplicate-attribute checks scan the tag), so
/// a hostile message could stall rendering for seconds. Real mail sits far
/// below both: layout tables nest a few dozen deep. Blink caps parser depth
/// at 512 for the same reason.
const MAX_NESTING_DEPTH: usize = 2048;

/// Shown instead of the HTML when a message trips the complexity guard.
fn render_simplified(html: &str) -> RenderedHtml {
    let text = prescan::prescan(html, true).text;
    let mut body = String::from(
        "<div class=\"pg-simplified\">Simplified view: this message's formatting was too complex to display safely.</div>",
    );
    let text = text.replace("\r\n", "\n");
    let collapsed: Vec<&str> = text.lines().map(str::trim).collect();
    let mut joined = collapsed.join("\n");
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    text::render_body(joined.trim(), 0, &mut body);
    RenderedHtml::text_only(document(TEXT_DOC_PREFIX, false, TEXT_CSS, "", None, "", &body))
}

pub fn render_html(html: &str, opts: &RenderOptions) -> RenderedHtml {
    let scan = prescan::prescan(html, false);
    if scan.max_depth > MAX_NESTING_DEPTH || scan.max_attrs > prescan::MAX_ATTRS {
        return render_simplified(html);
    }
    let policy = Arc::new(ImagePolicy::new(opts, scan.tiny_srcs));
    let body = build_sanitizer(policy.clone()).clean(html).to_string();

    let resolve = |u: &str| policy.resolve(u, false);
    let email_css = css::sanitize_stylesheet(&scan.style_text, &resolve);
    // A message that declares dark support, or ships dark-mode rules, gets
    // its dark sheet (possibly empty) so the UI can use its own dark design.
    let supports_dark = scan.meta_dark_scheme
        || !email_css.dark.is_empty()
        || css::declares_dark_scheme(&scan.style_text);

    // <body> attributes move onto our root wrapper.
    let mut root_style = String::new();
    if let Some(c) = scan.body_bgcolor.as_deref().and_then(simple_color) {
        root_style.push_str(&format!("background-color:{c};"));
    }
    if let Some(c) = scan.body_text_color.as_deref().and_then(simple_color) {
        root_style.push_str(&format!("color:{c};"));
    }
    if let Some(s) = scan.body_style.as_deref() {
        root_style.push_str(&css::sanitize_declarations(s, &resolve));
    }

    let doc = document(
        HTML_DOC_PREFIX,
        opts.allow_remote_images,
        BASE_CSS,
        &email_css.main,
        supports_dark.then_some(email_css.dark.as_str()),
        &root_style,
        &body,
    );
    let mut counts = policy.counts();
    RenderedHtml {
        html: doc,
        blocked_remote_images: counts.blocked.len() as u32,
        trackers_removed: counts.trackers_removed,
        trackers: std::mem::take(&mut counts.entries),
        trackers_allowed: counts.trackers_allowed,
        links_cleaned: counts.cleaned.len() as u32,
        link_params: std::mem::take(&mut counts.link_params),
        tracked_links: counts.tracked_links.len() as u32,
        original_links: std::mem::take(&mut counts.original_links),
    }
}

/// Plain-text body → safe HTML (escape, linkify, fold quoted blocks).
pub fn render_text(text: &str) -> RenderedHtml {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut body = String::with_capacity(text.len() + text.len() / 8);
    text::render_body(text.trim_end(), 0, &mut body);
    RenderedHtml::text_only(document(TEXT_DOC_PREFIX, false, TEXT_CSS, "", None, "", &body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_image_validation() {
        assert!(safe_data_image("data:image/png;base64,iVBORw0KGgo=").is_some());
        assert!(safe_data_image("data:image/svg+xml;base64,PHN2Zz4=").is_none());
        assert!(safe_data_image("data:text/html;base64,PHNjcmlwdD4=").is_none());
        assert!(safe_data_image("data:image/png,<svg onload=alert(1)>").is_none());
        assert!(safe_data_image("data:image/png;base64,abc\")").is_none());
    }

    #[test]
    fn dark_sheet_only_for_messages_that_support_dark() {
        let opts = RenderOptions::default();
        let render = |html: &str| render_html(html, &opts).html;
        let plain = render("<style>.a{color:red}</style><p class=a>hi</p>");
        assert!(!plain.contains("pg-dark-css"));
        let rules = render(
            "<style>.a{color:#111} @media (prefers-color-scheme: dark){.a{color:#eee!important}}</style><p class=a>hi</p>",
        );
        assert!(rules.contains(
            "<style>.a{color:#111;}</style><style class=\"pg-dark-css\" media=\"not all\">@media (min-width:0){.a{color:#eee!important;}}</style></head>"
        ));
        let meta =
            render(r#"<head><meta name="color-scheme" content="light dark"></head><p>hi</p>"#);
        assert!(meta.contains("<style class=\"pg-dark-css\" media=\"not all\"></style></head>"));
        // The body can't forge the marker: style elements never survive there.
        let forged =
            render(r#"<p>x</p><style class="pg-dark-css" media="all">p{color:red}</style>"#);
        assert_eq!(forged.matches("pg-dark-css").count(), 0);
    }

    #[test]
    fn removed_trackers_are_listed_without_query_strings() {
        let html = r#"<p>Hi</p>
            <img src="https://clicks.mlsend.com/td/op/abc123?email=sam%40mail.example&amp;id=77">
            <img src="https://pixel.news.example/o/open.gif?r=sam%40mail.example" width="1" height="1">
            <img src="https://pixel.news.example/o/open.gif?r=other" width="1" height="1">
            <img src="https://x.cmail20.com/t/abc#frag">
            <div style="background:url(https://clicks.mlsend.com/td/op/abc123?email=sam%40mail.example&id=77)">x</div>
            <img src="https://cdn.news.example/hero.jpg" width="600" height="300">"#;
        let r = render_html(html, &RenderOptions::default());
        // The CSS background repeats the first pixel's URL: counted once.
        assert_eq!(r.trackers_removed, 4);
        assert_eq!(r.blocked_remote_images, 1);
        assert_eq!(
            r.trackers,
            vec![
                TrackerRemoved {
                    host: "clicks.mlsend.com".into(),
                    path: "/td/op/abc123".into(),
                    kind: TrackerKind::KnownTracker,
                    company: Some("MailerLite".into()),
                    rule: "host clicks.mlsend.com".into(),
                    had_query: true,
                    count: 1,
                    status: TrackerStatus::Removed,
                },
                TrackerRemoved {
                    host: "pixel.news.example".into(),
                    path: "/o/open.gif".into(),
                    kind: TrackerKind::HiddenImage,
                    company: None,
                    rule: "hidden image".into(),
                    had_query: true,
                    count: 2,
                    status: TrackerStatus::Removed,
                },
                TrackerRemoved {
                    host: "x.cmail20.com".into(),
                    path: "/t/abc".into(),
                    kind: TrackerKind::KnownTracker,
                    company: Some("Campaign Monitor".into()),
                    rule: "host family cmail*".into(),
                    had_query: false,
                    count: 1,
                    status: TrackerStatus::Removed,
                },
            ]
        );
        // Nothing from a query string or fragment reaches the UI.
        let json = serde_json::to_string(&r.trackers).unwrap();
        for leak in ["sam", "%40", "email=", "id=77", "frag", "?"] {
            assert!(!json.contains(leak), "{leak} in {json}");
        }
        assert!(json.contains(r#""kind":"knownTracker""#), "{json}");
        assert!(json.contains(r#""kind":"hiddenImage""#), "{json}");
        assert!(json.contains(r#""hadQuery":true"#), "{json}");
    }

    #[test]
    fn tracker_list_is_capped_but_the_count_is_not() {
        let mut html = String::new();
        for i in 0..(MAX_TRACKER_ENTRIES + 10) {
            html.push_str(&format!(
                r#"<img src="https://p{i}.news.example/o.gif" width="1" height="1">"#
            ));
        }
        let long = format!("/{}", "a".repeat(200));
        html.push_str(&format!(r#"<img src="https://r.superhuman.com{long}">"#));
        let r = render_html(&html, &RenderOptions::default());
        assert_eq!(r.trackers_removed as usize, MAX_TRACKER_ENTRIES + 11);
        assert_eq!(r.trackers.len(), MAX_TRACKER_ENTRIES);
        assert!(r
            .trackers
            .iter()
            .all(|t| t.kind == TrackerKind::HiddenImage));
        assert_eq!(cap_path(&long).chars().count(), MAX_TRACKER_PATH);
        assert!(cap_path(&long).ends_with('…'));
        assert_eq!(cap_path("/o.gif"), "/o.gif");
    }

    #[test]
    fn text_and_clean_html_list_no_trackers() {
        assert!(render_text("hello https://r.superhuman.com/x.png")
            .trackers
            .is_empty());
        let r = render_html(
            "<p>plain <img src=\"https://cdn.news.example/a.png\"></p>",
            &RenderOptions::default(),
        );
        assert!(r.trackers.is_empty());
        assert_eq!(r.trackers_removed, 0);
    }

    const PIXELS: &str = r#"<p>Hi</p>
        <img src="https://r.superhuman.com/abc.png">
        <img src="https://pixel.news.example/o.gif" width="1" height="1">
        <img src="https://cdn.news.example/hero.jpg" width="600" height="300">"#;

    #[test]
    fn tracking_pixels_can_be_allowed_and_are_still_reported() {
        // Blocking off, images blocked: the pixels wait with the other images.
        let held = render_html(
            PIXELS,
            &RenderOptions {
                allow_tracking_pixels: true,
                ..RenderOptions::default()
            },
        );
        assert_eq!(held.trackers_removed, 0);
        assert_eq!(held.trackers_allowed, 2);
        assert_eq!(held.blocked_remote_images, 3);
        assert!(held.trackers.iter().all(|t| t.status == TrackerStatus::Held));
        assert!(!held.html.contains("superhuman"));

        // Blocking off, images allowed: they load like any image.
        let loaded = render_html(
            PIXELS,
            &RenderOptions {
                allow_tracking_pixels: true,
                allow_remote_images: true,
                ..RenderOptions::default()
            },
        );
        assert_eq!(loaded.trackers_allowed, 2);
        assert_eq!(loaded.blocked_remote_images, 0);
        assert!(loaded.trackers.iter().all(|t| t.status == TrackerStatus::Loaded));
        assert!(loaded.html.contains("https://r.superhuman.com/abc.png"));
        assert!(loaded.html.contains("https://pixel.news.example/o.gif"));

        // The default blocks them even when images load.
        let blocked = render_html(
            PIXELS,
            &RenderOptions {
                allow_remote_images: true,
                ..RenderOptions::default()
            },
        );
        assert_eq!(blocked.trackers_removed, 2);
        assert_eq!(blocked.trackers_allowed, 0);
        assert!(!blocked.html.contains("superhuman"));
        assert!(blocked.html.contains("hero.jpg"));
        let json = serde_json::to_string(&blocked.trackers).unwrap();
        assert!(json.contains(r#""status":"removed""#), "{json}");
    }

    const LINKS: &str = r#"<p><a href="https://shop.example/sale?utm_source=news&amp;mc_eid=ab12&amp;fbclid=IwAR">Sale</a>
        <a href="https://shop.example/sale?utm_source=news&amp;mc_eid=zz99">Sale again</a>
        <a href="https://links.shop.example/ls/click?upn=opaque">Tracked</a>
        <a href="https://shop.example/about">About</a>
        <a href="https://pages.shop.example/UnsubscribePage.html?mkt_tok=abc">Unsubscribe</a></p>"#;

    #[test]
    fn link_tracking_is_counted_and_optionally_removed() {
        let off = render_html(LINKS, &RenderOptions::default());
        assert_eq!(off.links_cleaned, 0);
        assert!(off.link_params.is_empty());
        assert_eq!(off.tracked_links, 1);
        assert!(off.html.contains("mc_eid=ab12"));

        let on = render_html(
            LINKS,
            &RenderOptions {
                strip_link_tracking: true,
                ..RenderOptions::default()
            },
        );
        assert_eq!(on.links_cleaned, 2);
        assert_eq!(on.link_params, vec!["mc_eid", "fbclid"]);
        assert_eq!(on.tracked_links, 1);
        assert!(!on.html.contains("mc_eid"), "{}", on.html);
        assert!(!on.html.contains("fbclid"));
        assert!(on
            .html
            .contains(r#"href="https://shop.example/sale?utm_source=news""#));
        // The unsubscribe page keeps the parameter it needs.
        assert!(on.html.contains("mkt_tok=abc"));
        // Callers can get back to the link as it was sent.
        assert_eq!(
            on.original_link("https://shop.example/sale?utm_source=news"),
            "https://shop.example/sale?utm_source=news&mc_eid=ab12&fbclid=IwAR"
        );
        assert_eq!(on.original_link("https://shop.example/about"), "https://shop.example/about");
        // Values never reach the UI.
        let json = serde_json::to_string(&on.link_params).unwrap();
        assert!(!json.contains("ab12") && !json.contains("IwAR"));
    }

    #[test]
    fn cid_normalization() {
        assert_eq!(normalize_cid("<Logo@Acme.example>"), "logo@acme.example");
        assert_eq!(normalize_cid("logo%40acme.example"), "logo@acme.example");
        assert_eq!(normalize_cid("bad%zz"), "bad%zz");
        assert_eq!(normalize_cid("end%4"), "end%4");
    }
}

#[cfg(test)]
mod test_dom;
#[cfg(test)]
mod xss_tests;
