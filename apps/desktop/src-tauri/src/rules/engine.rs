//! The rules engine, Tauri-free: which rules match which messages, and
//! running their actions through an [`Effects`] implementation (the app's,
//! or a fake in tests).
//!
//! Matching is `Store::match_query` — the search language, restricted to
//! the messages an event names — so a condition behaves exactly like the
//! same text typed into search. Idempotency is the store's
//! `rule_applications` claim: a live rule never acts twice on a message.
//! Dry-run rules only log (hooks and webhooks don't run either).

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::Path;
use std::time::Duration;

use penguin_core::store::{QueuedRuleEvent, RuleApplication, RuleEventKind, RuleLogEntry};
use penguin_core::{Label, Message, Store};
use serde::{Deserialize, Serialize};

use super::hooks::{self, RuleRef};
use super::model::{self, Action, Rule, RulesConfig, Trigger};
use crate::settings::Profile;
use crate::views::{ThreadAction, ThreadRef};

/// Queue rows handled per pass.
pub const EVENT_BATCH: usize = 200;
/// A scheduled or manual run acts on at most this many messages.
pub const MAX_RUN_MESSAGES: usize = 1000;
/// Circuit breaker: a sync-triggered rule acting on more messages than this
/// within an hour is paused (a condition that matches everything).
pub const MAX_RULE_MESSAGES_PER_HOUR: usize = 500;
pub const MAX_FORWARDS_PER_HOUR: usize = 20;
pub const MAX_FORWARDS_PER_DAY: usize = 100;
/// Thread changes rules may push to Gmail per minute (all rules together;
/// they share the quota with sync and your own clicks). The runtime waits
/// between passes rather than drop work.
pub const MAX_GMAIL_CHANGES_PER_MINUTE: usize = 240;
/// A label a rule added comes back from Gmail as a history labelsAdded;
/// for this long it doesn't count as a label-added trigger.
const ECHO_TTL_MS: i64 = 15 * 60 * 1000;
const MINUTE_MS: i64 = 60_000;
/// More matches than this in one pass collapse into one notification.
const MAX_SEPARATE_NOTIFICATIONS: usize = 3;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// Side effects the engine needs. Errors are user-facing strings, logged
/// in the rule's history.
pub trait Effects: Sync {
    /// The app's optimistic modify path (local now, Gmail in the background).
    fn modify(
        &self,
        targets: Vec<ThreadRef>,
        action: ThreadAction,
    ) -> impl Future<Output = Result<(), String>> + Send;
    fn forward(
        &self,
        message: &Message,
        to: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;
    fn notify(&self, title: &str, body: &str) -> Result<(), String>;
    fn hook(
        &self,
        program: &Path,
        args: &[String],
        env: Vec<(String, String)>,
        stdin: Vec<u8>,
        timeout: Duration,
    ) -> impl Future<Output = Result<String, String>> + Send;
    fn webhook(
        &self,
        url: &str,
        secret: Option<&str>,
        body: Vec<u8>,
    ) -> impl Future<Output = Result<String, String>> + Send;
}

/// What one action did for one message (stored as JSON in the rule log).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub action: String,
    pub ok: bool,
    pub detail: String,
    /// The inverse thread action, for "Undo" in the history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<ThreadAction>,
}

impl Outcome {
    fn ok(action: &Action, detail: impl Into<String>) -> Outcome {
        Outcome {
            action: action.name().into(),
            ok: true,
            detail: detail.into(),
            undo: None,
        }
    }
    fn err(action: &Action, detail: impl Into<String>) -> Outcome {
        Outcome {
            action: action.name().into(),
            ok: false,
            detail: detail.into(),
            undo: None,
        }
    }
}

/// Rate state kept across passes (in memory; resets with the app).
#[derive(Default)]
pub struct Limits {
    per_rule: HashMap<String, VecDeque<i64>>,
    forwards: VecDeque<i64>,
    /// Timestamps of thread changes pushed to Gmail (one per thread).
    gmail: VecDeque<i64>,
    /// (account, thread, label id) → when a rule added it.
    echoes: HashMap<(String, String, String), i64>,
}

impl Limits {
    /// Record `n` acted-on messages for `rule_id`; false once the hourly
    /// budget is exceeded.
    fn charge_rule(&mut self, rule_id: &str, n: usize, now: i64) -> bool {
        let q = self.per_rule.entry(rule_id.to_string()).or_default();
        while q.front().is_some_and(|t| *t <= now - HOUR_MS) {
            q.pop_front();
        }
        q.extend(std::iter::repeat_n(now, n));
        q.len() <= MAX_RULE_MESSAGES_PER_HOUR
    }

    fn charge_gmail(&mut self, n: usize, now: i64) {
        self.gmail.extend(std::iter::repeat_n(now, n));
    }

    /// How long to wait before the next pass so rules stay under
    /// MAX_GMAIL_CHANGES_PER_MINUTE (0 = go ahead).
    pub fn gmail_wait_ms(&mut self, now: i64) -> i64 {
        while self.gmail.front().is_some_and(|t| *t <= now - MINUTE_MS) {
            self.gmail.pop_front();
        }
        if self.gmail.len() < MAX_GMAIL_CHANGES_PER_MINUTE {
            return 0;
        }
        let excess = self.gmail.len() - MAX_GMAIL_CHANGES_PER_MINUTE;
        self.gmail
            .get(excess)
            .map_or(0, |t| (t + MINUTE_MS - now).max(0))
    }

    fn record_echo(&mut self, account: &str, thread: &str, label: &str, now: i64) {
        self.echoes.retain(|_, t| *t > now - ECHO_TTL_MS);
        self.echoes.insert(
            (account.to_string(), thread.to_string(), label.to_string()),
            now,
        );
    }

    /// A label a rule itself added (recently) on this thread.
    fn is_echo(&self, account: &str, thread: &str, label: &str, now: i64) -> bool {
        self.echoes
            .get(&(account.to_string(), thread.to_string(), label.to_string()))
            .is_some_and(|t| *t > now - ECHO_TTL_MS)
    }

    /// Take one forward from the hourly and daily budgets.
    fn take_forward(&mut self, now: i64) -> Result<(), String> {
        while self.forwards.front().is_some_and(|t| *t <= now - DAY_MS) {
            self.forwards.pop_front();
        }
        let last_hour = self.forwards.iter().filter(|t| **t > now - HOUR_MS).count();
        if last_hour >= MAX_FORWARDS_PER_HOUR {
            return Err(format!(
                "skipped: over {MAX_FORWARDS_PER_HOUR} forwards in the last hour"
            ));
        }
        if self.forwards.len() >= MAX_FORWARDS_PER_DAY {
            return Err(format!(
                "skipped: over {MAX_FORWARDS_PER_DAY} forwards in the last day"
            ));
        }
        self.forwards.push_back(now);
        Ok(())
    }
}

