//! "Ask your inbox": deterministic answers from the local index
//! (`penguin_core::ask`). Read-only, local-only, no language model and no
//! network. Structured facts come from the background scanner
//! (`crate::extract`). When the on-device embedding model and vector index
//! are loaded and the index covers the mail (`AppState::semantic`, filled by
//! the semantic indexer), they're passed to `Store::ask_with` so topic
//! questions also retrieve and rank by meaning.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, Offset};
use penguin_core::ask::query::{model_instructions, model_prompt, MODEL_RESPONSE_TOKENS};
use penguin_core::ask::{AskAlias, AskAnswer, AskQuery, AskScope, AskSemantic, QueryDraft, QuerySource};
use tauri::State;

use crate::error::{CmdError, CmdResult};
use crate::state::{blocking, AppState};
use crate::summary::engine::{EngineEvent, GenRequest, SCHEMA_QUERY};
use crate::summary::Summarizer;

type AppStateRef<'a> = State<'a, Arc<AppState>>;
type SummarizerRef<'a> = State<'a, Arc<Summarizer>>;

/// The longest the model may take to read a question before the grammar's
/// answer stands.
const MODEL_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest question accepted; anything longer isn't a question.
const MAX_QUESTION: usize = 500;

/// Answer a question about your mail. `scope.accountIds` limits it to a
/// profile; `scope.person` carries the previous answer's person for "he" /
/// "they". Profile names are added as aliases ("when did we start with
/// Acme" where Acme is a profile).
#[tauri::command]
pub async fn ask(
    state: AppStateRef<'_>,
    question: String,
    scope: Option<AskScope>,
) -> CmdResult<AskAnswer> {
    let question = question.trim().to_string();
    if question.is_empty() || question.chars().count() > MAX_QUESTION {
        return Err(CmdError::invalid("question must be 1–500 characters"));
    }
    let scope = app_scope(&state, scope);
    let now = chrono::Utc::now();
    let offset = Local::now().offset().fix().local_minus_utc();
    let store = state.store.clone();
    // The same model and index search uses, from the first embedded mail on
    // (the rule search follows; newest mail is embedded first).
    let semantic = state.semantic();
    blocking(move || {
        let sem = semantic.as_ref().map(|h| AskSemantic {
            embedder: h.embedder.as_ref(),
            index: h.index.as_ref(),
        });
        Ok(store.ask_with(&question, &scope, now.timestamp_millis(), offset, sem)?)
    })
    .await
}

/// The scope as the app uses it: codes shown, profile names as aliases.
fn app_scope(state: &AppState, scope: Option<AskScope>) -> AskScope {
    let mut scope = scope.unwrap_or_default();
    // The app shows verification codes (the CLI and MCP never do).
    scope.reveal_codes = true;
    for p in state.settings.get().profiles {
        if !scope
            .aliases
            .iter()
            .any(|a| a.name.eq_ignore_ascii_case(&p.name))
        {
            scope.aliases.push(AskAlias {
                name: p.name,
                account_ids: p.account_ids,
            });
        }
    }
    scope
}

/// Read a question the grammar couldn't with Apple's on-device model
/// (Settings → AI → "Understand questions with Apple Intelligence"), then
/// answer the model's reading exactly. The model fills the `MailQuery`
/// schema only (guided generation); the reading is checked against the
/// schema, the date grammar and the mailbox before it runs, and the answer
/// is computed from local mail like any other. `None`: off, unavailable,
/// the model failed, or its reading didn't check out; the grammar's answer
/// stands. Logs outcome codes and timings only, never the question.
#[tauri::command]
pub async fn ask_understand(
    state: AppStateRef<'_>,
    summarizer: SummarizerRef<'_>,
    question: String,
    scope: Option<AskScope>,
) -> CmdResult<Option<AskAnswer>> {
    let question = question.trim().to_string();
    if question.is_empty() || question.chars().count() > MAX_QUESTION {
        return Err(CmdError::invalid("question must be 1–500 characters"));
    }
    if !state.settings.get().ask_with_ai {
        return Ok(None);
    }
    let s = summarizer.inner().clone();
    let availability = {
        let s = s.clone();
        blocking(move || Ok(s.engine.availability())).await?
    };
    if !availability.available {
        return Ok(None);
    }
    let today = Local::now().format("%a %Y-%m-%d").to_string();
    let req = GenRequest {
        instructions: model_instructions(&today),
        prompt: model_prompt(&question),
        guided: true,
        permissive: false,
        stream: false,
        max_response_tokens: MODEL_RESPONSE_TOKENS,
        // The same question, the same reading.
        temperature: Some(0.0),
        schema: Some(SCHEMA_QUERY.into()),
    };
    let started = Instant::now();
    let value = {
        let _turn = s.gate.lock().await;
        let mut generation = s.engine.start(&req);
        let wait = async {
            while let Some(event) = generation.events.recv().await {
                match event {
                    EngineEvent::Snapshot(_) => {}
                    EngineEvent::Done(v) => return Ok(v),
                    EngineEvent::Error(e) => return Err(format!("{:?}", e.code)),
                }
            }
            Err("closed".to_string())
        };
        match tokio::time::timeout(MODEL_TIMEOUT, wait).await {
            Ok(r) => r,
            Err(_) => {
                (generation.cancel)();
                Err("timeout".into())
            }
        }
    };
    let ms = started.elapsed().as_millis() as u64;
    let draft: QueryDraft = match value.map(serde_json::from_value) {
        Ok(Ok(d)) => d,
        Ok(Err(_)) => {
            tracing::info!(ms, outcome = "decoding", "ask: model reading");
            return Ok(None);
        }
        Err(code) => {
            tracing::info!(ms, outcome = %code, "ask: model reading");
            return Ok(None);
        }
    };
    let scope = app_scope(&state, scope);
    let store = state.store.clone();
    let now = chrono::Utc::now().timestamp_millis();
    let offset = Local::now().offset().fix().local_minus_utc();
    let answer = blocking(move || Ok(store.ask_draft(&question, draft, &scope, now, offset)?)).await?;
    tracing::info!(ms, used = answer.is_some(), "ask: model reading");
    Ok(answer)
}

/// Answer a reading the user edited in the answer's chips.
#[tauri::command]
pub async fn ask_query(
    state: AppStateRef<'_>,
    question: String,
    query: AskQuery,
    scope: Option<AskScope>,
) -> CmdResult<AskAnswer> {
    let question: String = question.trim().chars().take(MAX_QUESTION).collect();
    let scope = app_scope(&state, scope);
    let store = state.store.clone();
    let now = chrono::Utc::now().timestamp_millis();
    let offset = Local::now().offset().fix().local_minus_utc();
    blocking(move || Ok(store.ask_query(&question, &query, QuerySource::Edited, &scope, now, offset)?)).await
}
