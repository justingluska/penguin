//! Link tracking in received mail: query parameters that identify the
//! recipient or the ad click (`mc_eid`, `_hsenc`, `mkt_tok`, `fbclid`), and
//! links wrapped by a click-tracking service.
//!
//! With `RenderOptions::strip_link_tracking` the renderer removes the
//! parameters on this list from every `<a href>` before the message is
//! shown, so the link the user opens (and sees on hover or copy) no longer
//! carries them. Same idea as Apple's Link Tracking Protection and
//! Firefox's query parameter stripping (sources in docs/PRIVACY.md).
//!
//! What it can't do: a link that goes through the sender's click tracker
//! (`links.example/ls/click?upn=…`) hides the destination inside an opaque
//! token, so the click is recorded whatever Penguin does. Those links are
//! counted (`is_tracked_link`) so the privacy details can say so.

use ammonia::Url;

/// Parameters removed from links, matched case-insensitively on the whole
/// name. This is Firefox's query-stripping list (Remote Settings collection
/// `query-stripping`, all of it) plus the entries of Brave's
/// `query-filter.json` that are per-person or per-click ids and not tied to
/// one site. Campaign tags (`utm_*`, `mc_cid`) are deliberately kept: they
/// name the newsletter, not you, and neither browser strips them.
const PARAMS: &[&str] = &[
    // Firefox
    "__s",    // Drip
    "_hsenc", // HubSpot
    "fbclid",
    "mc_eid", // Mailchimp subscriber id
    "mkt_tok", // Marketo
    "oly_anon_id", // Omeda
    "oly_enc_id",
    "vero_id", // Vero
    "__hsfp", // HubSpot
    "__hssc",
    "__hstc",
    "_openstat",
    "dclid",
    "gbraid",
    "gclid",
    "hsctatracking",
    "msclkid",
    "twclid",
    "wbraid",
    "wickedid",
    "yclid",
    "ysclid",
    // Brave additions
    "_branch_match_id",
    "bsft_clkid", // Blueshift
    "et_rid",     // Acoustic
    "fb_action_ids",
    "guce_referrer",
    "igshid",
    "irclickid",
    "ml_subscriber", // MailerLite
    "ml_subscriber_hash",
    "rb_clickid",
    "s_cid",      // Adobe
    "sfmc_id",    // Salesforce Marketing Cloud
    "sfmc_activityid",
    "srsltid",
    "ss_email_id", // Squarespace
    "ttclid",
    "vero_conv",
    "ymclid",
];

/// Paths of pages that manage the subscription itself. Their links are left
/// alone: some identify you with exactly these parameters (Marketo's
/// unsubscribe page reads `mkt_tok`), and removing them would break the
/// unsubscribe.
const KEEP_PATH_WORDS: &[&str] = &[
    "unsub",
    "optout",
    "opt-out",
    "opt_out",
    "preference",
    "subscription",
    "manage",
    "remove",
];

/// Path fragments of click-tracking redirects on the sender's own link
/// domain (the tracker list in `trackers.rs` covers dedicated hosts).
/// Compared against the lowercased path.
const CLICK_PATHS: &[&str] = &[
    "/ls/click",    // SendGrid
    "/wf/click",    // SendGrid
    "/track/click", // Mailchimp, Mandrill
    "/e1t/c/",      // HubSpot
    "/e2t/c/",
    "/e3t/c/",
    "/ss/c/",       // beehiiv
    "/tr/cl/",      // Brevo
    "/click.aspx",  // Salesforce Marketing Cloud
];

fn is_tracking_param(name: &str) -> bool {
    PARAMS.iter().any(|p| name.eq_ignore_ascii_case(p))
}

/// A link to a subscription-management page (see `KEEP_PATH_WORDS`).
fn manages_subscription(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    let query = url.query().unwrap_or_default().to_ascii_lowercase();
    KEEP_PATH_WORDS
        .iter()
        .any(|w| path.contains(w) || query.contains(w))
}

/// The result of cleaning one link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleaned {
    /// The link without the tracking parameters.
    pub url: String,
    /// Names of the removed parameters, lowercased, as they appeared (a
    /// name repeated in the link is listed once).
    pub removed: Vec<String>,
}

