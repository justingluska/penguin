//! Read receipts that come back (RFC 8098 MDNs) and the "Read by …" line on
//! the message they are about. Asking for them is `Draft::request_read_receipt`
//! (penguin-provider compose), set from Settings → Privacy by send_message.
//! Decisions and sources: docs/PRIVACY.md.
//!
//! The scanner reads each new email's `message/disposition-notification`
//! part (a few hundred bytes, fetched once through the attachment cache),
//! stores what it says, and refreshes the sent message's thread. Receipts
//! arrive only with mail, so it sleeps until local mail changes, like the
//! invitation scanner. Nothing is ever sent from here.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use penguin_core::receipts::{parse_mdn, MAX_MDN_BYTES};
use penguin_core::{Message, Store};

use crate::error::ErrorCode;
use crate::state::{blocking, AppState};
use crate::views::MessageView;

/// Receipt parts read per round.
const SCAN_BATCH: usize = 25;
const SCAN_EVERY: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_secs(2);

/// The ids receipts are matched by: each sent message's Message-ID.
pub fn sent_ids(messages: &[Message]) -> Vec<(String, String)> {
    messages
        .iter()
        .filter(|m| m.label_ids.iter().any(|l| l == "SENT"))
        .filter_map(|m| {
            let id = m.message_id_header.as_deref()?.trim();
            let id = id.trim_start_matches('<').trim_end_matches('>').trim();
            (!id.is_empty()).then(|| (m.id.clone(), id.to_string()))
        })
        .collect()
}

/// Fill `read_receipts` on the views of sent messages (`ids` from `sent_ids`).
pub fn annotate(
    store: &Store,
    account_id: &str,
    ids: &[(String, String)],
    views: &mut [MessageView],
) -> penguin_core::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let headers: Vec<String> = ids.iter().map(|(_, h)| h.clone()).collect();
    let found = store.receipts_for(account_id, &headers)?;
    if found.is_empty() {
        return Ok(());
    }
    let by_message: HashMap<&str, &str> = ids.iter().map(|(m, h)| (m.as_str(), h.as_str())).collect();
    for v in views.iter_mut() {
        if let Some(h) = by_message.get(v.id.as_str()) {
            v.read_receipts = found.iter().filter(|r| r.original_message_id == *h).cloned().collect();
        }
    }
    Ok(())
}

/// Start the scanner (once, at startup, after `Store::migrate_receipts`).
pub fn spawn_scanner(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut changes = state.mail_changes();
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            changes.borrow_and_update();
            if scan_round(&state).await == Round::More {
                tokio::time::sleep(SCAN_EVERY).await;
                continue;
            }
            if changes.changed().await.is_err() {
                return;
            }
            tokio::time::sleep(SETTLE).await;
        }
    });
}

#[derive(PartialEq, Eq)]
enum Round {
    Done,
    More,
}

async fn scan_round(state: &Arc<AppState>) -> Round {
    let store = state.store.clone();
    let Ok(candidates) = blocking(move || Ok(store.receipt_scan_candidates(SCAN_BATCH)?)).await else {
        return Round::More;
    };
    let mut round = if candidates.len() >= SCAN_BATCH { Round::More } else { Round::Done };
    let mut touched: HashMap<String, Vec<String>> = HashMap::new();
    for c in candidates {
        let Ok(provider) = state.provider(&c.account_id).await else {
            continue;
        };
        let mdn = match crate::attachments::bytes(&state.paths, provider.as_ref(), &c.account_id, &c.message_id, &c.part).await {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_MDN_BYTES)]).into_owned();
                let parsed = parse_mdn(&text);
                if parsed.is_none() {
                    // Sizes only: never the content.
                    tracing::warn!(account = %c.account_id, message = %c.message_id, bytes = bytes.len(), "receipts: part didn't parse");
                }
                parsed
            }
            Err(e) if e.code == ErrorCode::Network => {
                tracing::debug!(account = %c.account_id, error = %e.message, "receipts: scan paused");
                round = Round::More;
                break;
            }
            Err(e) if e.code == ErrorCode::NeedsReauth => continue,
            Err(e) => {
                tracing::warn!(account = %c.account_id, message = %c.message_id, code = ?e.code, "receipts: part unavailable");
                None
            }
        };
        let store = state.store.clone();
        let (acct, msg, date) = (c.account_id.clone(), c.message_id.clone(), c.date);
        let thread = blocking(move || Ok(store.put_receipt(&acct, &msg, date, mdn.as_ref())?)).await;
        if let Ok(Some(thread)) = thread {
            touched.entry(c.account_id).or_default().push(thread);
        }
    }
    for (account, threads) in touched {
        state.emit_mail_changed(&account, threads);
    }
    round
}

#[cfg(test)]
mod tests {
    use super::*;
    use penguin_core::Address;

    fn m(id: &str, labels: &[&str], mid: Option<&str>) -> Message {
        Message {
            account_id: "sam@mail.example".into(),
            id: id.into(),
            thread_id: "t1".into(),
            date: 0,
            from: Address { name: None, email: "sam@mail.example".into() },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: String::new(),
            snippet: String::new(),
            body_text: String::new(),
            body_html: None,
            label_ids: labels.iter().map(|s| s.to_string()).collect(),
            attachments: vec![],
            message_id_header: mid.map(str::to_string),
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: false,
        }
    }

    #[test]
    fn only_sent_messages_with_an_id_are_matched() {
        let ids = sent_ids(&[
            m("a", &["SENT"], Some("<a1@mail.example>")),
            m("b", &["INBOX"], Some("b1@mail.example")),
            m("c", &["SENT"], None),
            m("d", &["SENT"], Some("  ")),
        ]);
        assert_eq!(ids, vec![("a".to_string(), "a1@mail.example".to_string())]);
    }
}
