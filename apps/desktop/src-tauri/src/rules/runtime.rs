//! Rules glue: the background task (drains the event queue, runs due
//! schedules), the app's [`Effects`], the global audit log and the Tauri
//! commands. Never blocks sync: the sync task only appends to the queue.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use penguin_core::store::{RuleEvent, RuleEventKind, RuleLogEntry, RuleStats};
use penguin_core::{Message, Store};
use penguin_provider::compose::{Draft, OutgoingAttachment};
use penguin_provider::quote;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use super::engine::{self, Ctx, Effects, Fired, Limits, Outcome, RulePreview, RunReport};
use super::hooks;
use super::model::{self, Rule, RuleInput, RulesConfig, RulesStore, Trigger, MAX_RULES};
use crate::commands;
use crate::error::{CmdError, CmdResult};
use crate::ops;
use crate::state::{blocking, AppState};
use crate::views::{ThreadAction, ThreadRef};

/// Payload: `RulesOverview`, after every change to rules.json.
pub const EVENT_RULES_CHANGED: &str = "penguin://rules-changed";
/// Payload: `Fired`, once per rule per pass that matched something.
pub const EVENT_RULE_FIRED: &str = "penguin://rule-fired";
pub const AUDIT_FILE: &str = "rules-audit.log";
const MAX_AUDIT_BYTES: u64 = 5 * 1024 * 1024;
/// Longest sleep before a scheduled rule's slot (a timer doesn't count
/// time the Mac spends asleep, so this bounds the latency after a wake).
/// With no scheduled rule the task sleeps until woken: rule events and
/// rule edits wake it.
const MAX_IDLE: Duration = Duration::from_secs(60);
/// Preview: matches listed / counted.
const PREVIEW_ITEMS: usize = 50;
const PREVIEW_COUNT_CAP: usize = 100_000;

fn wake() -> &'static Notify {
    static WAKE: OnceLock<Notify> = OnceLock::new();
    WAKE.get_or_init(Notify::new)
}

/// Called by the sync observer (on the sync task): queue durably, then poke
/// the rules task. One small insert; errors are logged, never propagated
/// into sync.
pub fn enqueue(store: &Store, account_id: &str, events: Vec<RuleEvent>) {
    if events.is_empty() {
        return;
    }
    // A short write, but it may wait on the writer lock behind another
    // account's big transaction: let the scheduler move other tasks off this
    // worker meanwhile. Written before the sync cursor moves (durability).
    let write = || store.enqueue_rule_events(account_id, &events, ops::now_ms());
    let multi_thread = tokio::runtime::Handle::try_current()
        .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
    let result = if multi_thread {
        tokio::task::block_in_place(write)
    } else {
        write()
    };
    match result {
        Ok(()) => wake().notify_one(),
        Err(e) => tracing::warn!(error = %e, "could not queue mail for rules"),
    }
}

pub fn new_messages(ids: Vec<String>) -> Vec<RuleEvent> {
    ids.into_iter()
        .map(|message_id| RuleEvent {
            message_id,
            kind: RuleEventKind::NewMessage,
            labels: Vec::new(),
        })
        .collect()
}

pub fn labels_added(changes: Vec<(String, Vec<String>)>) -> Vec<RuleEvent> {
    changes
        .into_iter()
        .map(|(message_id, labels)| RuleEvent {
            message_id,
            kind: RuleEventKind::LabelAdded,
            labels,
        })
        .collect()
}

/// `<log dir>/rules-audit.log`: one JSON line per rule history row, with
/// ids and per-action ok/failed. Never subjects, addresses or bodies.
struct AuditFile {
    path: Option<PathBuf>,
    lock: Mutex<()>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditLine<'a> {
    ts: String,
    rule_id: &'a str,
    trigger: &'a str,
    dry_run: bool,
    ok: bool,
    account_id: Option<&'a str>,
    thread_id: Option<&'a str>,
    message_id: Option<&'a str>,
    matched: u32,
    actions: Vec<(String, bool)>,
}