pub struct Ctx<'a> {
    pub store: &'a Store,
    pub config: &'a RulesConfig,
    pub profiles: &'a [Profile],
    pub now: i64,
}

/// A rule that ran in a pass, for the `penguin://rule-fired` toast.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Fired {
    pub rule_id: String,
    pub rule_name: String,
    pub trigger: String,
    pub dry_run: bool,
    /// Messages matched (and acted on, unless dry-run).
    pub count: usize,
    pub failed: usize,
    /// History rows written (Undo acts on these).
    pub log_ids: Vec<i64>,
}

#[derive(Debug, Default)]
pub struct PassReport {
    pub fired: Vec<Fired>,
    /// Rules to switch off: (rule id, reason).
    pub pause: Vec<(String, String)>,
    /// Queue rows consumed.
    pub handled: usize,
}

type Key = (String, String);

/// Mail someone sent you: not a draft, and not your own sent copy (a
/// message to yourself also lands in INBOX and counts).
fn incoming(m: &Message) -> bool {
    let has = |l: &str| m.label_ids.iter().any(|x| x == l);
    !has("DRAFT") && (!has("SENT") || has("INBOX"))
}

/// The accounts a rule watches (None = all). A missing profile watches
/// nothing.
pub fn rule_scope(rule: &Rule, profiles: &[Profile]) -> Option<Vec<String>> {
    let from_profile = rule.profile_id.as_ref().map(|id| {
        profiles
            .iter()
            .find(|p| &p.id == id)
            .map(|p| p.account_ids.clone())
            .unwrap_or_default()
    });
    match (from_profile, &rule.account_ids) {
        (None, None) => None,
        (Some(p), None) => Some(p),
        (None, Some(a)) => Some(a.clone()),
        (Some(p), Some(a)) => Some(p.into_iter().filter(|x| a.contains(x)).collect()),
    }
}

fn in_scope(scope: &Option<Vec<String>>, account: &str) -> bool {
    scope
        .as_ref()
        .is_none_or(|s| s.iter().any(|a| a == account))
}

/// Per-account label lookup by id or (case-insensitive) name.
struct LabelCache<'a> {
    store: &'a Store,
    by_account: HashMap<String, Vec<Label>>,
}

impl<'a> LabelCache<'a> {
    fn new(store: &'a Store) -> Self {
        LabelCache {
            store,
            by_account: HashMap::new(),
        }
    }
    fn resolve(&mut self, account: &str, wanted: &str) -> Option<String> {
        let labels = self
            .by_account
            .entry(account.to_string())
            .or_insert_with(|| self.store.list_labels(Some(account)).unwrap_or_default());
        labels
            .iter()
            .find(|l| l.id.eq_ignore_ascii_case(wanted))
            .or_else(|| labels.iter().find(|l| l.name.eq_ignore_ascii_case(wanted)))
            .map(|l| l.id.clone())
            // System labels may not be in the stored list.
            .or_else(|| {
                const SYSTEM: &[&str] = &[
                    "INBOX",
                    "STARRED",
                    "IMPORTANT",
                    "UNREAD",
                    "SENT",
                    "TRASH",
                    "SPAM",
                ];
                SYSTEM
                    .iter()
                    .find(|s| s.eq_ignore_ascii_case(wanted))
                    .map(|s| s.to_string())
            })
    }
}

fn trigger_name(t: &Trigger) -> &'static str {
    match t {
        Trigger::NewMessage => "newMessage",
        Trigger::LabelAdded { .. } => "labelAdded",
        Trigger::Schedule { .. } => "schedule",
        Trigger::Manual => "manual",
    }
}

/// Handle a batch of queued sync events: match, claim, act, log.
pub async fn process_events<E: Effects>(
    ctx: &Ctx<'_>,
    fx: &E,
    limits: &mut Limits,
    events: &[QueuedRuleEvent],
) -> penguin_core::Result<PassReport> {
    let mut report = PassReport {
        handled: events.len(),
        ..Default::default()
    };
    let queue_ids: Vec<i64> = events.iter().map(|e| e.id).collect();
    let live_rules: Vec<&Rule> = ctx
        .config
        .rules
        .iter()
        .filter(|r| {
            r.enabled && matches!(r.trigger, Trigger::NewMessage | Trigger::LabelAdded { .. })
        })
        .collect();
    if live_rules.is_empty() {
        ctx.store
            .claim_rule_applications(&queue_ids, &[], ctx.now)?;
        return Ok(report);
    }

    let mut messages: HashMap<Key, Message> = HashMap::new();
    for e in events {
        let k = (e.account_id.clone(), e.message_id.clone());
        if messages.contains_key(&k) {
            continue;
        }
        if let Some(m) = ctx.store.get_message(&e.account_id, &e.message_id)? {
            messages.insert(k, m);
        }
    }

    let mut labels = LabelCache::new(ctx.store);
    let mut stopped: HashSet<Key> = HashSet::new();
    let mut plans: Vec<(&Rule, Vec<Key>)> = Vec::new();
    for rule in live_rules {
        let scope = rule_scope(rule, ctx.profiles);
        let mut candidates: Vec<Key> = Vec::new();
        for e in events {
            let k = (e.account_id.clone(), e.message_id.clone());
            let Some(m) = messages.get(&k) else { continue };
            if stopped.contains(&k) || !in_scope(&scope, &e.account_id) {
                continue;
            }
            let fits = match &rule.trigger {
                Trigger::NewMessage => e.kind == RuleEventKind::NewMessage && incoming(m),
                Trigger::LabelAdded { label } => match labels.resolve(&e.account_id, label) {
                    None => false,
                    // Still carrying it (a page may add then remove), and not
                    // the echo of a rule's own action (no rule loops).
                    Some(id) => match e.kind {
                        RuleEventKind::LabelAdded => {
                            e.labels.contains(&id)
                                && m.label_ids.contains(&id)
                                && !limits.is_echo(&e.account_id, &m.thread_id, &id, ctx.now)
                        }
                        // New mail that arrives already carrying the label.
                        RuleEventKind::NewMessage => m.label_ids.contains(&id),
                    },
                },
                _ => false,
            };
            if fits {
                candidates.push(k);
            }
        }
        let mut uniq = HashSet::new();
        candidates.retain(|k| uniq.insert(k.clone()));
        if candidates.is_empty() {
            continue;
        }
        let matched = match ctx.store.match_query(
            &rule.condition,
            scope.as_deref(),
            Some(&candidates),
            candidates.len(),
        ) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(rule = %rule.id, error = %e, "rule condition failed");
                continue;
            }
        };
        let keys: Vec<Key> = matched
            .into_iter()
            .map(|m| (m.account_id, m.message_id))
            .collect();
        if rule.stop_processing {
            stopped.extend(keys.iter().cloned());
        }
        if !keys.is_empty() {
            plans.push((rule, keys));
        }
    }

    // Claim every live (rule, message) and drop the queue rows, atomically.
    let claims: Vec<RuleApplication> = plans
        .iter()
        .filter(|(r, _)| !r.dry_run)
        .flat_map(|(r, keys)| {
            keys.iter().map(|(a, m)| RuleApplication {
                rule_id: r.id.clone(),
                account_id: a.clone(),
                message_id: m.clone(),
                thread_id: messages[&(a.clone(), m.clone())].thread_id.clone(),
            })
        })
        .collect();
    let fresh = ctx
        .store
        .claim_rule_applications(&queue_ids, &claims, ctx.now)?;
    let fresh: HashSet<(String, Key)> = claims
        .iter()
        .zip(fresh)
        .filter(|(_, new)| *new)
        .map(|(c, _)| {
            (
                c.rule_id.clone(),
                (c.account_id.clone(), c.message_id.clone()),
            )
        })
        .collect();

    for (rule, keys) in plans {
        let keys: Vec<Key> = if rule.dry_run {
            keys
        } else {
            keys.into_iter()
                .filter(|k| fresh.contains(&(rule.id.clone(), k.clone())))
                .collect()
        };
        if keys.is_empty() {
            continue;
        }
        if !rule.dry_run && !limits.charge_rule(&rule.id, keys.len(), ctx.now) {
            let reason = format!(
                "Paused: acted on more than {MAX_RULE_MESSAGES_PER_HOUR} messages in an hour. Check the condition, then turn it back on."
            );
            let entry = RuleLogEntry {
                id: 0,
                ts: ctx.now,
                rule_id: rule.id.clone(),
                trigger: trigger_name(&rule.trigger).into(),
                dry_run: false,
                ok: false,
                account_id: None,
                thread_id: None,
                message_id: None,
                subject: None,
                from_email: None,
                matched: 0,
                outcomes: serde_json::json!([{"action": "pause", "ok": false, "detail": reason}]),
                undone: false,
            };
            ctx.store.add_rule_log(&[entry])?;
            mark_failed(ctx, rule, &keys)?;
            report.pause.push((rule.id.clone(), reason));
            continue;
        }
        let msgs: Vec<&Message> = keys.iter().filter_map(|k| messages.get(k)).collect();
        let fired = execute(
            ctx,
            fx,
            limits,
            &mut labels,
            rule,
            trigger_name(&rule.trigger),
            rule.dry_run,
            &msgs,
            true,
        )
        .await?;
        report.fired.push(fired);
    }
    Ok(report)
}

