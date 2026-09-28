//! Known open-tracking pixels. Applied to every image URL, even after the
//! user clicks "Load images": loading a newsletter's pictures shouldn't also
//! tell the sender you opened it.
//!
//! Compiled from vendor documentation and observed pixel URLs, cross-checked
//! against public blocklists (UglyEmail, Leave Me Alone, Fastmail's writeup).
//! Tiny images (see `prescan`) are caught separately, so this list only has
//! to cover pixels that aren't sized 1×1 in the markup.
//!
//! Each entry names the company behind it where the entry is specific to
//! one, so the message's privacy details can say who was watching.

use ammonia::Url;

/// Hosts that only serve tracking (any image from them, including
/// subdomains, is a tracker), with the company behind each where known.
const HOSTS: &[(&str, Option<&str>)] = &[
    // Sales-engagement / read-receipt tools
    ("r.superhuman.com", Some("Superhuman")),
    ("track.mixmax.com", Some("Mixmax")),
    ("email.mixmax.com", Some("Mixmax")),
    ("email-analytics.mixmax.com", Some("Mixmax")),
    ("t.yesware.com", Some("Yesware")),
    ("app.yesware.com", Some("Yesware")),
    ("mailfoogae.appspot.com", Some("Streak")),
    ("mailtrack.io", Some("Mailtrack")),
    ("mltrk.io", Some("Mailtrack")),
    ("mailstat.us", Some("Boomerang")),
    ("bl-1.com", Some("Bananatag")),
    ("tracking.cirrusinsight.com", Some("Cirrus Insight")),
    ("yamm-track.appspot.com", Some("Yet Another Mail Merge")),
    ("t.mailpgn.com", Some("Pigeon")),
    ("track.gmass.co", Some("GMass")),
    ("x.gmtrack.net", None),
    ("bowtie.mailbutler.io", Some("Mailbutler")),
    ("tr.cloudmagic.com", Some("Newton Mail")),
    ("toutapp.com", Some("ToutApp")),
    ("emltrk.com", Some("Litmus")),
    ("gml.email", Some("Gmelius")),
    ("signaldomn.online", Some("Snov.io")),
    // HubSpot sales/marketing open tracking
    ("t.signaux.com", Some("HubSpot")),
    ("t.senal.com", Some("HubSpot")),
    ("t.signale.com", Some("HubSpot")),
    ("t.signauxtrois.com", Some("HubSpot")),
    ("t.hubspotemail.net", Some("HubSpot")),
    ("t.hubspotfree.net", Some("HubSpot")),
    ("t.hubspotstarter.net", Some("HubSpot")),
    // ESP open endpoints on dedicated hosts
    ("awstrack.me", Some("Amazon SES")),
    ("pstmrk.it", Some("Postmark")),
    ("openrate.aweber.com", Some("AWeber")),
    ("trk.klaviyomail.com", Some("Klaviyo")),
    ("trk.klaviyo.com", Some("Klaviyo")),
    ("track.customer.io", Some("Customer.io")),
    ("e.customeriomail.com", Some("Customer.io")),
    ("eotrx.substackcdn.com", Some("Substack")),
    ("clicks.mlsend.com", Some("MailerLite")),
    ("email.mg.substack.com", Some("Substack")),
    ("rover.ebay.com", Some("eBay")),
    ("fls-na.amazon.com", Some("Amazon")),
    ("fls-eu.amazon.com", Some("Amazon")),
    ("fls-fe.amazon.com", Some("Amazon")),
    ("t.paypal.com", Some("PayPal")),
    ("pixel.app.returnpath.net", Some("Return Path")),
    ("ea.twitter.com", Some("X (Twitter)")),
];

