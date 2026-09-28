//! Unsubscribe (thread view button, message menu, ⌘U): acting on the plan
//! `penguin_core::unsubscribe` makes for a message. Every action here is a
//! user click the UI has already confirmed; nothing runs on its own.
//!
//! - One-click (RFC 8058): a POST of `List-Unsubscribe=One-Click` to the
//!   header's https URL, from Rust: no cookies, no Referer, no redirects,
//!   public addresses only (the avatar fetcher's resolver), short timeouts.
//! - mailto: the message is sent from the receiving account through Gmail,
//!   like any other send; only for an authenticated sender.
//! - Link: opened in the default browser.
//!
//! Logs name the domain, never the URL (it carries a per-recipient token).
//! Successful unsubscribes are remembered per account and sender
//! (`Store::record_unsubscribe`) so later mail shows "Unsubscribed".

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use penguin_core::unsubscribe::{
    self as core_unsub, UnsubscribeMethod, UnsubscribeOffer, UnsubscribePlan, UnsubscribeRecord,
};
use penguin_core::{Address, Message, Store};
use penguin_provider::compose::{build_rfc822_with_attachments, Draft};
use penguin_render::unsubscribe::find_unsubscribe_link;
use penguin_render::{render_html, render_text, RenderOptions};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::avatars::net::{acceptable_url, PublicOnlyResolver};
use crate::error::{CmdError, CmdResult, ErrorCode};
use crate::ops;
use crate::state::{blocking, AppState};
use crate::views::MessageView;

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Result of `unsubscribe`, for the UI's toast.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeOutcome {
    pub method: UnsubscribeMethod,
    pub domain: String,
    /// The sender now remembered as unsubscribed.
    pub sender: String,
    /// Where the mailto message went (method `mailto`).
    pub sent_to: Option<String>,
    pub record: UnsubscribeRecord,
}

/// The plan for a stored message. The body is only rendered (to look for a
/// link) when the headers give nothing.
fn plan_for(m: &Message) -> Option<UnsubscribePlan> {
    core_unsub::plan(m, || {
        let rendered = match m.body_html.as_deref() {
            Some(html) if !html.trim().is_empty() => render_html(html, &RenderOptions::default()),
            _ => render_text(&m.body_text),
        };
        find_unsubscribe_link(&rendered.html)
    })
}

/// Fill in "already unsubscribed" for rendered messages (get_thread,
/// load_remote_images). One indexed lookup per message with an offer.
pub fn annotate(store: &Store, views: &mut [MessageView]) -> penguin_core::Result<()> {
    for v in views {
        if let Some(offer) = v.unsubscribe.as_mut() {
            offer.unsubscribed = store.unsubscribe_record(&v.account_id, &v.from.email)?;
        }
    }
    Ok(())
}

async fn load(state: &AppState, account_id: &str, message_id: &str) -> CmdResult<Message> {
    let (store, a, m) = (
        state.store.clone(),
        account_id.to_string(),
        message_id.to_string(),
    );
    blocking(move || Ok(store.get_message(&a, &m)?))
        .await?
        .ok_or_else(|| CmdError::not_found("message not found"))
}

/// Keep a fetched `List-Unsubscribe-Post` for a message stored without it
/// (get_message_details and unsubscribe_check fetch every header anyway).
/// Returns the flag.
pub async fn remember_post_flag(
    state: &AppState,
    m: &Message,
    headers: &[(String, String)],
) -> CmdResult<bool> {
    let one_click = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("List-Unsubscribe-Post") && core_unsub::is_one_click(v)
    });
    if m.list_unsubscribe_post.is_none() {
        let (store, a, id) = (state.store.clone(), m.account_id.clone(), m.id.clone());
        blocking(move || Ok(store.set_list_unsubscribe_post(&a, &id, one_click)?)).await?;
    }
    Ok(one_click)
}

/// The offer for a message, first learning whether one-click is available
/// when it was stored before Penguin kept that header (`needsCheck`): one
/// format=metadata fetch at interactive priority (~20 units), stored so it
/// is never fetched again.
#[tauri::command]
pub async fn unsubscribe_check(
    state: AppStateRef<'_>,
    account_id: String,
    message_id: String,
) -> CmdResult<Option<UnsubscribeOffer>> {
    let mut m = load(&state, &account_id, &message_id).await?;
    let needs_check = plan_for(&m).is_some_and(|p| p.offer.needs_check);
    if needs_check {
        let provider = state.provider(&account_id).await?;
        let meta = provider
            .get_message_metadata(&message_id)
            .await?
            .ok_or_else(|| {
                CmdError::not_found(format!(
                    "{} no longer has this message",
                    crate::commands::cap_first(provider.provider().service_name())
                ))
            })?;
        m.list_unsubscribe_post = Some(remember_post_flag(&state, &m, &meta.headers).await?);
    }
    let Some(plan) = plan_for(&m) else {
        return Ok(None);
    };
    let mut offer = plan.offer;
    let store = state.store.clone();
    let sender = m.from.email.clone();
    offer.unsubscribed =
        blocking(move || Ok(store.unsubscribe_record(&account_id, &sender)?)).await?;
    Ok(Some(offer))
}