/// `url` without its tracking parameters, or None when it has none (or is
/// a subscription-management link, or not http(s)). Everything else about
/// the URL (other parameters and their order, the fragment) is kept.
pub fn strip_tracking_params(url: &Url) -> Option<Cleaned> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let query = url.query()?;
    if query.is_empty() || manages_subscription(url) {
        return None;
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for pair in query.split('&') {
        let raw_name = pair.split('=').next().unwrap_or_default();
        // Names are compared decoded only for the common %5F ("_") case;
        // anything more exotic is left in place.
        let name = raw_name.replace("%5F", "_").replace("%5f", "_");
        if !name.is_empty() && is_tracking_param(&name) {
            let lower = name.to_ascii_lowercase();
            if !removed.contains(&lower) {
                removed.push(lower);
            }
        } else if !pair.is_empty() {
            kept.push(pair);
        }
    }
    if removed.is_empty() {
        return None;
    }
    let mut out = url.clone();
    if kept.is_empty() {
        out.set_query(None);
    } else {
        out.set_query(Some(&kept.join("&")));
    }
    Some(Cleaned {
        url: out.to_string(),
        removed,
    })
}

/// True when this link goes through a click-tracking redirect: a known
/// tracker host (`trackers::match_tracker`) or a click path an email
/// service puts on the sender's own link domain.
pub fn is_tracked_link(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    if crate::trackers::match_tracker(url).is_some() {
        return true;
    }
    let path = url.path().to_ascii_lowercase();
    CLICK_PATHS.iter().any(|p| path.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(u: &str) -> Option<(String, Vec<String>)> {
        strip_tracking_params(&Url::parse(u).unwrap()).map(|c| (c.url, c.removed))
    }

    #[test]
    fn removes_only_tracking_parameters() {
        let (url, removed) = clean(
            "https://shop.example/p/42?color=blue&utm_source=news&mc_eid=ab12&size=m&mc_cid=c9#reviews",
        )
        .unwrap();
        // Campaign tags stay: they name the newsletter, not the reader.
        assert_eq!(
            url,
            "https://shop.example/p/42?color=blue&utm_source=news&size=m&mc_cid=c9#reviews"
        );
        assert_eq!(removed, vec!["mc_eid"]);
    }

    #[test]
    fn a_link_with_only_tracking_loses_its_query() {
        let (url, removed) =
            clean("https://blog.example/post?fbclid=IwAR0&_hsenc=p2AN&mkt_tok=99").unwrap();
        assert_eq!(url, "https://blog.example/post");
        assert_eq!(removed, vec!["fbclid", "_hsenc", "mkt_tok"]);
    }

    #[test]
    fn names_are_case_insensitive_and_listed_once() {
        let (url, removed) =
            clean("https://a.example/?MC_EID=x&mc_eid=y&GCLID=1&q=penguin").unwrap();
        assert_eq!(url, "https://a.example/?q=penguin");
        assert_eq!(removed, vec!["mc_eid", "gclid"]);
    }

    #[test]
    fn leaves_clean_and_look_alike_links_alone() {
        assert_eq!(clean("https://a.example/page"), None);
        assert_eq!(clean("https://a.example/page?id=7&ref=home"), None);
        // Not on the list, even though they look similar.
        assert_eq!(clean("https://a.example/?utm=1&source=x&eid=3&hsenc=4"), None);
        assert_eq!(clean("https://a.example/?utm_source=x&utm_medium=email"), None);
        assert_eq!(clean("mailto:sam@mail.example?subject=fbclid"), None);
    }

    #[test]
    fn subscription_pages_keep_their_parameters() {
        // Marketo's unsubscribe page identifies you by mkt_tok.
        assert_eq!(
            clean("https://pages.shop.example/UnsubscribePage.html?mkt_unsubscribe=1&mkt_tok=abc"),
            None
        );
        assert_eq!(
            clean("https://news.example/preferences?_hsenc=p2&__hstc=1"),
            None
        );
    }

    #[test]
    fn encoded_underscore_still_matches() {
        let (url, removed) = clean("https://a.example/?mc%5Feid=x&k=1").unwrap();
        assert_eq!(url, "https://a.example/?k=1");
        assert_eq!(removed, vec!["mc_eid"]);
    }

    #[test]
    fn click_trackers_are_recognized() {
        let t = |u: &str| is_tracked_link(&Url::parse(u).unwrap());
        assert!(t("https://u123.ct.sendgrid.net/ls/click?upn=abc"));
        assert!(t("https://acme.us1.list-manage.com/track/click?u=1&id=2&e=3"));
        assert!(t("https://links.acme.example/ls/click?upn=abc"));
        assert!(t("https://clicks.mlsend.com/tj/c/abc"));
        assert!(t("https://d2v8tf04.na1.hubspotlinks.example/Ctc/x/e3t/c/abc"));
        assert!(!t("https://shop.example/c/shoes"));
        assert!(!t("https://www.example/blog/click-here"));
    }
}