/// Host labels that identify rotating tracking domains, e.g.
/// `t.sidekickopen87.com` (HubSpot) or `x.cmail20.com` (Campaign Monitor).
/// Matched against the registrable label with its trailing digits removed.
const HOST_LABEL_STEMS: &[(&str, Option<&str>)] = &[
    ("sidekickopen", Some("HubSpot")),
    ("sigopn", Some("HubSpot")),
    ("cmail", Some("Campaign Monitor")),
    ("sendibt", Some("Brevo")),
    ("sendibm", Some("Brevo")),
    ("strk", None),
];

/// (host suffix or "" for any host, path/query substring, company). Covers
/// ESPs whose pixel lives on the sender's own tracking domain.
const PATHS: &[(&str, &str, Option<&str>)] = &[
    ("", "/track/open.php", Some("Mailchimp")), // also Mandrill
    ("", "/track/open?", None),
    ("", "/wf/open", Some("SendGrid")), // branded link domains
    ("", "/open.aspx", Some("Salesforce Marketing Cloud")), // ExactTarget
    ("", "/e/eo?", Some("Iterable")),
    ("", "/tr/op/", Some("Brevo")), // Sendinblue
    ("", "/ss/o/", Some("beehiiv")),
    ("", "/e2t/o/", Some("HubSpot")),
    ("", "/e1t/o/", Some("HubSpot")),
    ("", "/e3t/o/", Some("HubSpot")),
    ("", "/trk?t=", Some("Marketo")),
    ("", "/on.jsp", Some("Constant Contact")), // rs6.net
    ("", "/api/mailings/opened", Some("Outreach")),
    ("", "/email_opened", Some("Close")),
    ("", "/api/v1/tracker", Some("ContactMonkey")),
    ("", "/api/v1/track/email", Some("NetHunt")),
    ("", "/api/track/open", None),
    ("", "/emimp/", Some("LinkedIn")),
    ("", "/notifications/beacon/", Some("GitHub")),
    ("", "/qemail/mark_read", Some("Quora")),
    ("", "/_/stat?", Some("Medium")),
    ("", "/roveropen/", Some("eBay")),
    ("", "/scribe/ibis", Some("X (Twitter)")),
    ("", "/FooterImages/FooterImage", Some("Oracle Eloqua")),
    ("", "/lt.php?", Some("ActiveCampaign")), // open pixel variant
    ("mjt.lu", "/oo/", Some("Mailjet")),
    ("sparkpostmail.com", "/q/", Some("SparkPost")),
    ("list-manage.com", "/track", Some("Mailchimp")),
    ("polymail.io", "/v2/z", Some("Polymail")),
    ("frontapp.com", "/seen", Some("Front")),
    ("getmailspring.com", "/open", Some("Mailspring")),
    ("intercom-mail.com", "/o", Some("Intercom")),
    ("intercom.io", "/o/", Some("Intercom")),
    ("nytimes.com", "/pixel", Some("The New York Times")),
    ("google-analytics.com", "/collect", Some("Google Analytics")),
    ("facebook.com", "/tr", Some("Meta")),
    ("convertkit-mail.com", "/o/", Some("Kit (ConvertKit)")),
    ("convertkit-mail2.com", "/o/", Some("Kit (ConvertKit)")),
];

fn host_matches(host: &str, suffix: &str) -> bool {
    host == suffix || host.strip_suffix(suffix).is_some_and(|h| h.ends_with('.'))
}

/// Which list entry flagged a URL as a tracker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerMatch {
    /// The company behind it, when the entry is specific to one.
    pub company: Option<&'static str>,
    /// The entry that matched, worded for the UI: `host awstrack.me`,
    /// `host family cmail*`, `path /wf/open`, `path /oo/ on mjt.lu`.
    pub rule: String,
}