impl AuditFile {
    fn open(dir: Option<&Path>) -> AuditFile {
        let path = dir.and_then(|d| {
            std::fs::create_dir_all(d).ok()?;
            let path = d.join(AUDIT_FILE);
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_AUDIT_BYTES) {
                if let Err(e) = std::fs::rename(&path, d.join(format!("{AUDIT_FILE}.1"))) {
                    tracing::warn!(error = %e, "could not rotate the rules audit log");
                }
            }
            Some(path)
        });
        AuditFile {
            path,
            lock: Mutex::new(()),
        }
    }

    fn record(&self, entries: &[RuleLogEntry]) {
        let Some(path) = &self.path else { return };
        let mut text = String::new();
        for e in entries {
            let outcomes: Vec<Outcome> =
                serde_json::from_value(e.outcomes.clone()).unwrap_or_default();
            let line = AuditLine {
                ts: crate::agent::output::iso(e.ts),
                rule_id: &e.rule_id,
                trigger: &e.trigger,
                dry_run: e.dry_run,
                ok: e.ok,
                account_id: e.account_id.as_deref(),
                thread_id: e.thread_id.as_deref(),
                message_id: e.message_id.as_deref(),
                matched: e.matched,
                actions: outcomes.into_iter().map(|o| (o.action, o.ok)).collect(),
            };
            if let Ok(s) = serde_json::to_string(&line) {
                text.push_str(&s);
                text.push('\n');
            }
        }
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        if let Err(e) = opts
            .open(path)
            .and_then(|mut f| f.write_all(text.as_bytes()))
        {
            tracing::warn!(error = %e, "could not write the rules audit log");
        }
    }
}

/// Managed state for the rules feature.
pub struct Rules {
    pub config: RulesStore,
    limits: tokio::sync::Mutex<Limits>,
    audit: AuditFile,
}

impl Rules {
    pub fn init(app: &AppHandle, state: &AppState) -> Arc<Rules> {
        let log_dir = app.path().app_log_dir().ok();
        Arc::new(Rules {
            config: RulesStore::load(&state.paths.config_dir),
            limits: tokio::sync::Mutex::new(Limits::default()),
            audit: AuditFile::open(log_dir.as_deref()),
        })
    }

    fn audit_path(&self) -> Option<String> {
        self.audit.path.as_ref().map(|p| p.display().to_string())
    }
}

/// The app's side effects: the real modify path, Gmail send, macOS
/// notifications, processes and HTTP.
struct AppEffects {
    app: AppHandle,
    state: Arc<AppState>,
}

/// "Fwd:" copy of `m` to `to`, the way Gmail forwards: the header block and
/// the original's HTML (sanitized, formatting kept) in Gmail's quote markup,
/// the same as text, and the original's attachments: its files, plus the
/// inline images its HTML shows as inline parts under fresh Content-IDs.
pub fn forward_draft(m: &Message, to: &str) -> Draft {
    let subject = m.subject.trim();
    let subject = if subject.to_ascii_lowercase().starts_with("fwd:") {
        subject.to_string()
    } else {
        format!("Fwd: {subject}")
    };
    let date = chrono::DateTime::from_timestamp_millis(m.date)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%a, %b %-d, %Y at %-I:%M %p")
                .to_string()
        })
        .unwrap_or_default();
    let text = if m.body_text.trim().is_empty() {
        m.body_html
            .as_deref()
            .map(penguin_core::text::html_to_text)
            .unwrap_or_default()
    } else {
        m.body_text.clone()
    };
    // The original's inline images ride along under fresh ids.
    let (cids, images) = penguin_provider::inline::quote_images(m, None);
    let original = quote::Original {
        from: &m.from,
        to: &m.to,
        cc: &m.cc,
        subject: &m.subject,
        date: &date,
        html: m.body_html.as_deref(),
        text: &text,
        cids: Some(&cids),
    };
    Draft {
        request_read_receipt: None,
        account_id: m.account_id.clone(),
        to: vec![penguin_core::Address {
            name: None,
            email: to.to_string(),
        }],
        cc: vec![],
        bcc: vec![],
        subject,
        body_text: quote::forward_text(&original),
        body_html: Some(format!("<br>{}", quote::forward_html(&original))),
        reply_to_thread_id: None,
        reply_to_message_id: None,
        attachments: m
            .attachments
            .iter()
            .filter(|a| !a.inline)
            .map(|a| OutgoingAttachment::Gmail {
                message_id: m.id.clone(),
                attachment_id: a.id.clone(),
                filename: a.filename.clone(),
                mime_type: a.mime_type.clone(),
                size: a.size,
                account_id: None,
                content_id: None,
            })
            .chain(images)
            .collect(),
    }
}