fn mark_failed(ctx: &Ctx<'_>, rule: &Rule, keys: &[Key]) -> penguin_core::Result<()> {
    use penguin_core::store::ApplicationStatus as S;
    let mut by_account: HashMap<&str, Vec<String>> = HashMap::new();
    for (a, m) in keys {
        by_account.entry(a.as_str()).or_default().push(m.clone());
    }
    for (a, ids) in by_account {
        ctx.store
            .set_rule_application_status(&rule.id, a, &ids, S::Failed)?;
    }
    Ok(())
}

/// Result of a scheduled or manual run.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    pub rule_id: String,
    pub dry_run: bool,
    /// Messages matching the condition (up to MAX_RUN_MESSAGES).
    pub matched: usize,
    /// Of those, ones this rule already acted on earlier (skipped).
    pub already_applied: usize,
    /// Acted on now (or would be, in a dry run).
    pub acted: usize,
    pub failed: usize,
    pub capped: bool,
    pub log_ids: Vec<i64>,
}

/// Run a rule over existing mail (its schedule, or "Run now"). Messages the
/// rule already acted on are skipped, so a daily run only handles new
/// matches.
pub async fn run_rule<E: Effects>(
    ctx: &Ctx<'_>,
    fx: &E,
    limits: &mut Limits,
    rule: &Rule,
    trigger: &str,
    dry_run: bool,
) -> penguin_core::Result<RunReport> {
    let scope = rule_scope(rule, ctx.profiles);
    let matches = ctx.store.match_query(
        &rule.condition,
        scope.as_deref(),
        None,
        MAX_RUN_MESSAGES + 1,
    )?;
    let capped = matches.len() > MAX_RUN_MESSAGES;
    let matches = &matches[..matches.len().min(MAX_RUN_MESSAGES)];
    let pairs: Vec<Key> = matches
        .iter()
        .map(|m| (m.account_id.clone(), m.message_id.clone()))
        .collect();
    let already = ctx.store.applied_rule_messages(&rule.id, &pairs)?;
    let todo: Vec<&penguin_core::QueryMatch> = matches
        .iter()
        .filter(|m| !already.contains(&(m.account_id.clone(), m.message_id.clone())))
        .collect();
    let mut report = RunReport {
        rule_id: rule.id.clone(),
        dry_run,
        matched: matches.len(),
        already_applied: already.len(),
        acted: 0,
        failed: 0,
        capped,
        log_ids: Vec::new(),
    };
    let mut keys: Vec<Key> = todo
        .iter()
        .map(|m| (m.account_id.clone(), m.message_id.clone()))
        .collect();
    if !dry_run {
        let claims: Vec<RuleApplication> = todo
            .iter()
            .map(|m| RuleApplication {
                rule_id: rule.id.clone(),
                account_id: m.account_id.clone(),
                message_id: m.message_id.clone(),
                thread_id: m.thread_id.clone(),
            })
            .collect();
        let fresh = ctx.store.claim_rule_applications(&[], &claims, ctx.now)?;
        let mut it = fresh.into_iter();
        keys.retain(|_| it.next().unwrap_or(false));
    }
    let mut msgs = Vec::with_capacity(keys.len());
    for (a, m) in &keys {
        if let Some(msg) = ctx.store.get_message(a, m)? {
            msgs.push(msg);
        }
    }
    if msgs.is_empty() {
        return Ok(report);
    }
    let refs: Vec<&Message> = msgs.iter().collect();
    let mut labels = LabelCache::new(ctx.store);
    let fired = execute(
        ctx,
        fx,
        limits,
        &mut labels,
        rule,
        trigger,
        dry_run,
        &refs,
        false,
    )
    .await?;
    report.acted = fired.count;
    report.failed = fired.failed;
    report.log_ids = fired.log_ids;
    Ok(report)
}