/// The list entry that marks this image URL as a known open-tracking pixel.
pub fn match_tracker(url: &Url) -> Option<TrackerMatch> {
    let host = url.host_str()?.to_ascii_lowercase();
    if let Some((h, company)) = HOSTS.iter().find(|(h, _)| host_matches(&host, h)) {
        return Some(TrackerMatch {
            company: *company,
            rule: format!("host {h}"),
        });
    }
    // Registrable label = second-to-last label (good enough for .com/.net stems).
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() >= 2 {
        let stem = labels[labels.len() - 2].trim_end_matches(|c: char| c.is_ascii_digit());
        if let Some((s, company)) = HOST_LABEL_STEMS.iter().find(|(s, _)| *s == stem) {
            return Some(TrackerMatch {
                company: *company,
                rule: format!("host family {s}*"),
            });
        }
    }
    let mut path = url.path().to_string();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    PATHS
        .iter()
        .find(|(h, p, _)| (h.is_empty() || host_matches(&host, h)) && path.contains(p))
        .map(|(h, p, company)| TrackerMatch {
            company: *company,
            rule: if h.is_empty() {
                format!("path {p}")
            } else {
                format!("path {p} on {h}")
            },
        })
}

/// True if this image URL is a known open-tracking pixel.
pub fn is_tracker(url: &Url) -> bool {
    match_tracker(url).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(u: &str) -> bool {
        is_tracker(&Url::parse(u).unwrap())
    }

    fn m(u: &str) -> TrackerMatch {
        match_tracker(&Url::parse(u).unwrap()).unwrap()
    }

    #[test]
    fn known_trackers() {
        assert!(t(
            "https://acme.us1.list-manage.com/track/open.php?u=1&id=2&e=3"
        ));
        assert!(t("https://mandrillapp.com/track/open.php?u=1"));
        assert!(t("https://u123.ct.sendgrid.net/wf/open?upn=abc"));
        assert!(t("https://links.acme.example/wf/open?upn=abc"));
        assert!(t("https://t.sidekickopen87.com/e1t/o/5/abc"));
        assert!(t("https://r.superhuman.com/abc.png"));
        assert!(t("https://track.mixmax.com/api/track/v2/abc"));
        assert!(t("https://t.yesware.com/t/abc/o.gif"));
        assert!(t("https://mailfoogae.appspot.com/t?sender=abc"));
        assert!(t("https://mailtrack.io/trace/mail/abc.png"));
        assert!(t("https://r.us-east-1.awstrack.me/I0/abc"));
        assert!(t("https://x.cmail20.com/t/abc"));
        assert!(t("https://github.com/notifications/beacon/abc.gif"));
        assert!(t("https://www.linkedin.com/emimp/ip_abc.gif"));
        assert!(t("https://abc.r.bh.d.sendibt3.com/tr/op/abc"));
        assert!(t("https://eotrx.substackcdn.com/open?token=abc"));
    }

    #[test]
    fn matches_name_the_rule_and_company() {
        let a = m("https://r.us-east-1.awstrack.me/I0/abc");
        assert_eq!(a.company, Some("Amazon SES"));
        assert_eq!(a.rule, "host awstrack.me");
        let c = m("https://x.cmail20.com/t/abc");
        assert_eq!(c.company, Some("Campaign Monitor"));
        assert_eq!(c.rule, "host family cmail*");
        let s = m("https://links.acme.example/wf/open?upn=abc");
        assert_eq!(s.company, Some("SendGrid"));
        assert_eq!(s.rule, "path /wf/open");
        let j = m("https://x.mjt.lu/oo/abc");
        assert_eq!(j.company, Some("Mailjet"));
        assert_eq!(j.rule, "path /oo/ on mjt.lu");
        // A generic entry doesn't guess a company.
        assert_eq!(
            m("https://t.acme.example/api/track/open?id=1").company,
            None
        );
    }

    #[test]
    fn ordinary_images_pass() {
        assert!(!t("https://mcusercontent.com/abc/images/hero.png"));
        assert!(!t("https://cdn.acme.example/newsletter/header.jpg"));
        assert!(!t("https://github.githubassets.com/images/email/logo.png"));
        assert!(!t("https://images.example/track/opening-night.jpg"));
        assert!(!t("https://notsuperhuman.com/a.png"));
    }
}
