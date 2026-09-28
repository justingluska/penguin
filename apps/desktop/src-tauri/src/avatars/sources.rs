//! The avatar sources, each turning an identity into our PNG or a miss:
//! BIMI logos, company icons, Gravatar and Google contact photos.

use sha2::{Digest, Sha256};

use super::cache::hex;
use super::image::{self, MIN_ICON_EDGE};
use super::net::{FetchError, Net};

/// Result of asking one source about one identity.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Our normalized PNG.
    Image(Vec<u8>),
    /// The source has nothing (cached for the source's miss TTL).
    Miss,
    /// Couldn't tell (offline, 5xx): cached briefly.
    Error(String),
}

/// Mailbox providers: a person at gmail.com shouldn't get Gmail's logo.
const MAILBOX_PROVIDERS: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "outlook.com",
    "hotmail.com",
    "live.com",
    "msn.com",
    "yahoo.com",
    "ymail.com",
    "rocketmail.com",
    "aol.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "proton.me",
    "protonmail.com",
    "pm.me",
    "gmx.com",
    "gmx.de",
    "gmx.net",
    "web.de",
    "mail.com",
    "zoho.com",
    "yandex.com",
    "yandex.ru",
    "mail.ru",
    "fastmail.com",
    "fastmail.fm",
    "hey.com",
    "tutanota.com",
    "tuta.io",
    "qq.com",
    "163.com",
    "126.com",
    "naver.com",
    "comcast.net",
    "att.net",
    "verizon.net",
];

/// Second-level public suffixes common enough to matter for the
/// organizational-domain heuristic (no full Public Suffix List needed:
/// a wrong guess only costs a missing icon).
const TWO_PART_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "com.au", "net.au", "org.au", "co.nz", "co.jp",
    "ne.jp", "or.jp", "co.kr", "co.in", "co.za", "com.br", "com.mx", "com.ar", "com.sg", "com.hk",
    "com.tw", "com.cn", "com.tr", "co.il", "com.my", "com.ph", "co.id",
];

/// Lowercased domain of an address, when it's a plausible DNS name.
pub fn domain_of(email: &str) -> Option<String> {
    let (_, domain) = email.trim().rsplit_once('@')?;
    let d = domain.trim_end_matches('.').to_ascii_lowercase();
    let ok = d.len() <= 253
        && d.contains('.')
        && d.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && !d.split('.').all(|l| l.bytes().all(|b| b.is_ascii_digit()));
    ok.then_some(d)
}

/// `news.shop.co.uk` → `shop.co.uk`; `mail.shop.example` → `shop.example`.
pub fn org_domain(domain: &str) -> String {
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() <= 2 {
        return domain.to_string();
    }
    let last_two = labels[labels.len() - 2..].join(".");
    let keep = if TWO_PART_SUFFIXES.contains(&last_two.as_str()) {
        3
    } else {
        2
    };
    labels[labels.len().saturating_sub(keep)..].join(".")
}

pub fn is_mailbox_provider(domain: &str) -> bool {
    let org = org_domain(domain);
    MAILBOX_PROVIDERS.contains(&org.as_str())
        || org.starts_with("yahoo.")
        || org.starts_with("hotmail.")
        || org.starts_with("outlook.")
}

/// `sha256(lowercased address)`, the Gravatar identity.
pub fn email_hash(email: &str) -> String {
    hex(&Sha256::digest(email.trim().to_lowercase().as_bytes()))
}

// ---------- BIMI ----------

/// The logo URL from a BIMI assertion record, if it's a valid v=BIMI1
/// record with an https `l=`. An empty `l=` (declined) gives None.
pub fn parse_bimi(record: &str) -> Option<String> {
    let mut tags = record.split(';').map(str::trim).filter(|t| !t.is_empty());
    let first = tags.next()?;
    let (k, v) = first.split_once('=')?;
    if !k.trim().eq_ignore_ascii_case("v") || !v.trim().eq_ignore_ascii_case("BIMI1") {
        return None;
    }
    for tag in tags {
        let Some((k, v)) = tag.split_once('=') else {
            continue;
        };
        if k.trim().eq_ignore_ascii_case("l") {
            let url = v.trim().split(',').next()?.trim();
            return url
                .get(..8)
                .filter(|p| p.eq_ignore_ascii_case("https://"))
                .map(|_| url.to_string());
        }
    }
    None
}

/// `default._bimi.<domain>`, falling back to the organizational domain.
/// Exactly one v=BIMI1 record must exist at a name, per the spec.
pub async fn bimi(net: &dyn Net, domain: &str) -> Outcome {
    let org = org_domain(domain);
    let mut names = vec![domain.to_string()];
    if org != domain {
        names.push(org);
    }
    for name in names {
        let records = match net.txt(&format!("default._bimi.{name}")).await {
            Ok(r) => r,
            Err(e) => return Outcome::Error(e),
        };
        let bimi: Vec<&String> = records
            .iter()
            .filter(|r| r.trim_start().to_ascii_lowercase().starts_with("v=bimi1"))
            .collect();
        match bimi.as_slice() {
            [] => continue,
            [one] => {
                let Some(url) = parse_bimi(one) else {
                    return Outcome::Miss;
                };
                return match net.get(&url, image::MAX_SVG_BYTES, None).await {
                    Ok(bytes) if image::looks_like_svg(&bytes) => {
                        match image::rasterize_svg(&bytes) {
                            Ok(png) => Outcome::Image(png),
                            Err(e) => {
                                tracing::debug!(error = %e, "BIMI logo rejected");
                                Outcome::Miss
                            }
                        }
                    }
                    Ok(_) => Outcome::Miss,
                    Err(FetchError::Transient(e)) => Outcome::Error(e),
                    Err(_) => Outcome::Miss,
                };
            }
            _ => return Outcome::Miss,
        }
    }
    Outcome::Miss
}