/// Unsubscribe with `method`, which must be what the UI confirmed: the
/// message's own plan (re-derived here; the UI never supplies a URL), or
/// `link` to open the page of a one-click plan instead.
#[tauri::command]
pub async fn unsubscribe(
    app: AppHandle,
    state: AppStateRef<'_>,
    account_id: String,
    message_id: String,
    method: UnsubscribeMethod,
) -> CmdResult<UnsubscribeOutcome> {
    let m = load(&state, &account_id, &message_id).await?;
    let plan =
        plan_for(&m).ok_or_else(|| CmdError::invalid("This message has no unsubscribe option"))?;
    let allowed =
        method == plan.offer.method || (method == UnsubscribeMethod::Link && plan.url.is_some());
    if !allowed {
        return Err(CmdError::invalid(
            "This message's unsubscribe option changed. Try again.",
        ));
    }
    let domain = plan.offer.domain.clone();
    let mut sent_to = None;
    match method {
        UnsubscribeMethod::OneClick => {
            let url = plan
                .url
                .as_deref()
                .ok_or_else(|| CmdError::other("no unsubscribe URL"))?;
            one_click_post(url).await.map_err(|e| {
                tracing::warn!(account = %account_id, %domain, error = %e, "one-click unsubscribe failed");
                CmdError::new(ErrorCode::Network, format!("{domain} didn't accept the unsubscribe ({e})"))
            })?;
        }
        UnsubscribeMethod::Mailto => {
            if !plan.offer.verified {
                return Err(CmdError::invalid(
                    "Couldn't verify this sender, so Penguin won't send mail for it. Use Edit first.",
                ));
            }
            let mailto = plan
                .offer
                .mailto
                .clone()
                .ok_or_else(|| CmdError::other("no unsubscribe address"))?;
            send_mailto(&state, &account_id, &mailto).await?;
            sent_to = Some(mailto.to);
        }
        UnsubscribeMethod::Link => {
            let url = plan
                .url
                .as_deref()
                .ok_or_else(|| CmdError::other("no unsubscribe URL"))?;
            let parsed = crate::commands::check_external_url(url)?;
            app.opener()
                .open_url(parsed.as_str(), None::<&str>)
                .map_err(|e| CmdError::other(e.to_string()))?;
        }
    }
    let record = UnsubscribeRecord {
        at: now_ms(),
        method,
    };
    let (store, a, sender) = (
        state.store.clone(),
        account_id.clone(),
        m.from.email.clone(),
    );
    blocking(move || Ok(store.record_unsubscribe(&a, &sender, method, record.at)?)).await?;
    tracing::info!(account = %account_id, %domain, method = method.as_str(), "unsubscribed");
    Ok(UnsubscribeOutcome {
        method,
        domain,
        sender: m.from.email.to_lowercase(),
        sent_to,
        record,
    })
}

async fn send_mailto(
    state: &AppState,
    account_id: &str,
    mailto: &core_unsub::Mailto,
) -> CmdResult<()> {
    let account = state.account(account_id).await?;
    let draft = Draft {
        request_read_receipt: None,
        account_id: account.id.clone(),
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
    };
    let raw = build_rfc822_with_attachments(&draft, &ops::from_address(&account), None, &[], &[])?;
    let provider = state.provider(&account.id).await?;
    provider.send_raw(&raw, None).await?;
    // The sent copy arrives through sync.
    state.poke(&account.id);
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
fn one_click_target(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "not a valid URL".to_string())?;
    if acceptable_url(&parsed) {
        Ok(parsed)
    } else {
        Err("refused: not a public https address".to_string())
    }
}

async fn one_click_post(url: &str) -> Result<(), String> {
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
    use crate::avatars::net::is_public_ip;

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
        // Names that resolve to these are refused by PublicOnlyResolver.
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

    #[test]
    fn body_link_is_found_through_the_renderer() {
        let m = Message {
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
        };
        let p = plan_for(&m).unwrap();
        assert_eq!(p.offer.method, UnsubscribeMethod::Link);
        assert_eq!(p.url.as_deref(), Some("https://shop.example/unsub?u=1&k=2"));
    }
}