fn thread_action(
    action: &Action,
    labels: &mut LabelCache<'_>,
    account: &str,
) -> Result<(ThreadAction, ThreadAction), String> {
    Ok(match action {
        Action::AddLabel { label } | Action::RemoveLabel { label } => {
            let id = labels
                .resolve(account, label)
                .ok_or_else(|| format!("no label named {label:?} in {account}"))?;
            let add = ThreadAction::AddLabel {
                label_id: id.clone(),
            };
            let remove = ThreadAction::RemoveLabel { label_id: id };
            if matches!(action, Action::AddLabel { .. }) {
                (add, remove)
            } else {
                (remove, add)
            }
        }
        Action::Archive => (ThreadAction::Archive, ThreadAction::MoveToInbox),
        Action::MarkRead => (ThreadAction::MarkRead, ThreadAction::MarkUnread),
        Action::Star => (ThreadAction::Star, ThreadAction::Unstar),
        Action::Trash { .. } => (ThreadAction::Trash, ThreadAction::Untrash),
        _ => unreachable!("not a thread action"),
    })
}

fn is_thread_action(a: &Action) -> bool {
    matches!(
        a,
        Action::AddLabel { .. }
            | Action::RemoveLabel { .. }
            | Action::Archive
            | Action::MarkRead
            | Action::Star
            | Action::Trash { .. }
    )
}

fn describe(m: &Message) -> String {
    let who = m.from.name.as_deref().unwrap_or(&m.from.email);
    if m.subject.trim().is_empty() {
        format!("From {who}")
    } else {
        format!("{who}: {}", m.subject.trim())
    }
}

/// Run `rule`'s actions on `msgs` and write its history. `per_message`:
/// notifications, hooks and webhooks fire once per message (sync
/// triggers); otherwise once for the whole run (schedule / manual).
#[allow(clippy::too_many_arguments)]
async fn execute<E: Effects>(
    ctx: &Ctx<'_>,
    fx: &E,
    limits: &mut Limits,
    labels: &mut LabelCache<'_>,
    rule: &Rule,
    trigger: &str,
    dry_run: bool,
    msgs: &[&Message],
    per_message: bool,
) -> penguin_core::Result<Fired> {
    let mut outcomes: Vec<Vec<Outcome>> = vec![Vec::new(); msgs.len()];
    let mut aggregate: Vec<Outcome> = Vec::new();
    let rule_ref = RuleRef {
        id: rule.id.clone(),
        name: rule.name.clone(),
    };

    for action in &rule.actions {
        if is_thread_action(action) {
            // One call per (account, resolved action): threads deduped.
            let mut groups: Vec<(ThreadAction, ThreadAction, Vec<usize>)> = Vec::new();
            for (i, m) in msgs.iter().enumerate() {
                match thread_action(action, labels, &m.account_id) {
                    Err(e) => outcomes[i].push(Outcome::err(action, e)),
                    Ok((act, undo)) => match groups
                        .iter_mut()
                        .find(|g| g.0 == act && msgs[g.2[0]].account_id == m.account_id)
                    {
                        Some(g) => g.2.push(i),
                        None => groups.push((act, undo, vec![i])),
                    },
                }
            }
            for (act, undo, idx) in groups {
                if dry_run {
                    for i in idx {
                        outcomes[i].push(Outcome::ok(action, format!("would {}", act.verb())));
                    }
                    continue;
                }
                let mut seen = HashSet::new();
                let targets: Vec<ThreadRef> = idx
                    .iter()
                    .filter(|i| seen.insert(msgs[**i].thread_id.clone()))
                    .map(|i| ThreadRef {
                        account_id: msgs[*i].account_id.clone(),
                        thread_id: msgs[*i].thread_id.clone(),
                    })
                    .collect();
                let threads = targets.len();
                let result = fx.modify(targets, act.clone()).await;
                if result.is_ok() {
                    limits.charge_gmail(threads, ctx.now);
                    for label in act.local_delta().0 {
                        for i in &idx {
                            limits.record_echo(
                                &msgs[*i].account_id,
                                &msgs[*i].thread_id,
                                &label,
                                ctx.now,
                            );
                        }
                    }
                }
                for i in idx {
                    outcomes[i].push(match &result {
                        Ok(()) => Outcome {
                            undo: Some(undo.clone()),
                            ..Outcome::ok(action, "")
                        },
                        Err(e) => Outcome::err(action, e.clone()),
                    });
                }
            }
            continue;
        }
        match action {
            Action::Forward { to, .. } => {
                for (i, m) in msgs.iter().enumerate() {
                    let o = if m.from.email.eq_ignore_ascii_case(to) {
                        Outcome::err(action, "skipped: the message is from the forward address")
                    } else if m.label_ids.iter().any(|l| l == "SENT") {
                        Outcome::err(action, "skipped: your own sent mail isn't forwarded")
                    } else if dry_run {
                        Outcome::ok(action, format!("would forward to {to}"))
                    } else {
                        match limits.take_forward(ctx.now) {
                            Err(e) => Outcome::err(action, e),
                            Ok(()) => match fx.forward(m, to).await {
                                Ok(d) => Outcome::ok(action, d),
                                Err(e) => Outcome::err(action, e),
                            },
                        }
                    };
                    outcomes[i].push(o);
                }
            }
            Action::Notify => {
                let separate = per_message && msgs.len() <= MAX_SEPARATE_NOTIFICATIONS;
                if dry_run {
                    let o = Outcome::ok(action, "would notify");
                    if separate {
                        outcomes.iter_mut().for_each(|v| v.push(o.clone()));
                    } else {
                        aggregate.push(o);
                    }
                } else if separate {
                    for (i, m) in msgs.iter().enumerate() {
                        let o = match fx.notify(&rule.name, &describe(m)) {
                            Ok(()) => Outcome::ok(action, ""),
                            Err(e) => Outcome::err(action, e),
                        };
                        outcomes[i].push(o);
                    }
                } else {
                    let body = match msgs {
                        [only] => describe(only),
                        [first, ..] => format!("{} messages, e.g. {}", msgs.len(), describe(first)),
                        [] => String::new(),
                    };
                    aggregate.push(match fx.notify(&rule.name, &body) {
                        Ok(()) => Outcome::ok(action, ""),
                        Err(e) => Outcome::err(action, e),
                    });
                }
            }
            Action::Hook { .. } | Action::Webhook { .. } => {
                let batches: Vec<Vec<usize>> = if per_message {
                    (0..msgs.len()).map(|i| vec![i]).collect()
                } else {
                    vec![(0..msgs.len()).collect()]
                };
                for batch in batches {
                    let o = if !ctx.config.allow_hooks {
                        Outcome::err(action, "skipped: hooks are off (Settings → Developer)")
                    } else if dry_run {
                        Outcome::ok(action, "would run")
                    } else {
                        let batch_msgs: Vec<Message> =
                            batch.iter().map(|i| msgs[*i].clone()).collect();
                        run_hook_action(fx, action, rule, &rule_ref, trigger, &batch_msgs).await
                    };
                    if per_message {
                        outcomes[batch[0]].push(o);
                    } else {
                        aggregate.push(o);
                    }
                }
            }
            _ => unreachable!("thread actions handled above"),
        }
    }

    let mut entries: Vec<RuleLogEntry> = msgs
        .iter()
        .zip(&outcomes)
        .map(|(m, o)| RuleLogEntry {
            id: 0,
            ts: ctx.now,
            rule_id: rule.id.clone(),
            trigger: trigger.to_string(),
            dry_run,
            ok: o.iter().all(|x| x.ok),
            account_id: Some(m.account_id.clone()),
            thread_id: Some(m.thread_id.clone()),
            message_id: Some(m.id.clone()),
            subject: Some(m.subject.clone()),
            from_email: Some(m.from.email.clone()),
            matched: 1,
            outcomes: serde_json::to_value(o).unwrap_or_default(),
            undone: false,
        })
        .collect();
    if !aggregate.is_empty() {
        entries.push(RuleLogEntry {
            id: 0,
            ts: ctx.now,
            rule_id: rule.id.clone(),
            trigger: trigger.to_string(),
            dry_run,
            ok: aggregate.iter().all(|x| x.ok),
            account_id: None,
            thread_id: None,
            message_id: None,
            subject: None,
            from_email: None,
            matched: 0,
            outcomes: serde_json::to_value(&aggregate).unwrap_or_default(),
            undone: false,
        });
    }
    let failed = outcomes.iter().filter(|o| o.iter().any(|x| !x.ok)).count();
    let log_ids = ctx.store.add_rule_log(&entries)?;
    if !dry_run {
        use penguin_core::store::ApplicationStatus as S;
        for (m, o) in msgs.iter().zip(&outcomes) {
            let status = if o.iter().all(|x| x.ok) {
                S::Done
            } else {
                S::Failed
            };
            ctx.store.set_rule_application_status(
                &rule.id,
                &m.account_id,
                std::slice::from_ref(&m.id),
                status,
            )?;
        }
    }
    Ok(Fired {
        rule_id: rule.id.clone(),
        rule_name: rule.name.clone(),
        trigger: trigger.to_string(),
        dry_run,
        count: msgs.len(),
        failed,
        log_ids,
    })
}