impl Effects for AppEffects {
    async fn modify(&self, targets: Vec<ThreadRef>, action: ThreadAction) -> Result<(), String> {
        commands::apply_thread_action(self.state.clone(), targets, action)
            .await
            .map_err(|e| e.message)
    }

    async fn forward(&self, message: &Message, to: &str) -> Result<String, String> {
        let send = async {
            let account = self.state.account(&message.account_id).await?;
            let from = ops::from_address(&account);
            let draft = forward_draft(message, to);
            let provider = self.state.provider(&account.id).await?;
            crate::outgoing::send(
                &self.state.store,
                &self.state.paths,
                provider.as_ref(),
                &Default::default(),
                &from,
                &draft,
                None,
            )
            .await?;
            self.state.poke(&account.id);
            CmdResult::Ok(())
        };
        send.await.map_err(|e| e.message)?;
        Ok(format!("forwarded to {to}"))
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), String> {
        use tauri_plugin_notification::NotificationExt;
        self.app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|e| e.to_string())
    }

    async fn hook(
        &self,
        program: &Path,
        args: &[String],
        env: Vec<(String, String)>,
        stdin: Vec<u8>,
        timeout: Duration,
    ) -> Result<String, String> {
        hooks::run_hook(program, args, &env, &stdin, timeout).await
    }

    async fn webhook(
        &self,
        url: &str,
        secret: Option<&str>,
        body: Vec<u8>,
    ) -> Result<String, String> {
        hooks::post_webhook(url, secret, body, ops::now_ms()).await
    }
}

fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    if let Err(e) = app.emit(event, payload) {
        tracing::warn!(event, error = %e, "failed to emit event");
    }
}

/// Audit, toast and pause after a pass or a run.
async fn publish(
    app: &AppHandle,
    state: &AppState,
    rules: &Rules,
    fired: &[Fired],
    pause: Vec<(String, String)>,
) {
    let ids: Vec<i64> = fired
        .iter()
        .flat_map(|f| f.log_ids.iter().copied())
        .collect();
    if !ids.is_empty() {
        let store = state.store.clone();
        match blocking(move || {
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                out.extend(store.rule_log_entry(id)?);
            }
            Ok(out)
        })
        .await
        {
            Ok(entries) => rules.audit.record(&entries),
            Err(e) => tracing::warn!(error = %e, "could not read rule history for the audit log"),
        }
    }
    for f in fired {
        tracing::info!(rule = %f.rule_id, count = f.count, failed = f.failed, dry_run = f.dry_run, "rule fired");
        emit(app, EVENT_RULE_FIRED, f.clone());
    }
    if pause.is_empty() {
        return;
    }
    let result = rules.config.update(|c| {
        for (id, reason) in &pause {
            if let Some(r) = c.rules.iter_mut().find(|r| &r.id == id) {
                r.enabled = false;
                r.paused_reason = Some(reason.clone());
            }
        }
        Ok(())
    });
    match result {
        Ok(_) => {
            tracing::warn!(rules = pause.len(), "paused runaway rules");
            emit_changed(app, state, rules).await;
        }
        Err(e) => tracing::error!(error = %e, "could not pause a runaway rule"),
    }
}

