//! Unsubscribe, the acting half (the deciding half is
//! `penguin_core::unsubscribe`, which plans from the stored message). Every
//! action here is a user click the app has already confirmed; nothing runs
//! on its own. Shared by the Mac (src-tauri unsubscribe.rs) and the iPhone
//! (penguin-ffi), for every provider.
//!
//! - [`plan_for`]: the plan for a stored message, looking for an
//!   unsubscribe link in the rendered body only when the headers give none.
//! - One-click (RFC 8058): [`one_click_post`] POSTs `List-Unsubscribe=One-Click`
//!   to the header's https URL: no cookies, no Referer, no redirects, public
//!   addresses only ([`PublicOnlyResolver`]), short timeouts.
//! - mailto: [`mailto_draft`] is the message the app sends from the
//!   receiving account, like any other send; only for an authenticated
//!   sender.
//! - Link: the app opens it in the browser.
//!
//! Logs name the domain, never the URL (it carries a per-recipient token).

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use penguin_core::unsubscribe::{self as plan, Mailto, UnsubscribePlan};
use penguin_core::{Address, Message};
use penguin_render::unsubscribe::find_unsubscribe_link;
use penguin_render::{render_html, render_text, RenderOptions};

use crate::compose::Draft;

/// The plan for a stored message. The body is only rendered (to look for a
/// link) when the headers give nothing.
pub fn plan_for(m: &Message) -> Option<UnsubscribePlan> {
    plan::plan(m, || {
        let rendered = match m.body_html.as_deref() {
            Some(html) if !html.trim().is_empty() => render_html(html, &RenderOptions::default()),
            _ => render_text(&m.body_text),
        };
        find_unsubscribe_link(&rendered.html)
    })
}

/// Whether a `List-Unsubscribe-Post` header (fetched for a message stored
/// without it) says one-click.
pub fn headers_say_one_click(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("List-Unsubscribe-Post") && plan::is_one_click(v))
}

/// The unsubscribe message for a `mailto:` plan, from `account_id`.
pub fn mailto_draft(account_id: &str, mailto: &Mailto) -> Draft {
    Draft {
        request_read_receipt: None,
        account_id: account_id.to_string(),
        to: vec![Address {
            name: None,
            email: mailto.to.clone(),
        }],
        cc: vec![],
        bcc: vec![],
        subject: mailto.subject.clone(),
        body_text: mailto.body.clone(),
        body_html: None,
        reply_to_thread_id: None,
        reply_to_message_id: None,
        attachments: vec![],
    }
}

/// Whether an address is fine to connect to from a user's device on behalf
/// of a stranger's DNS record or URL.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (18..20).contains(&o[1]))
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0] == 0x64 && s[1] == 0xff9b)) // NAT64 embeds v4 we can't vet
        }
    }
}

/// An https URL whose host is a DNS name (not an IP literal, not
/// localhost), on port 443, without userinfo.
pub fn acceptable_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && matches!(url.host(), Some(url::Host::Domain(d)) if d.contains('.') && !d.ends_with(".localhost") && !d.ends_with(".local"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none_or(|p| p == 443)
}

/// A resolver that drops loopback, private, link-local and other
/// non-public addresses, so a sender's DNS can't point a request at the
/// local network (DNS rebinding included: the check is on what resolved).
pub struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 443))
                .await?
                .filter(|a| is_public_ip(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err("no public address".into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The one-click client: HTTPS only, no redirects at all (RFC 8058: the
/// POST must not be redirected; following one could land on a private
/// address or turn into a GET), public addresses only, no cookie store,
/// no Referer.
fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .user_agent("Penguin")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .dns_resolver(Arc::new(PublicOnlyResolver))
            .build()
            .expect("reqwest client with rustls builds")
    })
}

/// A one-click URL we're willing to POST to: https, a DNS host (no IP
/// literal, localhost or .local), port 443, no userinfo.
pub fn one_click_target(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "not a valid URL".to_string())?;
    if acceptable_url(&parsed) {
        Ok(parsed)
    } else {
        Err("refused: not a public https address".to_string())
    }
}

/// RFC 8058 one-click: POST `List-Unsubscribe=One-Click`. The error is
/// worded for the user and never carries the URL.
pub async fn one_click_post(url: &str) -> Result<(), String> {
    let target = one_click_target(url)?;
    let resp = http()
        .post(target)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body("List-Unsubscribe=One-Click")
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("HTTP {}", status.as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use penguin_core::unsubscribe::UnsubscribeMethod;

    #[test]
    fn one_click_targets_must_be_public_https_names() {
        assert!(one_click_target("https://list.example/u?t=abc").is_ok());
        for bad in [
            "http://list.example/u",
            "https://127.0.0.1/u",
            "https://10.0.0.8/u",
            "https://192.168.1.1/u",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/u",
            "https://[fd00::1]/u",
            "https://localhost/u",
            "https://tauri.localhost/u",
            "https://router.local/u",
            "https://intranet/u",
            "https://list.example:8443/u",
            "https://user:pw@list.example/u",
            "javascript:alert(1)",
            "data:text/plain,x",
            "not a url",
        ] {
            assert!(one_click_target(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolver_drops_private_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.0.10",
            "100.64.0.1",
            "::1",
            "fe80::1",
            "fd12::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public_ip("93.184.216.34".parse().unwrap()));
    }

    /// DNS rebinding: a name that passes the URL check but resolves only to
    /// private addresses is refused by the client's resolver, before any
    /// connection. `localhost` resolves to loopback without the network.
    #[tokio::test]
    async fn resolver_refuses_names_that_resolve_to_private_addresses() {
        use reqwest::dns::Resolve;
        let name: reqwest::dns::Name = "localhost".parse().unwrap();
        assert!(PublicOnlyResolver.resolve(name).await.is_err());
    }

    fn message() -> Message {
        Message {
            account_id: "me@example.com".into(),
            id: "m1".into(),
            thread_id: "t1".into(),
            date: 0,
            from: Address {
                name: None,
                email: "hello@shop.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: "Sale".into(),
            snippet: String::new(),
            body_text: String::new(),
            body_html: Some(
                r#"<p>Sale!</p><p><a href="https://shop.example/unsub?u=1&amp;k=2">Unsubscribe</a></p>"#
                    .into(),
            ),
            label_ids: vec!["INBOX".into()],
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: true,
        }
    }

    #[test]
    fn body_link_is_found_through_the_renderer() {
        let p = plan_for(&message()).unwrap();
        assert_eq!(p.offer.method, UnsubscribeMethod::Link);
        assert_eq!(p.url.as_deref(), Some("https://shop.example/unsub?u=1&k=2"));
    }

    #[test]
    fn mailto_plans_become_a_plain_message_from_the_account() {
        let mut m = message();
        m.list_unsubscribe = Some("<mailto:leave@list.example?subject=unsubscribe>".into());
        let p = plan_for(&m).unwrap();
        assert_eq!(p.offer.method, UnsubscribeMethod::Mailto);
        let d = mailto_draft("me@example.com", p.offer.mailto.as_ref().unwrap());
        assert_eq!(d.to[0].email, "leave@list.example");
        assert_eq!(d.subject, "unsubscribe");
        assert_eq!(d.account_id, "me@example.com");
        assert!(headers_say_one_click(&[(
            "list-unsubscribe-post".into(),
            "List-Unsubscribe=One-Click".into()
        )]));
        assert!(!headers_say_one_click(&[("X".into(), "y".into())]));
    }
}