async fn run_hook_action<E: Effects>(
    fx: &E,
    action: &Action,
    rule: &Rule,
    rule_ref: &RuleRef,
    trigger: &str,
    msgs: &[Message],
) -> Outcome {
    let body = match serde_json::to_vec(&hooks::payload(
        rule_ref.clone(),
        trigger,
        rule.include_body,
        msgs,
    )) {
        Ok(b) => b,
        Err(e) => return Outcome::err(action, e.to_string()),
    };
    let result = match action {
        Action::Hook {
            program,
            args,
            timeout_secs,
        } => {
            let path = match model::resolve_program(program) {
                Ok(p) => p,
                Err(e) => return Outcome::err(action, e),
            };
            let mut vars: Vec<(&str, String)> = vec![
                ("PENGUIN_RULE_ID", rule.id.clone()),
                ("PENGUIN_TRIGGER", trigger.to_string()),
                ("PENGUIN_MESSAGE_COUNT", msgs.len().to_string()),
                (
                    "PENGUIN_SCHEMA_VERSION",
                    crate::agent::output::SCHEMA_VERSION.to_string(),
                ),
            ];
            if let [m] = msgs {
                vars.push(("PENGUIN_ACCOUNT", m.account_id.clone()));
                vars.push(("PENGUIN_THREAD_ID", m.thread_id.clone()));
                vars.push(("PENGUIN_MESSAGE_ID", m.id.clone()));
            }
            fx.hook(
                &path,
                args,
                hooks::hook_env(&vars),
                body,
                Duration::from_secs(u64::from(*timeout_secs)),
            )
            .await
        }
        Action::Webhook { url, secret } => fx.webhook(url, secret.as_deref(), body).await,
        _ => unreachable!(),
    };
    match result {
        Ok(d) => Outcome::ok(action, d),
        Err(e) => Outcome::err(action, e),
    }
}