/// Start the rules task (once, at app setup).
pub fn spawn(app: AppHandle, state: Arc<AppState>, rules: Arc<Rules>) {
    tauri::async_runtime::spawn(async move {
        let store = state.store.clone();
        match blocking(move || {
            let n = store.interrupt_pending_rule_applications()?;
            store.prune_rule_data(ops::now_ms())?;
            Ok(n)
        })
        .await
        {
            Ok(0) => {}
            Ok(n) => tracing::warn!(
                count = n,
                "rule actions were interrupted by the last shutdown; not retried"
            ),
            Err(e) => tracing::warn!(error = %e, "rules startup housekeeping failed"),
        }
        let fx = AppEffects {
            app: app.clone(),
            state: state.clone(),
        };
        loop {
            if let Err(e) = drain(&app, &state, &rules, &fx).await {
                tracing::warn!(error = %e, "rules pass failed");
            }
            let next = run_due_schedules(&app, &state, &rules, &fx).await;
            let Some(next) = next else {
                wake().notified().await;
                continue;
            };
            let wait = Duration::from_millis((next - ops::now_ms()).max(0) as u64)
                .clamp(Duration::from_millis(250), MAX_IDLE);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = wake().notified() => {}
            }
        }
    });
}

async fn drain(
    app: &AppHandle,
    state: &Arc<AppState>,
    rules: &Rules,
    fx: &AppEffects,
) -> CmdResult<()> {
    loop {
        // Stay under the rules' share of the Gmail quota.
        let wait = rules.limits.lock().await.gmail_wait_ms(ops::now_ms());
        if wait > 0 {
            tokio::time::sleep(Duration::from_millis(wait as u64)).await;
        }
        let store = state.store.clone();
        let events = blocking(move || Ok(store.pending_rule_events(engine::EVENT_BATCH)?)).await?;
        if events.is_empty() {
            return Ok(());
        }
        let config = rules.config.get();
        let profiles = state.settings.get().profiles;
        let ctx = Ctx {
            store: &state.store,
            config: &config,
            profiles: &profiles,
            now: ops::now_ms(),
        };
        let report = {
            let mut limits = rules.limits.lock().await;
            // Store calls here are small indexed reads/writes on the
            // just-arrived ids; actions themselves are async.
            engine::process_events(&ctx, fx, &mut limits, &events).await?
        };
        publish(app, state, rules, &report.fired, report.pause).await;
    }
}

/// Run scheduled rules whose slot has passed; returns the next slot.
async fn run_due_schedules(
    app: &AppHandle,
    state: &Arc<AppState>,
    rules: &Rules,
    fx: &AppEffects,
) -> Option<i64> {
    let config = rules.config.get();
    let now = ops::now_ms();
    let mut next: Option<i64> = None;
    for rule in config.rules.iter().filter(|r| r.enabled) {
        let Some((_, slot)) = super::schedule::slots(&rule.trigger, now, &chrono::Local) else {
            continue;
        };
        next = Some(next.map_or(slot, |n| n.min(slot)));
        let store = state.store.clone();
        let id = rule.id.clone();
        let last = blocking(move || Ok(store.rule_last_scheduled(&id)?))
            .await
            .unwrap_or(None);
        if !super::schedule::is_due(&rule.trigger, last, rule.updated_at, now, &chrono::Local) {
            continue;
        }
        // Mark first: a crash mid-run skips this slot instead of repeating it.
        let store = state.store.clone();
        let id = rule.id.clone();
        if let Err(e) = blocking(move || Ok(store.set_rule_last_scheduled(&id, now)?)).await {
            tracing::warn!(rule = %rule.id, error = %e, "could not record a scheduled run");
            continue;
        }
        let profiles = state.settings.get().profiles;
        let ctx = Ctx {
            store: &state.store,
            config: &config,
            profiles: &profiles,
            now,
        };
        let result = {
            let mut limits = rules.limits.lock().await;
            engine::run_rule(&ctx, fx, &mut limits, rule, "schedule", rule.dry_run).await
        };
        match result {
            Ok(run) if run.acted > 0 => {
                let fired = Fired {
                    rule_id: rule.id.clone(),
                    rule_name: rule.name.clone(),
                    trigger: "schedule".into(),
                    dry_run: run.dry_run,
                    count: run.acted,
                    failed: run.failed,
                    log_ids: run.log_ids,
                };
                publish(app, state, rules, &[fired], Vec::new()).await;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(rule = %rule.id, error = %e, "scheduled rule failed"),
        }
    }
    next
}

// ---------------------------------------------------------------------------
// commands

type AppStateRef<'a> = State<'a, Arc<AppState>>;
type RulesRef<'a> = State<'a, Arc<Rules>>;

/// Settings → Rules.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulesOverview {
    pub allow_hooks: bool,
    pub rules: Vec<Rule>,
    pub stats: Vec<RuleStats>,
    pub audit_log_path: Option<String>,
}