// ---------- company icons ----------

const ICON_PATHS: &[&str] = &["/apple-touch-icon.png", "/favicon.ico"];
const MAX_ICON_BYTES: usize = 512 * 1024;

/// apple-touch-icon, then favicon, on the organizational domain and then
/// its `www.` host. A host that can't be reached skips to the next host.
pub async fn company_icon(net: &dyn Net, domain: &str) -> Outcome {
    let org = org_domain(domain);
    let mut transient = None;
    let mut definite = false;
    for host in [org.clone(), format!("www.{org}")] {
        for path in ICON_PATHS {
            let url = format!("https://{host}{path}");
            match net.get(&url, MAX_ICON_BYTES, None).await {
                Ok(bytes) => {
                    definite = true;
                    // Many sites answer every path with an HTML page.
                    if image::looks_like_svg(&bytes) {
                        continue;
                    }
                    if let Ok(png) = image::normalize_raster(&bytes, MIN_ICON_EDGE) {
                        return Outcome::Image(png);
                    }
                }
                Err(FetchError::Transient(e)) => {
                    transient = Some(e);
                    break; // this host is unreachable; try the next one
                }
                Err(_) => definite = true,
            }
        }
    }
    match transient {
        Some(e) if !definite => Outcome::Error(e),
        _ => Outcome::Miss,
    }
}

// ---------- Gravatar ----------

pub async fn gravatar(net: &dyn Net, email: &str) -> Outcome {
    let url = format!(
        "https://gravatar.com/avatar/{}?s=128&d=404",
        email_hash(email)
    );
    fetch_photo(net, &url).await
}

// ---------- Google contact photos ----------

/// A People API photo URL we're willing to fetch, asking for 128 px.
pub fn contact_photo_url(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    if parsed.scheme() != "https"
        || !(host == "googleusercontent.com" || host.ends_with(".googleusercontent.com"))
    {
        return None;
    }
    // `…=s100` (optionally `-c` etc.) → `…=s128-c`.
    if let Some((base, opts)) = url.rsplit_once('=') {
        if opts.starts_with('s') && opts[1..].chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Some(format!("{base}=s128-c"));
        }
    }
    Some(url.to_string())
}

pub async fn contact_photo(net: &dyn Net, url: &str) -> Outcome {
    match contact_photo_url(url) {
        Some(u) => fetch_photo(net, &u).await,
        None => Outcome::Miss,
    }
}

async fn fetch_photo(net: &dyn Net, url: &str) -> Outcome {
    match net.get(url, image::MAX_RASTER_BYTES, None).await {
        Ok(bytes) => match image::normalize_raster(&bytes, 1) {
            Ok(png) => Outcome::Image(png),
            Err(_) => Outcome::Miss,
        },
        Err(FetchError::Transient(e)) => Outcome::Error(e),
        Err(_) => Outcome::Miss,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains() {
        assert_eq!(
            domain_of("Bo@News.Shop.Example").as_deref(),
            Some("news.shop.example")
        );
        assert_eq!(domain_of("x@localhost"), None);
        assert_eq!(domain_of("x@[192.168.1.1]"), None);
        assert_eq!(domain_of("x@10.0.0.1"), None);
        assert_eq!(domain_of("x@bad_domain.example"), None);
        assert_eq!(domain_of("no-at-sign"), None);
        assert_eq!(org_domain("news.shop.example"), "shop.example");
        assert_eq!(org_domain("e.mail.shop.co.uk"), "shop.co.uk");
        assert_eq!(org_domain("shop.example"), "shop.example");
        assert!(is_mailbox_provider("gmail.com"));
        assert!(is_mailbox_provider("yahoo.co.uk"));
        assert!(!is_mailbox_provider("shop.example"));
    }

    #[test]
    fn bimi_records() {
        assert_eq!(
            parse_bimi("v=BIMI1; l=https://shop.example/logo.svg; a=https://shop.example/vmc.pem")
                .as_deref(),
            Some("https://shop.example/logo.svg")
        );
        assert_eq!(
            parse_bimi("V=bimi1;L=https://a.example/l.svg").as_deref(),
            Some("https://a.example/l.svg")
        );
        assert_eq!(parse_bimi("v=BIMI1; l=; a=;"), None, "declined");
        assert_eq!(parse_bimi("v=BIMI1; l=http://a.example/l.svg"), None);
        assert_eq!(parse_bimi("v=spf1 include:_spf.example ~all"), None);
        assert_eq!(parse_bimi("l=https://a.example/l.svg; v=BIMI1"), None);
    }

    #[test]
    fn photo_urls() {
        assert_eq!(
            contact_photo_url("https://lh3.googleusercontent.com/cm/ABC=s100").as_deref(),
            Some("https://lh3.googleusercontent.com/cm/ABC=s128-c")
        );
        assert_eq!(
            contact_photo_url("https://lh3.googleusercontent.com/a-/XYZ").as_deref(),
            Some("https://lh3.googleusercontent.com/a-/XYZ")
        );
        assert_eq!(contact_photo_url("https://evil.example/x.png"), None);
        assert_eq!(
            contact_photo_url("https://googleusercontent.com.evil.example/x"),
            None
        );
        assert_eq!(
            contact_photo_url("http://lh3.googleusercontent.com/x"),
            None
        );
    }

    #[test]
    fn gravatar_uses_a_hash_of_the_address() {
        assert_eq!(
            email_hash(" Bo@Shop.example "),
            email_hash("bo@shop.example")
        );
        assert_eq!(email_hash("x").len(), 64);
    }
}
