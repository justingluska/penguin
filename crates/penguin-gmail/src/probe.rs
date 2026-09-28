//! Quota-cost probe (diagnostics tool, run via `penguin-cli probe-cost`).
//! OWNER: gmail-sync agent.
//!
//! Gmail's documented per-method costs don't match what the per-user
//! "Total Query Cost" quota actually charges, and the API doesn't report a
//! call's cost. This fetches the SAME set of recent messages once per
//! variant (format / `fields=`), each variant alone in its own wall-clock
//! minute with an idle minute between, so Cloud Monitoring's per-minute
//! `total_query_cost` for method GetMessage can be attributed to one
//! variant. Nothing is stored; only ids, counts and byte sizes are printed.
//!
//! Run it with the app quit: Monitoring aggregates per project and method,
//! so any other GetMessage traffic in those minutes contaminates the result.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::api::GmailClient;
use crate::Result;

/// `format=full` minus body data: tells whether cost tracks response size.
const NO_BODY_FIELDS: &str = "id,threadId,labelIds,snippet,internalDate,payload(partId,mimeType,filename,headers,body/size,body/attachmentId,parts(partId,mimeType,filename,headers,body/size,body/attachmentId,parts(partId,mimeType,filename,headers,body/size,body/attachmentId,parts(partId,mimeType,filename,headers,body/size,body/attachmentId))))";

/// What a variant fetches.
#[derive(Clone, Copy)]
enum Target {
    /// messages.get?format=…[&fields=…] per message id.
    Message(&'static str, Option<&'static str>),
    /// threads.get?format=… per distinct thread of those ids (Monitoring
    /// method GetThread); cost per message = cost ÷ `messages` below.
    Thread(&'static str),
}

const VARIANTS: &[(&str, Target)] = &[
    ("full", Target::Message("full", None)),
    ("metadata", Target::Message("metadata", None)),
    ("minimal", Target::Message("minimal", None)),
    (
        "full-no-body-data",
        Target::Message("full", Some(NO_BODY_FIELDS)),
    ),
    ("raw", Target::Message("raw", None)),
    ("thread-full", Target::Thread("full")),
    ("thread-metadata", Target::Thread("metadata")),
];

/// One variant's result.
#[derive(Debug, Clone)]
pub struct ProbeRow {
    pub variant: &'static str,
    /// UTC minute the calls ran in (unix seconds, minute-aligned).
    pub minute_start_unix: u64,
    pub calls: usize,
    pub failed: usize,
    pub bytes: u64,
    /// Messages returned (equals `calls` for message variants).
    pub messages: usize,
}

fn unix_now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

/// Sleep until `offset` seconds past the next wall-clock minute boundary.
async fn wait_for_next_minute(offset: u64) -> u64 {
    let now = unix_now().as_secs_f64();
    let next = (now / 60.0).floor() * 60.0 + 60.0;
    tokio::time::sleep(Duration::from_secs_f64(next + offset as f64 - now)).await;
    next as u64
}

/// Variant names, in run order.
pub fn variant_names() -> Vec<&'static str> {
    VARIANTS.iter().map(|(name, _)| *name).collect()
}

/// Fetch the newest `n` messages once per variant (all when `only` is empty),
/// one variant per minute. `report` is called after each variant.
pub async fn cost_probe(
    client: &GmailClient,
    n: usize,
    only: &[String],
    mut report: impl FnMut(&ProbeRow),
) -> Result<Vec<ProbeRow>> {
    let page = client.list_message_ids(None, None).await?;
    let listed: Vec<(String, String)> = page.ids.into_iter().take(n).collect();
    let ids: Vec<String> = listed.iter().map(|(id, _)| id.clone()).collect();
    let mut threads: Vec<String> = Vec::new();
    for (_, t) in &listed {
        if !threads.contains(t) {
            threads.push(t.clone());
        }
    }
    let mut rows = Vec::new();
    for &(variant, target) in VARIANTS
        .iter()
        .filter(|(v, _)| only.is_empty() || only.iter().any(|o| o == v))
    {
        // Leave the current minute (and one idle minute) clean, start 3 s in.
        wait_for_next_minute(0).await;
        let minute = wait_for_next_minute(3).await;
        let mut row = ProbeRow {
            variant,
            minute_start_unix: minute,
            calls: 0,
            failed: 0,
            bytes: 0,
            messages: 0,
        };
        let keys = match target {
            Target::Message(..) => &ids,
            Target::Thread(_) => &threads,
        };
        // Launch on a fixed schedule across ~40 s so every call lands inside
        // this minute regardless of latency.
        let gap = Duration::from_secs_f64(40.0 / keys.len().max(1) as f64);
        let results =
            futures_util::future::join_all(keys.iter().enumerate().map(|(i, key)| async move {
                tokio::time::sleep(gap * i as u32).await;
                let result = match target {
                    Target::Message(format, fields) => client
                        .probe_fetch(key, format, fields)
                        .await
                        .map(|b| (b, 1)),
                    Target::Thread(format) => client.probe_fetch_thread(key, format).await,
                };
                (key, result)
            }))
            .await;
        for (key, result) in results {
            row.calls += 1;
            match result {
                Ok((bytes, messages)) => {
                    row.bytes += bytes as u64;
                    row.messages += messages;
                }
                Err(e) => {
                    row.failed += 1;
                    tracing::warn!(id = %key, variant, error = %e, "probe fetch failed");
                }
            }
        }
        report(&row);
        rows.push(row);
    }
    Ok(rows)
}