async fn overview(state: &AppState, rules: &Rules) -> CmdResult<RulesOverview> {
    let config = rules.config.get();
    let store = state.store.clone();
    let stats = blocking(move || Ok(store.rule_stats()?)).await?;
    Ok(RulesOverview {
        allow_hooks: config.allow_hooks,
        rules: config.rules,
        stats,
        audit_log_path: rules.audit_path(),
    })
}

async fn emit_changed(app: &AppHandle, state: &AppState, rules: &Rules) {
    match overview(state, rules).await {
        Ok(o) => emit(app, EVENT_RULES_CHANGED, o),
        Err(e) => tracing::warn!(error = %e, "could not load rules for rules-changed"),
    }
}

fn update(
    rules: &Rules,
    f: impl FnOnce(&mut RulesConfig) -> Result<Rule, String>,
) -> CmdResult<Rule> {
    rules
        .config
        .update(f)
        .map(|(_, r)| r)
        .map_err(CmdError::invalid)
}

#[tauri::command]
pub async fn list_rules(state: AppStateRef<'_>, rules: RulesRef<'_>) -> CmdResult<RulesOverview> {
    overview(&state, &rules).await
}

/// Create (`rule.id` null) or replace a rule. The condition must parse to at
/// least one search term; trash and forward actions must carry `confirmed`.
#[tauri::command]
pub async fn save_rule(
    app: AppHandle,
    state: AppStateRef<'_>,
    rules: RulesRef<'_>,
    rule: RuleInput,
) -> CmdResult<Rule> {
    let store = state.store.clone();
    let condition = rule.condition.clone();
    blocking(move || Ok(store.count_query_matches(&condition, None, 1).map(|_| ())?)).await?;
    let now = ops::now_ms();
    let saved = update(&rules, |c| {
        let prev_idx = match &rule.id {
            Some(id) => Some(
                c.rules
                    .iter()
                    .position(|r| &r.id == id)
                    .ok_or("That rule no longer exists.")?,
            ),
            None => None,
        };
        if prev_idx.is_none() && c.rules.len() >= MAX_RULES {
            return Err(format!("You can have at most {MAX_RULES} rules."));
        }
        let mut id = model::new_rule_id();
        while c.rules.iter().any(|r| r.id == id) {
            id = model::new_rule_id();
        }
        let saved = rule.into_rule(prev_idx.map(|i| &c.rules[i]), id, now)?;
        match prev_idx {
            Some(i) => c.rules[i] = saved.clone(),
            None => c.rules.push(saved.clone()),
        }
        Ok(saved)
    })?;
    emit_changed(&app, &state, &rules).await;
    wake().notify_one();
    Ok(saved)
}

#[tauri::command]
pub async fn delete_rule(
    app: AppHandle,
    state: AppStateRef<'_>,
    rules: RulesRef<'_>,
    id: String,
) -> CmdResult<()> {
    rules
        .config
        .update(|c| {
            c.rules.retain(|r| r.id != id);
            Ok(())
        })
        .map_err(CmdError::invalid)?;
    let store = state.store.clone();
    blocking(move || Ok(store.delete_rule_data(&id)?)).await?;
    emit_changed(&app, &state, &rules).await;
    Ok(())
}

/// The list's toggle. Turning a paused rule on clears the pause.
#[tauri::command]
pub async fn set_rule_enabled(
    app: AppHandle,
    state: AppStateRef<'_>,
    rules: RulesRef<'_>,
    id: String,
    enabled: bool,
) -> CmdResult<Rule> {
    let now = ops::now_ms();
    let saved = update(&rules, |c| {
        let r = c
            .rules
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or("That rule no longer exists.")?;
        r.enabled = enabled;
        r.updated_at = now;
        if enabled {
            r.paused_reason = None;
        }
        Ok(r.clone())
    })?;
    emit_changed(&app, &state, &rules).await;
    wake().notify_one();
    Ok(saved)
}