/// A preview row for the editor ("test against recent mail").
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewItem {
    pub account_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub subject: String,
    pub from: penguin_core::Address,
    pub date: i64,
    /// The saved rule already acted on it (a run would skip it).
    pub already_applied: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RulePreview {
    /// Messages matching the condition in scope (counted up to a cap).
    pub count: u64,
    pub capped: bool,
    /// Newest matches.
    pub items: Vec<PreviewItem>,
}

pub fn preview(
    store: &Store,
    condition: &str,
    scope: Option<&[String]>,
    rule_id: Option<&str>,
    limit: usize,
    count_cap: usize,
) -> penguin_core::Result<RulePreview> {
    let (count, capped) = store.count_query_matches(condition, scope, count_cap)?;
    let matches = store.match_query(condition, scope, None, limit)?;
    let applied = match rule_id {
        Some(id) => store.applied_rule_messages(
            id,
            &matches
                .iter()
                .map(|m| (m.account_id.clone(), m.message_id.clone()))
                .collect::<Vec<_>>(),
        )?,
        None => HashSet::new(),
    };
    Ok(RulePreview {
        count,
        capped,
        items: matches
            .into_iter()
            .map(|m| PreviewItem {
                already_applied: applied.contains(&(m.account_id.clone(), m.message_id.clone())),
                account_id: m.account_id,
                thread_id: m.thread_id,
                message_id: m.message_id,
                subject: m.subject,
                from: m.from,
                date: m.date,
            })
            .collect(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use penguin_core::store::RuleEvent;
    use penguin_core::{Account, Address};
    use std::sync::Mutex;

    pub(crate) fn message(account: &str, id: &str, thread: &str) -> Message {
        Message {
            account_id: account.into(),
            id: id.into(),
            thread_id: thread.into(),
            date: 1_758_000_000_000 + id.len() as i64,
            from: Address {
                name: Some("Uber Receipts".into()),
                email: "receipts@uber.example".into(),
            },
            to: vec![Address {
                name: None,
                email: account.into(),
            }],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: format!("Your trip {id}"),
            snippet: String::new(),
            body_text: format!("Thanks for riding, total $12 ({id})"),
            body_html: None,
            label_ids: vec!["INBOX".into(), "UNREAD".into()],
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            sender_authenticated: true,
        }
    }

    /// (args, env, stdin JSON) of each hook run.
    type HookCall = (Vec<String>, Vec<(String, String)>, serde_json::Value);

    #[derive(Default)]
    pub(crate) struct Fake {
        pub modified: Mutex<Vec<(Vec<ThreadRef>, ThreadAction)>>,
        pub forwarded: Mutex<Vec<(String, String)>>,
        pub notified: Mutex<Vec<(String, String)>>,
        pub hooks: Mutex<Vec<HookCall>>,
        pub webhooks: Mutex<Vec<String>>,
    }

    impl Effects for Fake {
        async fn modify(
            &self,
            targets: Vec<ThreadRef>,
            action: ThreadAction,
        ) -> Result<(), String> {
            self.modified.lock().unwrap().push((targets, action));
            Ok(())
        }
        async fn forward(&self, message: &Message, to: &str) -> Result<String, String> {
            self.forwarded
                .lock()
                .unwrap()
                .push((message.id.clone(), to.to_string()));
            Ok("sent".into())
        }
        fn notify(&self, title: &str, body: &str) -> Result<(), String> {
            self.notified
                .lock()
                .unwrap()
                .push((title.to_string(), body.to_string()));
            Ok(())
        }
        async fn hook(
            &self,
            _program: &Path,
            args: &[String],
            env: Vec<(String, String)>,
            stdin: Vec<u8>,
            _timeout: Duration,
        ) -> Result<String, String> {
            self.hooks.lock().unwrap().push((
                args.to_vec(),
                env,
                serde_json::from_slice(&stdin).unwrap(),
            ));
            Ok("exit 0".into())
        }
        async fn webhook(
            &self,
            url: &str,
            _secret: Option<&str>,
            _body: Vec<u8>,
        ) -> Result<String, String> {
            self.webhooks.lock().unwrap().push(url.to_string());
            Ok("HTTP 200".into())
        }
    }

    const ACCT: &str = "me@x.example";

    pub(crate) fn store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&Account {
            id: ACCT.into(),
            email: ACCT.into(),
            display_name: None,
            nickname: None,
            color: "#123456".into(),
            added_at: 1,
            ..Account::default()
        })
        .unwrap();
        s
    }

    pub(crate) fn rule(id: &str, condition: &str, actions: Vec<Action>) -> Rule {
        Rule {
            id: id.into(),
            name: format!("Rule {id}"),
            enabled: true,
            dry_run: false,
            account_ids: None,
            profile_id: None,
            trigger: Trigger::NewMessage,
            condition: condition.into(),
            actions,
            stop_processing: false,
            include_body: false,
            created_at: 0,
            updated_at: 0,
            paused_reason: None,
        }
    }

    fn arrive(s: &Store, msgs: &[Message]) {
        s.upsert_messages(msgs).unwrap();
        let events: Vec<RuleEvent> = msgs
            .iter()
            .map(|m| RuleEvent {
                message_id: m.id.clone(),
                kind: RuleEventKind::NewMessage,
                labels: vec![],
            })
            .collect();
        s.enqueue_rule_events(ACCT, &events, 1).unwrap();
    }

    async fn pass(s: &Store, config: &RulesConfig, fx: &Fake, limits: &mut Limits) -> PassReport {
        let events = s.pending_rule_events(EVENT_BATCH).unwrap();
        let ctx = Ctx {
            store: s,
            config,
            profiles: &[],
            now: 1_758_000_000_000,
        };
        process_events(&ctx, fx, limits, &events).await.unwrap()
    }

    fn config(rules: Vec<Rule>) -> RulesConfig {
        RulesConfig {
            rules,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn matching_new_mail_is_acted_on_exactly_once() {
        let s = store();
        let mut other = message(ACCT, "m2", "t2");
        other.from.email = "ana@ruiz.example".into();
        arrive(&s, &[message(ACCT, "m1", "t1"), other]);
        let cfg = config(vec![rule("r1", "from:uber.example", vec![Action::Archive])]);
        let fx = Fake::default();
        let mut limits = Limits::default();
        let report = pass(&s, &cfg, &fx, &mut limits).await;
        assert_eq!(report.handled, 2);
        assert_eq!(report.fired.len(), 1);
        assert_eq!(report.fired[0].count, 1);
        let modified = fx.modified.lock().unwrap().clone();
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].0[0].thread_id, "t1");
        assert_eq!(modified[0].1, ThreadAction::Archive);
        assert!(s.pending_rule_events(10).unwrap().is_empty());

        // The same message reported again (history replay after a crash):
        // claimed already, nothing happens.
        arrive(&s, &[message(ACCT, "m1", "t1")]);
        let report = pass(&s, &cfg, &fx, &mut limits).await;
        assert!(report.fired.is_empty());
        assert_eq!(fx.modified.lock().unwrap().len(), 1);

        // History has an undo for it.
        let log = s.rule_log(Some("r1"), 10, None).unwrap();
        assert_eq!(log.len(), 1);
        let outcomes: Vec<Outcome> = serde_json::from_value(log[0].outcomes.clone()).unwrap();
        assert_eq!(outcomes[0].undo, Some(ThreadAction::MoveToInbox));
        assert_eq!(log[0].subject.as_deref(), Some("Your trip m1"));
    }

    #[tokio::test]
    async fn dry_run_logs_without_acting_or_claiming() {
        let s = store();
        arrive(&s, &[message(ACCT, "m1", "t1")]);
        let mut r = rule(
            "r1",
            "from:uber.example",
            vec![
                Action::Archive,
                Action::Forward {
                    to: "me@y.example".into(),
                    confirmed: true,
                },
                Action::Hook {
                    program: "/bin/echo".into(),
                    args: vec![],
                    timeout_secs: 5,
                },
            ],
        );
        r.dry_run = true;
        let mut cfg = config(vec![r.clone()]);
        cfg.allow_hooks = true;
        let fx = Fake::default();
        let report = pass(&s, &cfg, &fx, &mut Limits::default()).await;
        assert_eq!(report.fired[0].count, 1);
        assert!(report.fired[0].dry_run);
        assert!(fx.modified.lock().unwrap().is_empty());
        assert!(fx.forwarded.lock().unwrap().is_empty());
        assert!(fx.hooks.lock().unwrap().is_empty());
        let log = s.rule_log(Some("r1"), 10, None).unwrap();
        assert!(log[0].dry_run);
        assert!(log[0].outcomes.to_string().contains("would archive"));
        // Not claimed: switching to live later still acts on it.
        assert!(s
            .applied_rule_messages("r1", &[(ACCT.into(), "m1".into())])
            .unwrap()
            .is_empty());
        let ctx = Ctx {
            store: &s,
            config: &cfg,
            profiles: &[],
            now: 5,
        };
        r.dry_run = false;
        let run = run_rule(&ctx, &fx, &mut Limits::default(), &r, "manual", false)
            .await
            .unwrap();
        assert_eq!((run.matched, run.acted, run.already_applied), (1, 1, 0));
        assert_eq!(fx.modified.lock().unwrap().len(), 1);
        // A second manual run skips what the rule already did.
        let run = run_rule(&ctx, &fx, &mut Limits::default(), &r, "manual", false)
            .await
            .unwrap();
        assert_eq!((run.matched, run.acted, run.already_applied), (1, 0, 1));
    }

    #[tokio::test]
    async fn backfilled_sent_and_draft_mail_never_triggers() {
        let s = store();
        // Backfill stores mail without queueing any event.
        s.upsert_messages(&[message(ACCT, "old", "t0")]).unwrap();
        let mut sent = message(ACCT, "s1", "t1");
        sent.label_ids = vec!["SENT".into()];
        let mut draft = message(ACCT, "d1", "t2");
        draft.label_ids = vec!["DRAFT".into()];
        arrive(&s, &[sent, draft]);
        let cfg = config(vec![rule("r1", "from:uber.example", vec![Action::Star])]);
        let fx = Fake::default();
        let report = pass(&s, &cfg, &fx, &mut Limits::default()).await;
        assert!(report.fired.is_empty());
        assert!(fx.modified.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn stop_processing_scope_and_label_trigger() {
        let s = store();
        s.replace_labels(
            ACCT,
            &[Label {
                account_id: ACCT.into(),
                id: "Label_7".into(),
                name: "Receipts".into(),
                kind: "user".into(),
                color: None,
                unread_count: None,
                hidden: false,
            }],
        )
        .unwrap();
        arrive(&s, &[message(ACCT, "m1", "t1")]);
        let mut first = rule(
            "r1",
            "from:uber.example",
            vec![Action::AddLabel {
                label: "receipts".into(),
            }],
        );
        first.stop_processing = true;
        let second = rule("r2", "trip", vec![Action::Archive]);
        let mut elsewhere = rule("r3", "trip", vec![Action::Star]);
        elsewhere.account_ids = Some(vec!["other@x.example".into()]);
        let mut on_label = rule("r4", "trip", vec![Action::Notify]);
        on_label.trigger = Trigger::LabelAdded {
            label: "Receipts".into(),
        };
        let cfg = config(vec![first, second, elsewhere, on_label.clone()]);
        let fx = Fake::default();
        let mut limits = Limits::default();
        let report = pass(&s, &cfg, &fx, &mut limits).await;
        let fired: Vec<&str> = report.fired.iter().map(|f| f.rule_id.as_str()).collect();
        assert_eq!(fired, vec!["r1"]);
        assert_eq!(
            fx.modified.lock().unwrap()[0].1,
            ThreadAction::AddLabel {
                label_id: "Label_7".into()
            }
        );

        // You add the label yourself (Gmail reports it via history): r4 fires.
        // (r1's own add would be an echo; rules_do_not_trigger_on_their_own_echo.)
        s.modify_thread_labels(ACCT, "t1", &["Label_7".into()], &[])
            .unwrap();
        let mut limits = Limits::default();
        s.enqueue_rule_events(
            ACCT,
            &[RuleEvent {
                message_id: "m1".into(),
                kind: RuleEventKind::LabelAdded,
                labels: vec!["Label_7".into()],
            }],
            2,
        )
        .unwrap();
        let cfg = config(vec![on_label]);
        let report = pass(&s, &cfg, &fx, &mut limits).await;
        assert_eq!(report.fired[0].rule_id, "r4");
        assert_eq!(
            fx.notified.lock().unwrap()[0],
            (
                "Rule r4".to_string(),
                "Uber Receipts: Your trip m1".to_string()
            )
        );
    }

    #[tokio::test]
    async fn hooks_need_the_setting_and_get_id_env_only() {
        let s = store();
        let mut m = message(ACCT, "m1", "t1");
        m.subject = "$(rm -rf ~)".into();
        arrive(&s, &[m]);
        let hook = Action::Hook {
            program: "/usr/local/bin/on-mail".into(),
            args: vec!["--quiet".into()],
            timeout_secs: 5,
        };
        let web = Action::Webhook {
            url: "https://hooks.example/in".into(),
            secret: Some("k".into()),
        };
        let mut cfg = config(vec![rule("r1", "from:uber.example", vec![hook, web])]);
        let fx = Fake::default();
        pass(&s, &cfg, &fx, &mut Limits::default()).await;
        assert!(fx.hooks.lock().unwrap().is_empty());
        assert!(fx.webhooks.lock().unwrap().is_empty());
        let log = s.rule_log(Some("r1"), 10, None).unwrap();
        assert!(!log[0].ok);
        assert!(log[0].outcomes.to_string().contains("hooks are off"));

        cfg.allow_hooks = true;
        arrive(&s, &[message(ACCT, "m2", "t2")]);
        pass(&s, &cfg, &fx, &mut Limits::default()).await;
        let hooks = fx.hooks.lock().unwrap().clone();
        assert_eq!(hooks.len(), 1);
        let (args, env, json) = &hooks[0];
        assert_eq!(args, &vec!["--quiet".to_string()]);
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("PENGUIN_THREAD_ID").as_deref(), Some("t2"));
        assert_eq!(get("PENGUIN_ACCOUNT").as_deref(), Some(ACCT));
        assert!(
            env.iter().all(|(_, v)| !v.contains("trip")),
            "no content in env"
        );
        assert_eq!(json["kind"], "ruleMatch");
        assert_eq!(json["data"]["includesBody"], false);
        assert_eq!(json["data"]["messages"][0]["message"]["text"], "");
        assert_eq!(fx.webhooks.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn forward_is_rate_limited_and_runaway_rules_pause() {
        let s = store();
        let many: Vec<Message> = (0..(MAX_FORWARDS_PER_HOUR + 5))
            .map(|i| message(ACCT, &format!("f{i}"), &format!("t{i}")))
            .collect();
        arrive(&s, &many);
        let cfg = config(vec![rule(
            "r1",
            "from:uber.example",
            vec![Action::Forward {
                to: "me@y.example".into(),
                confirmed: true,
            }],
        )]);
        let fx = Fake::default();
        let mut limits = Limits::default();
        let report = pass(&s, &cfg, &fx, &mut limits).await;
        assert_eq!(fx.forwarded.lock().unwrap().len(), MAX_FORWARDS_PER_HOUR);
        assert_eq!(report.fired[0].failed, 5);
        assert!(s
            .rule_log(Some("r1"), 100, None)
            .unwrap()
            .iter()
            .any(|e| e.outcomes.to_string().contains("forwards in the last hour")));

        // Circuit breaker.
        let mut limits = Limits::default();
        assert!(limits.charge_rule("r9", MAX_RULE_MESSAGES_PER_HOUR, 0));
        assert!(!limits.charge_rule("r9", 1, 1));
        assert!(limits.charge_rule("r9", 1, HOUR_MS + 1));
        let flood: Vec<Message> = (0..(MAX_RULE_MESSAGES_PER_HOUR + 1))
            .map(|i| message(ACCT, &format!("x{i}"), &format!("tx{i}")))
            .collect();
        arrive(&s, &flood);
        let cfg = config(vec![rule(
            "r2",
            "from:uber.example",
            vec![Action::MarkRead],
        )]);
        let fx = Fake::default();
        let mut limits = Limits::default();
        let mut paused = Vec::new();
        while !s.pending_rule_events(1).unwrap().is_empty() {
            paused.extend(pass(&s, &cfg, &fx, &mut limits).await.pause);
        }
        assert_eq!(paused.len(), 1);
        assert_eq!(paused[0].0, "r2");
    }

    /// The queue and the claims live in the database: events queued before a
    /// restart are handled after it, and a claim interrupted mid-action is
    /// never re-run when history replays the same message.
    #[tokio::test]
    async fn queue_and_claims_survive_a_restart() {
        let dir =
            std::env::temp_dir().join(format!("penguin-rules-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("penguin.db");
        let cfg = config(vec![rule("r1", "from:uber.example", vec![Action::Archive])]);
        {
            let s = Store::open(&path).unwrap();
            s.upsert_account(&Account {
                id: ACCT.into(),
                email: ACCT.into(),
                display_name: None,
                nickname: None,
                color: "#123456".into(),
                added_at: 1,
                ..Account::default()
            })
            .unwrap();
            arrive(&s, &[message(ACCT, "m1", "t1"), message(ACCT, "m2", "t2")]);
            // "Crash" after m2 was claimed but before its actions ran.
            let queued = s.pending_rule_events(10).unwrap();
            let m2 = queued.iter().find(|e| e.message_id == "m2").unwrap().id;
            s.claim_rule_applications(
                &[m2],
                &[RuleApplication {
                    rule_id: "r1".into(),
                    account_id: ACCT.into(),
                    message_id: "m2".into(),
                    thread_id: "t2".into(),
                }],
                1,
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.interrupt_pending_rule_applications().unwrap(), 1);
        // History replays m2 after the restart.
        s.enqueue_rule_events(
            ACCT,
            &[RuleEvent {
                message_id: "m2".into(),
                kind: RuleEventKind::NewMessage,
                labels: vec![],
            }],
            2,
        )
        .unwrap();
        let fx = Fake::default();
        let report = pass(&s, &cfg, &fx, &mut Limits::default()).await;
        assert_eq!(report.handled, 2);
        let modified = fx.modified.lock().unwrap().clone();
        assert_eq!(modified.len(), 1, "m1 once; interrupted m2 not retried");
        assert_eq!(modified[0].0[0].thread_id, "t1");
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A rule labels a message; Gmail echoes that as labelsAdded. Neither
    /// the same rule nor a label-triggered rule fires on the echo, but a
    /// label you add yourself does trigger.
    #[tokio::test]
    async fn rules_do_not_trigger_on_their_own_echo() {
        let s = store();
        s.replace_labels(
            ACCT,
            &[Label {
                account_id: ACCT.into(),
                id: "Label_7".into(),
                name: "Receipts".into(),
                kind: "user".into(),
                color: None,
                unread_count: None,
                hidden: false,
            }],
        )
        .unwrap();
        arrive(&s, &[message(ACCT, "m1", "t1"), message(ACCT, "m2", "t2")]);
        let tagger = rule(
            "r1",
            "from:uber.example",
            vec![Action::AddLabel {
                label: "Receipts".into(),
            }],
        );
        let mut on_label = rule("r2", "trip", vec![Action::Star]);
        on_label.trigger = Trigger::LabelAdded {
            label: "Receipts".into(),
        };
        let cfg = config(vec![tagger, on_label]);
        let fx = Fake::default();
        let mut limits = Limits::default();
        pass(&s, &cfg, &fx, &mut limits).await;
        // The fake doesn't touch the store; apply what Gmail would echo.
        for t in ["t1", "t2"] {
            s.modify_thread_labels(ACCT, t, &["Label_7".into()], &[])
                .unwrap();
        }
        let echo = |id: &str| RuleEvent {
            message_id: id.into(),
            kind: RuleEventKind::LabelAdded,
            labels: vec!["Label_7".into()],
        };
        s.enqueue_rule_events(ACCT, &[echo("m1")], 2).unwrap();
        let report = pass(&s, &cfg, &fx, &mut limits).await;
        assert!(report.fired.is_empty(), "echo re-triggered: {report:?}");

        // Someone labels m3 by hand: the label rule fires.
        let m3 = message(ACCT, "m3", "t3");
        s.upsert_messages(&[Message {
            label_ids: vec!["INBOX".into(), "Label_7".into()],
            ..m3
        }])
        .unwrap();
        s.enqueue_rule_events(ACCT, &[echo("m3")], 3).unwrap();
        let report = pass(&s, &cfg, &fx, &mut limits).await;
        let fired: Vec<&str> = report.fired.iter().map(|f| f.rule_id.as_str()).collect();
        assert_eq!(fired, vec!["r2"]);
        // Labels removed again on the same history page don't trigger.
        s.upsert_messages(&[message(ACCT, "m4", "t4")]).unwrap();
        s.enqueue_rule_events(ACCT, &[echo("m4")], 4).unwrap();
        assert!(pass(&s, &cfg, &fx, &mut limits).await.fired.is_empty());
    }

    #[test]
    fn gmail_pacing_waits_out_the_minute() {
        let mut l = Limits::default();
        assert_eq!(l.gmail_wait_ms(0), 0);
        l.charge_gmail(MAX_GMAIL_CHANGES_PER_MINUTE, 1_000);
        assert_eq!(l.gmail_wait_ms(1_000), MINUTE_MS);
        assert_eq!(l.gmail_wait_ms(31_000), 30_000);
        assert_eq!(l.gmail_wait_ms(61_001), 0);
    }

    #[tokio::test]
    async fn preview_counts_and_marks_applied() {
        let s = store();
        arrive(&s, &[message(ACCT, "m1", "t1"), message(ACCT, "m22", "t2")]);
        let cfg = config(vec![rule("r1", "from:uber.example", vec![Action::Archive])]);
        let fx = Fake::default();
        let events = s.pending_rule_events(1).unwrap();
        let ctx = Ctx {
            store: &s,
            config: &cfg,
            profiles: &[],
            now: 1,
        };
        process_events(&ctx, &fx, &mut Limits::default(), &events)
            .await
            .unwrap();
        let p = preview(&s, "from:uber.example", None, Some("r1"), 10, 100).unwrap();
        assert_eq!((p.count, p.capped, p.items.len()), (2, false, 2));
        let applied: Vec<bool> = p.items.iter().map(|i| i.already_applied).collect();
        assert_eq!(applied.iter().filter(|a| **a).count(), 1);
        assert!(preview(&s, "", None, None, 10, 100).is_err());
    }
}