/// New order (every rule id exactly once).
#[tauri::command]
pub async fn reorder_rules(
    app: AppHandle,
    state: AppStateRef<'_>,
    rules: RulesRef<'_>,
    ids: Vec<String>,
) -> CmdResult<()> {
    rules
        .config
        .update(|c| {
            let mut sorted = ids.clone();
            sorted.sort();
            let mut have: Vec<String> = c.rules.iter().map(|r| r.id.clone()).collect();
            have.sort();
            if sorted != have {
                return Err("The rule list changed; try again.".to_string());
            }
            c.rules
                .sort_by_key(|r| ids.iter().position(|i| i == &r.id).unwrap_or(usize::MAX));
            Ok(())
        })
        .map_err(CmdError::invalid)?;
    emit_changed(&app, &state, &rules).await;
    Ok(())
}

/// Settings → Developer → "Allow hooks".
#[tauri::command]
pub async fn set_allow_hooks(
    app: AppHandle,
    state: AppStateRef<'_>,
    rules: RulesRef<'_>,
    allow: bool,
) -> CmdResult<RulesOverview> {
    rules
        .config
        .update(|c| {
            c.allow_hooks = allow;
            Ok(())
        })
        .map_err(CmdError::invalid)?;
    tracing::info!(allow, "rule hooks toggled");
    emit_changed(&app, &state, &rules).await;
    overview(&state, &rules).await
}

/// Editor: live match count and the newest matches for a condition (local
/// only; no rule needs to exist). `ruleId` marks what that rule already did.
#[tauri::command]
pub async fn preview_rule(
    state: AppStateRef<'_>,
    condition: String,
    account_ids: Option<Vec<String>>,
    profile_id: Option<String>,
    rule_id: Option<String>,
    limit: Option<u32>,
) -> CmdResult<RulePreview> {
    let probe = Rule {
        id: String::new(),
        name: String::new(),
        enabled: true,
        dry_run: true,
        account_ids,
        profile_id,
        trigger: Trigger::Manual,
        condition,
        actions: Vec::new(),
        stop_processing: false,
        include_body: false,
        created_at: 0,
        updated_at: 0,
        paused_reason: None,
    };
    let scope = engine::rule_scope(&probe, &state.settings.get().profiles);
    let store = state.store.clone();
    let limit = limit.map_or(PREVIEW_ITEMS, |l| (l as usize).min(200));
    blocking(move || {
        Ok(engine::preview(
            &store,
            &probe.condition,
            scope.as_deref(),
            rule_id.as_deref(),
            limit,
            PREVIEW_COUNT_CAP,
        )?)
    })
    .await
}

/// "Run now" / "Also apply to existing mail": up to 1,000 matching messages
/// the rule hasn't acted on. `dryRun` true only logs what would happen.
#[tauri::command]
pub async fn run_rule(
    app: AppHandle,
    state: AppStateRef<'_>,
    rules: RulesRef<'_>,
    id: String,
    dry_run: bool,
) -> CmdResult<RunReport> {
    let config = rules.config.get();
    let rule = config
        .rules
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| CmdError::not_found("That rule no longer exists."))?;
    let profiles = state.settings.get().profiles;
    let fx = AppEffects {
        app: app.clone(),
        state: state.inner().clone(),
    };
    let ctx = Ctx {
        store: &state.store,
        config: &config,
        profiles: &profiles,
        now: ops::now_ms(),
    };
    let run = {
        let mut limits = rules.limits.lock().await;
        engine::run_rule(&ctx, &fx, &mut limits, rule, "manual", dry_run).await?
    };
    if run.acted > 0 {
        let fired = Fired {
            rule_id: rule.id.clone(),
            rule_name: rule.name.clone(),
            trigger: "manual".into(),
            dry_run,
            count: run.acted,
            failed: run.failed,
            log_ids: run.log_ids.clone(),
        };
        publish(&app, &state, &rules, &[fired], Vec::new()).await;
    }
    Ok(run)
}

/// Newest first. `ruleId` null = every rule; `beforeId` pages back.
#[tauri::command]
pub async fn rule_history(
    state: AppStateRef<'_>,
    rule_id: Option<String>,
    limit: Option<u32>,
    before_id: Option<i64>,
) -> CmdResult<Vec<RuleLogEntry>> {
    let store = state.store.clone();
    let limit = limit.map_or(100, |l| (l as usize).min(500));
    blocking(move || Ok(store.rule_log(rule_id.as_deref(), limit, before_id)?)).await
}

/// Undo history rows: each successful label/archive/read/star/trash action
/// is reversed on its thread through the normal modify path. Returns how
/// many rows were undone (dry-run, already-undone and forward/notify/hook
/// rows are skipped — those can't be taken back).
#[tauri::command]
pub async fn undo_rule_actions(state: AppStateRef<'_>, log_ids: Vec<i64>) -> CmdResult<usize> {
    let store = state.store.clone();
    let entries = blocking(move || {
        let mut out = Vec::new();
        for id in log_ids {
            out.extend(store.rule_log_entry(id)?);
        }
        Ok(out)
    })
    .await?;
    // (action, account) → threads, reversed in the order they were applied.
    let mut batches: Vec<(ThreadAction, String, Vec<String>)> = Vec::new();
    let mut undone = Vec::new();
    for e in entries.iter().filter(|e| !e.dry_run && !e.undone) {
        let (Some(account), Some(thread)) = (&e.account_id, &e.thread_id) else {
            continue;
        };
        let outcomes: Vec<Outcome> = serde_json::from_value(e.outcomes.clone()).unwrap_or_default();
        let mut any = false;
        for undo in outcomes
            .into_iter()
            .rev()
            .filter(|o| o.ok)
            .filter_map(|o| o.undo)
        {
            any = true;
            match batches
                .iter_mut()
                .find(|(a, acct, _)| *a == undo && acct == account)
            {
                Some((_, _, threads)) if !threads.contains(thread) => threads.push(thread.clone()),
                Some(_) => {}
                None => batches.push((undo, account.clone(), vec![thread.clone()])),
            }
        }
        if any {
            undone.push(e.id);
        }
    }
    for (action, account, threads) in batches {
        let targets = threads
            .into_iter()
            .map(|thread_id| ThreadRef {
                account_id: account.clone(),
                thread_id,
            })
            .collect();
        commands::apply_thread_action(state.inner().clone(), targets, action).await?;
    }
    let n = undone.len();
    let store = state.store.clone();
    blocking(move || {
        for id in undone {
            store.mark_rule_log_undone(id)?;
        }
        Ok(())
    })
    .await?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_draft_quotes_the_original_and_keeps_attachments() {
        let mut m = engine::tests::message("me@x.example", "m1", "t1");
        m.attachments = vec![
            penguin_core::AttachmentMeta {
                id: "a1".into(),
                filename: "receipt.pdf".into(),
                mime_type: "application/pdf".into(),
                size: 1234,
                content_id: None,
                inline: false,
            },
            penguin_core::AttachmentMeta {
                id: "logo".into(),
                filename: "logo.png".into(),
                mime_type: "image/png".into(),
                size: 10,
                content_id: Some("logo".into()),
                inline: true,
            },
        ];
        let d = forward_draft(&m, "books@y.example");
        assert_eq!(d.subject, "Fwd: Your trip m1");
        assert_eq!(d.to[0].email, "books@y.example");
        assert!(d.body_text.starts_with(
            "---------- Forwarded message ---------\nFrom: Uber Receipts <receipts@uber.example>\n"
        ));
        assert!(d.body_text.ends_with("Thanks for riding, total $12 (m1)"));
        assert_eq!(d.attachments.len(), 1);
        m.subject = "FWD: already".into();
        assert_eq!(forward_draft(&m, "b@y.example").subject, "FWD: already");
    }
}
