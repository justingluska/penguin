//! penguin-cli: drive the Penguin crates headlessly (no UI) against the same
//! database, OAuth client and Keychain entries as the app. See docs/CLI.md.
//!
//!   penguin-cli search "<query>" [--json] [--account <email>] [--profile <name>] [--limit <n>]
//!   penguin-cli thread <account> <threadId> [--json | --md] [--full] [--max-chars <n>]
//!   penguin-cli accounts [--json]
//!   penguin-cli attachments <account> <messageId> [--json]
//!   penguin-cli attachment <account> <messageId> <attachmentId> [--out <file>]
//!   penguin-cli draft create|update|list|delete …   (asks the running app; Settings → Developer → Agents)
//!   penguin-cli send <draftId> | send --to … (queued in the app's outbox; needs the send level)
//!   penguin-cli share-link <account> <messageId> <attachmentId> [--json]   (made by the app; Settings → Share links)
//!   penguin-cli labels [--json] [--account <email>] [--profile <name>]
//!   penguin-cli archive|unarchive|mark-read|mark-unread|star|unstar|trash|untrash|report-spam|not-spam
//!               |reply-later|clear-reply-later|unsnooze <account> <threadId>… [--stdin] [--json]
//!   penguin-cli add-label|remove-label <label> <account> <threadId>… [--stdin] [--json]
//!   penguin-cli snooze --until <time> <account> <threadId>… [--stdin] [--json]   (organizing: done by the app)
//!   penguin-cli mcp                       (stdio MCP server; enable in Settings → Developer → Agents)
//!   penguin-cli set-client <google-oauth-client.json>
//!   penguin-cli add-account
//!   penguin-cli sync <email> [--once]
//!   penguin-cli probe-cost <email> [--n <count>] [--variants a,b]   (quota-cost experiment; quit the app first)
//!
//! Streams: the payload (results, JSON, Markdown) goes to stdout; progress,
//! status and errors go to stderr, so `penguin-cli search … --json | jq`
//! stays clean. Output never contains ANSI color (NO_COLOR is always honored).
//! Exit codes: 0 ok, 1 error, 2 nothing found, 3 account needs sign-in,
//! 64 usage error, 69 Penguin isn't running, 77 not allowed (the agent
//! level in Settings → Developer → Agents, or share links for agents in
//! Settings → Share links).
//!
//! Drafting, sending, share links and organizing never happen in this process: they are
//! requests to the running Penguin app over its private socket
//! (agent/ipc.rs), which checks the level and uses its own credentials (and,
//! for share links, its own storage secret). penguin-cli never starts the
//! app.
//!
//! Env: PENGUIN_DATA_DIR=<dir> to use a throwaway database/config/cache;
//! PENGUIN_LOG=<filter> (e.g. `info`) for logs on stderr.

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use penguin_core::{Store, SyncStatus};
use penguin_desktop_lib::agent::attach::AttachmentArg;
use penguin_desktop_lib::agent::organize::{
    LabelArgs, SnoozeArgs, TargetsArgs, ThreadTarget, Until, ORGANIZE_TOOLS,
};
use penguin_desktop_lib::agent::output::{self as out, envelope, ThreadOptions};
use penguin_desktop_lib::agent::sharing::ShareLinkArgs;
use penguin_desktop_lib::agent::writes::{
    BodyFormat, DraftArgs, DraftIdArgs, ListDraftsArgs, UpdateDraftArgs,
};
use penguin_desktop_lib::agent::{self, audit::AuditLog, files, ipc, queries, AgentCtx};
use penguin_desktop_lib::error::{CmdError, ErrorCode};
use penguin_desktop_lib::logging;
use penguin_desktop_lib::ops::{self, Paths};
use penguin_desktop_lib::settings::AgentAccess;
use penguin_gmail::api::GmailClient;
use penguin_gmail::auth::{AuthManager, OAuthClientConfig};
use penguin_gmail::probe;
use penguin_gmail::sync::{SyncEngine, SyncObserver};

const USAGE: &str = "usage:
  penguin-cli search \"<query>\" [--json] [--account <email>] [--profile <name>] [--limit <n>]
  penguin-cli thread <account> <threadId> [--json | --md] [--full] [--max-chars <n>]
  penguin-cli accounts [--json]
  penguin-cli attachments <account> <messageId> [--json]
  penguin-cli attachment <account> <messageId> <attachmentId> [--out <file>]
  penguin-cli draft create [DRAFT FIELDS] [--json]
  penguin-cli draft update <draftId> [--account <email>] [DRAFT FIELDS] [--json]
  penguin-cli draft list [--account <email>] [--json]
  penguin-cli draft delete <draftId> [--account <email>] [--json]
  penguin-cli send <draftId> [--account <email>] [--json]
  penguin-cli send [DRAFT FIELDS] [--json]
  penguin-cli share-link <account> <messageId> <attachmentId> [--json]
  penguin-cli labels [--json] [--account <email>] [--profile <name>]
  penguin-cli archive|unarchive|mark-read|mark-unread|star|unstar <account> <threadId>… [TARGETS]
  penguin-cli add-label|remove-label <label> <account> <threadId>… [TARGETS]
  penguin-cli snooze --until <time> <account> <threadId>… [TARGETS]
  penguin-cli unsnooze|reply-later|clear-reply-later <account> <threadId>… [TARGETS]
  penguin-cli trash|untrash|report-spam|not-spam <account> <threadId>… [TARGETS]
  penguin-cli mcp
  penguin-cli set-client <google-oauth-client.json>
  penguin-cli add-account
  penguin-cli sync <email> [--once]
  penguin-cli probe-cost <email> [--n <count>] [--variants a,b]
  penguin-cli --version

DRAFT FIELDS: --from <email> --to <addr>… --cc <addr>… --bcc <addr>… --subject <text>
  --body <text> | --body-file <path|->  --markdown  --reply-to <messageId>  --attach <path>…
  (drafts and sends are done by the running Penguin app, at the level set in
  Settings → Developer → Agents; a send waits in its outbox before it goes)
share-link: the running Penguin uploads that attachment (or cid: picture) to your
  storage and prints a link anyone holding it can use until it expires; needs
  \"Read, organize and draft\" and \"Let agents (CLI and MCP) create share links\" in Settings → Share links
TARGETS: conversations as <account> <threadId>… (one account), or --stdin with one
  \"<account> <threadId>\" per line (up to 100; trash and report-spam 25, and 200 an hour);
  --json for the result. Organizing is done by the running Penguin at \"Read, organize
  and draft\"; nothing is deleted, and each result prints the commands that undo it.
  snooze --until: RFC 3339 with an offset (2026-10-01T09:00:00-04:00) or Unix ms

exit codes: 0 ok, 1 error, 2 nothing found, 3 needs sign-in, 64 usage,
  69 Penguin isn't running, 77 not allowed (agent level, share links for agents,
  the hourly trash and spam limits)
env: PENGUIN_DATA_DIR=<dir> (isolated data), PENGUIN_LOG=<filter> (stderr logs)
docs: docs/CLI.md";

const EXIT_ERROR: u8 = 1;
const EXIT_NOTHING_FOUND: u8 = 2;
const EXIT_NEEDS_REAUTH: u8 = 3;
const EXIT_USAGE: u8 = 64;
/// sysexits EX_UNAVAILABLE: Penguin isn't running.
const EXIT_UNAVAILABLE: u8 = 69;
/// sysexits EX_NOPERM: the agent level doesn't allow it.
const EXIT_DENIED: u8 = 77;

/// Legacy commands report plain strings (exit 1).
type CliResult = Result<(), String>;

struct Failure {
    exit: u8,
    code: &'static str,
    message: String,
    /// The command already wrote its payload (e.g. an empty result set), so
    /// no JSON error document follows it on stdout.
    payload_written: bool,
}

impl Failure {
    fn usage(message: impl Into<String>) -> Self {
        Failure {
            exit: EXIT_USAGE,
            code: "invalidInput",
            message: message.into(),
            payload_written: false,
        }
    }
    /// Ran fine, found nothing; the (empty) payload is already on stdout.
    fn nothing_after_output(message: impl Into<String>) -> Self {
        Failure {
            exit: EXIT_NOTHING_FOUND,
            code: "notFound",
            message: message.into(),
            payload_written: true,
        }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure {
            exit: EXIT_ERROR,
            code: "other",
            message,
            payload_written: false,
        }
    }
}

impl From<CmdError> for Failure {
    fn from(e: CmdError) -> Self {
        let (exit, code) = match e.code {
            ErrorCode::NotFound => (EXIT_NOTHING_FOUND, "notFound"),
            ErrorCode::NeedsReauth => (EXIT_NEEDS_REAUTH, "needsReauth"),
            ErrorCode::InvalidInput => (EXIT_USAGE, "invalidInput"),
            ErrorCode::NotConfigured => (EXIT_ERROR, "notConfigured"),
            ErrorCode::Network => (EXIT_ERROR, "network"),
            ErrorCode::Cancelled => (EXIT_ERROR, "cancelled"),
            ErrorCode::PermissionDenied => (EXIT_DENIED, "permissionDenied"),
            ErrorCode::Unavailable => (EXIT_UNAVAILABLE, "unavailable"),
            ErrorCode::Other => (EXIT_ERROR, "other"),
        };
        Failure {
            exit,
            code,
            message: e.message,
            payload_written: false,
        }
    }
}

type Outcome = Result<(), Failure>;

#[tokio::main]
async fn main() -> ExitCode {
    logging::init_cli();
    penguin_gmail::set_app_version(penguin_desktop_lib::VERSION);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let json = rest.iter().any(|a| a == "--json");
    let result: Outcome = match args.first().map(String::as_str) {
        Some("search") => search(rest),
        Some("thread") => thread(rest),
        Some("accounts") => accounts(rest),
        Some("attachments") => list_attachments(rest),
        Some("attachment") => attachment(rest).await,
        Some("draft") => draft(rest).await,
        Some("send") => send(rest).await,
        Some("share-link") => share_link(rest).await,
        Some("labels") => labels(rest),
        Some(cmd) if organize_tool(cmd).is_some() => organize(cmd, rest).await,
        Some("mcp") => mcp().await,
        Some("set-client") => set_client(rest).map_err(Failure::from),
        Some("add-account") => add_account().await.map_err(Failure::from),
        Some("sync") => sync(rest).await.map_err(Failure::from),
        Some("probe-cost") => probe_cost(rest).await.map_err(Failure::from),
        Some("-V" | "--version" | "version") => {
            println!("penguin-cli {}", penguin_desktop_lib::VERSION);
            Ok(())
        }
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(Failure::usage(USAGE)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            if json && !f.payload_written {
                // Scripts parsing stdout get a typed error too.
                let v = serde_json::json!({
                    "schemaVersion": out::SCHEMA_VERSION,
                    "kind": "error",
                    "data": { "code": f.code, "message": f.message },
                });
                println!("{v}");
            }
            eprintln!("{}", f.message);
            ExitCode::from(f.exit)
        }
    }
}

fn paths() -> Result<Paths, String> {
    let paths = Paths::resolve()?;
    paths
        .create_all()
        .map_err(|e| format!("creating app directories: {e}"))?;
    Ok(paths)
}

/// Read-only view of the app's database for query commands.
fn agent_ctx() -> Result<AgentCtx, Failure> {
    let paths = Paths::resolve()?;
    Ok(AgentCtx::open(paths)?)
}

fn open_store(paths: &Paths) -> Result<Store, String> {
    Store::open(&paths.db_path()).map_err(|e| format!("opening {}: {e}", paths.db_path().display()))
}

fn auth(paths: &Paths) -> Result<AuthManager, String> {
    match OAuthClientConfig::load(&paths.config_dir).map_err(|e| e.to_string())? {
        Some(config) => Ok(AuthManager::new(config)),
        None => Err(format!(
            "no Google OAuth client configured; run `penguin-cli set-client <json>` (expected at {})",
            paths.oauth_client_path().display()
        )),
    }
}

/// Value following `--flag`, if present.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// Positional arguments: everything that isn't a `--flag` or a flag's value.
fn positionals<'a>(args: &'a [String], valued_flags: &[&str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if valued_flags.contains(&a.as_str()) {
            skip = true;
        } else if !a.starts_with("--") {
            out.push(a.as_str());
        }
    }
    out
}

fn number<T: std::str::FromStr>(args: &[String], name: &str) -> Result<Option<T>, Failure> {
    flag(args, name)
        .map(|v| {
            v.parse()
                .map_err(|_| Failure::usage(format!("{name} expects a number, got {v}")))
        })
        .transpose()
}

fn print_json<T: serde::Serialize>(kind: &str, data: T) -> Outcome {
    let text = serde_json::to_string_pretty(&envelope(kind, data)).map_err(|e| e.to_string())?;
    println!("{text}");
    Ok(())
}

// ---------- query commands (read-only) ----------

fn search(args: &[String]) -> Outcome {
    let pos = positionals(args, &["--account", "--profile", "--limit"]);
    let [query] = pos.as_slice() else {
        return Err(Failure::usage(
            "usage: penguin-cli search \"<query>\" [--json] [--account <email>] [--profile <name>] [--limit <n>]",
        ));
    };
    let ctx = agent_ctx()?;
    let scope = ctx.scope(flag(args, "--account"), flag(args, "--profile"))?;
    let r = queries::search(&ctx, query, &scope, number(args, "--limit")?)?;
    let found = !r.hits.is_empty();

    if args.iter().any(|a| a == "--json") {
        print_json("search", &r)?;
    } else {
        if !r.chips.is_empty() {
            let chips: Vec<String> = r.chips.iter().map(|c| format!("[{}]", c.label)).collect();
            println!("chips: {}", chips.join(" "));
        }
        for h in &r.hits {
            let from = h.from.name.clone().unwrap_or_else(|| h.from.email.clone());
            println!(
                "{}  {:<24.24}  {:<60.60}  {}x  {}  {}",
                &h.date_iso[..10],
                from,
                h.subject,
                h.match_count,
                h.account_id,
                h.thread_id
            );
            println!("    {}", h.snippet);
        }
        for a in &r.attachments {
            println!(
                "attachment: {} ({}, {} bytes) in {}",
                a.attachment.filename, a.attachment.mime_type, a.attachment.size, a.thread_id
            );
        }
        for p in &r.people {
            println!(
                "person: {} <{}> ({} messages)",
                p.name.clone().unwrap_or_default(),
                p.email,
                p.message_count
            );
        }
        eprintln!(
            "{} hits in {:.1} ms over {} indexed messages",
            r.hits.len(),
            r.took_ms,
            r.indexed_messages
        );
    }
    if found {
        Ok(())
    } else {
        Err(Failure::nothing_after_output("no matches"))
    }
}

fn thread(args: &[String]) -> Outcome {
    let pos = positionals(args, &["--max-chars"]);
    let [account, thread_id] = pos.as_slice() else {
        return Err(Failure::usage(
            "usage: penguin-cli thread <account> <threadId> [--json | --md] [--full] [--max-chars <n>]",
        ));
    };
    let json = args.iter().any(|a| a == "--json");
    if json && args.iter().any(|a| a == "--md") {
        return Err(Failure::usage("choose one of --json or --md"));
    }
    let max: Option<usize> = number(args, "--max-chars")?;
    let ctx = agent_ctx()?;
    if json {
        let opts = ThreadOptions {
            include_full_text: args.iter().any(|a| a == "--full"),
            max_chars_per_message: max,
        };
        print_json("thread", queries::thread(&ctx, account, thread_id, opts)?)
    } else {
        // Markdown is the default human view (and what --md asks for).
        print!(
            "{}",
            queries::thread_markdown(&ctx, account, thread_id, max)?
        );
        Ok(())
    }
}

fn accounts(args: &[String]) -> Outcome {
    let ctx = agent_ctx()?;
    let r = queries::accounts(&ctx)?;
    if args.iter().any(|a| a == "--json") {
        return print_json("accounts", &r);
    }
    if r.accounts.is_empty() {
        eprintln!("no accounts; run `penguin-cli add-account`");
    }
    for a in &r.accounts {
        let cursor = ctx
            .store
            .get_sync_cursor(&a.id)
            .map_err(|e| e.to_string())?;
        println!(
            "{:<40} {:>9} messages  backfill {}  {}",
            a.email,
            a.indexed_messages,
            if cursor.backfill_done {
                "done"
            } else {
                "in progress"
            },
            a.display_name.clone().unwrap_or_default()
        );
    }
    for p in &r.profiles {
        println!("profile {:<20} {}", p.name, p.account_ids.join(", "));
    }
    eprintln!("db: {}", ctx.paths.db_path().display());
    Ok(())
}

fn labels(args: &[String]) -> Outcome {
    let pos = positionals(args, &["--account", "--profile"]);
    if !pos.is_empty() {
        return Err(Failure::usage(
            "usage: penguin-cli labels [--json] [--account <email>] [--profile <name>]",
        ));
    }
    let ctx = agent_ctx()?;
    let scope = ctx.scope(flag(args, "--account"), flag(args, "--profile"))?;
    let out = queries::labels(&ctx, &scope)?;
    if args.iter().any(|a| a == "--json") {
        return print_json("labels", &out);
    }
    for l in &out.labels {
        println!("{:<32} {:<28} {:<7} {}", l.account_id, l.name, l.kind, l.id);
    }
    if out.labels.is_empty() {
        return Err(Failure::nothing_after_output("no labels"));
    }
    Ok(())
}

// ---------- attachments (read) ----------

fn list_attachments(args: &[String]) -> Outcome {
    let pos = positionals(args, &[]);
    let [account, message_id] = pos.as_slice() else {
        return Err(Failure::usage(
            "usage: penguin-cli attachments <account> <messageId> [--json]",
        ));
    };
    let ctx = agent_ctx()?;
    let out = files::list_attachments(&ctx, account, message_id)?;
    if args.iter().any(|a| a == "--json") {
        return print_json("attachments", &out);
    }
    for a in &out.attachments {
        println!(
            "{}  {}  {}  {} bytes{}{}",
            a.id,
            a.filename,
            a.mime_type,
            a.size,
            if a.inline { "  [embedded picture]" } else { "" },
            if a.cached { "  [cached]" } else { "" }
        );
    }
    for url in &out.remote_images {
        println!("remote picture (not fetched): {url}");
    }
    if let Some(note) = &out.remote_images_note {
        eprintln!("{note}");
    }
    if out.attachments.is_empty() {
        return Err(Failure::nothing_after_output("no attachments"));
    }
    Ok(())
}

/// One attachment's bytes: from Penguin's cache, else downloaded by the
/// running app. Written to --out (never over an existing file) or to
/// stdout when it isn't a terminal.
async fn attachment(args: &[String]) -> Outcome {
    let pos = positionals(args, &["--out"]);
    let [account, message_id, attachment_id] = pos.as_slice() else {
        return Err(Failure::usage(
            "usage: penguin-cli attachment <account> <messageId> <attachmentId> [--out <file>]",
        ));
    };
    let out_path = flag(args, "--out").map(std::path::PathBuf::from);
    if out_path.is_none() && std::io::stdout().is_terminal() {
        return Err(Failure::usage(
            "that would print a file to the terminal; give --out <file>, or pipe it",
        ));
    }
    let ctx = agent_ctx()?;
    let meta = files::find(&ctx, account, message_id, attachment_id)?;
    let account = account.trim().to_lowercase();
    let bytes = match files::cached(&ctx, &account, message_id, &meta) {
        Some(b) => b,
        None => {
            eprintln!(
                "{} isn't downloaded yet; asking Penguin for it…",
                meta.filename
            );
            let reply = ipc::call(
                &ctx.paths,
                "cli",
                "fetch_attachment",
                serde_json::json!({"accountId": account, "messageId": message_id, "attachmentId": meta.id}),
            )
            .await?;
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(reply["dataBase64"].as_str().unwrap_or_default())
                .map_err(|e| format!("Penguin sent unreadable bytes: {e}"))?
        }
    };
    match out_path {
        Some(p) => {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p)
                .map_err(|e| format!("writing {}: {e}", p.display()))?;
            f.write_all(&bytes)
                .map_err(|e| format!("writing {}: {e}", p.display()))?;
            eprintln!(
                "saved {} ({} bytes) to {}",
                meta.filename,
                bytes.len(),
                p.display()
            );
        }
        None => {
            use std::io::Write;
            std::io::stdout()
                .write_all(&bytes)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ---------- drafts and sends (asked of the running app) ----------

/// Every value of a repeatable `--flag`; a value with commas and no angle
/// brackets or quotes is split (`--to a@x.example,b@y.example`).
fn flag_all(args: &[String], name: &str, split: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            if let Some(v) = args.get(i + 1) {
                if split && v.contains(',') && !v.contains('<') && !v.contains('"') {
                    out.extend(
                        v.split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(String::from),
                    );
                } else {
                    out.push(v.clone());
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

const DRAFT_FLAGS: [&str; 12] = [
    "--from",
    "--to",
    "--cc",
    "--bcc",
    "--subject",
    "--body",
    "--body-file",
    "--reply-to",
    "--attach",
    "--account",
    "--out",
    "--limit",
];

fn body_arg(args: &[String]) -> Result<Option<String>, Failure> {
    match (flag(args, "--body"), flag(args, "--body-file")) {
        (Some(_), Some(_)) => Err(Failure::usage("give --body or --body-file, not both")),
        (Some(b), None) => Ok(Some(b.to_string())),
        (None, Some("-")) => {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)
                .map_err(|e| format!("reading the body from stdin: {e}"))?;
            Ok(Some(s))
        }
        (None, Some(path)) => std::fs::read_to_string(path)
            .map(Some)
            .map_err(|e| Failure::usage(format!("reading {path}: {e}"))),
        (None, None) => Ok(None),
    }
}

/// `--attach` paths, made absolute against the current directory (the app
/// reads them; it has no idea where this shell is).
fn attach_args(args: &[String]) -> Result<Vec<AttachmentArg>, Failure> {
    let cwd = std::env::current_dir().map_err(|e| format!("current directory: {e}"))?;
    Ok(flag_all(args, "--attach", false)
        .into_iter()
        .map(|p| AttachmentArg {
            path: Some(cwd.join(p).display().to_string()),
            ..AttachmentArg::default()
        })
        .collect())
}

fn draft_args(args: &[String]) -> Result<DraftArgs, Failure> {
    Ok(DraftArgs {
        to: flag_all(args, "--to", true),
        cc: flag_all(args, "--cc", true),
        bcc: flag_all(args, "--bcc", true),
        subject: flag(args, "--subject").map(String::from),
        body: body_arg(args)?.unwrap_or_default(),
        format: args
            .iter()
            .any(|a| a == "--markdown")
            .then_some(BodyFormat::Markdown),
        from_account: flag(args, "--from").map(String::from),
        reply_to_message_id: flag(args, "--reply-to").map(String::from),
        attachments: attach_args(args)?,
    })
}

async fn ask_app(tool: &str, args: serde_json::Value) -> Result<serde_json::Value, Failure> {
    let paths = Paths::resolve()?;
    Ok(ipc::call(&paths, "cli", tool, args).await?)
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value, Failure> {
    Ok(serde_json::to_value(v).map_err(|e| e.to_string())?)
}

/// The app's answer: as is with --json, else one line per draft.
fn report(args: &[String], v: &serde_json::Value) -> Outcome {
    if args.iter().any(|a| a == "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(v).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    let d = &v["data"];
    match v["kind"].as_str() {
        Some("draft") => println!(
            "draft {} saved in {} (thread {}): {}",
            d["draftId"].as_str().unwrap_or_default(),
            d["accountId"].as_str().unwrap_or_default(),
            d["threadId"].as_str().unwrap_or_default(),
            d["subject"].as_str().unwrap_or_default()
        ),
        Some("drafts") => {
            for x in d["drafts"].as_array().into_iter().flatten() {
                println!(
                    "{}  {}  {}  {}{}",
                    x["draftId"].as_str().unwrap_or_default(),
                    x["accountId"].as_str().unwrap_or_default(),
                    x["updatedAtIso"].as_str().unwrap_or_default(),
                    x["subject"].as_str().unwrap_or_default(),
                    if x["queuedSend"].is_object() {
                        "  [queued to send]"
                    } else {
                        ""
                    }
                );
            }
        }
        Some("draftDeleted") => println!(
            "deleted draft {}",
            d["draftId"].as_str().unwrap_or_default()
        ),
        Some("shareLink") => {
            // The link alone on stdout (`penguin-cli share-link … | pbcopy`).
            println!("{}", d["url"].as_str().unwrap_or_default());
            eprintln!(
                "Anyone with this link can download {} ({} bytes) until {}.",
                d["name"].as_str().unwrap_or_default(),
                d["size"],
                d["expiresAtIso"].as_str().unwrap_or_default()
            );
        }
        Some("organized") => {
            let line = |status: &str, t: &serde_json::Value| {
                println!(
                    "{status:<10} {}  {}",
                    t["accountId"].as_str().unwrap_or_default(),
                    t["threadId"].as_str().unwrap_or_default()
                )
            };
            for t in d["changed"].as_array().into_iter().flatten() {
                line("changed", t);
            }
            for t in d["unchanged"].as_array().into_iter().flatten() {
                line("unchanged", t);
            }
            for t in d["notFound"].as_array().into_iter().flatten() {
                line("not found", t);
            }
            for t in d["failed"].as_array().into_iter().flatten() {
                line("failed", t);
            }
            let n = |k: &str| d[k].as_array().map_or(0, Vec::len);
            eprintln!(
                "{}: {} changed, {} already so, {} not found, {} refused by the provider.",
                d["tool"].as_str().unwrap_or_default().replace('_', "-"),
                n("changed"),
                n("unchanged"),
                n("notFound"),
                n("failed")
            );
            if let Some(note) = d["note"].as_str() {
                eprintln!("{note}");
            }
            for cmd in undo_commands(d) {
                eprintln!("undo: {cmd}");
            }
        }
        Some("sendQueued") => {
            println!(
                "queued draft {}: goes at {} (in {} s) unless cancelled in Penguin",
                d["draftId"].as_str().unwrap_or_default(),
                d["sendAtIso"].as_str().unwrap_or_default(),
                d["delaySeconds"]
            );
            eprintln!("It isn't sent yet: it waits in Penguin's outbox, and goes only while Penguin runs.");
        }
        _ => println!("{v}"),
    }
    Ok(())
}

async fn draft(args: &[String]) -> Outcome {
    const USAGE_DRAFT: &str =
        "usage: penguin-cli draft create|update <draftId>|list|delete <draftId> … (see --help)";
    let pos = positionals(args, &DRAFT_FLAGS);
    let v = match pos.as_slice() {
        ["create"] => ask_app("create_draft", to_json(&draft_args(args)?)?).await?,
        ["update", id] => {
            let d = draft_args(args)?;
            if d.reply_to_message_id.is_some() || d.from_account.is_some() {
                return Err(Failure::usage(
                    "an existing draft keeps its account and reply; --from and --reply-to are for create",
                ));
            }
            let has = |f: &str| args.iter().any(|a| a == f);
            let u = UpdateDraftArgs {
                account_id: flag(args, "--account").map(String::from),
                draft_id: id.to_string(),
                to: has("--to").then_some(d.to),
                cc: has("--cc").then_some(d.cc),
                bcc: has("--bcc").then_some(d.bcc),
                subject: d.subject,
                body: (has("--body") || has("--body-file")).then_some(d.body),
                format: d.format,
                attachments: has("--attach").then_some(d.attachments),
            };
            ask_app("update_draft", to_json(&u)?).await?
        }
        ["list"] => {
            let a = ListDraftsArgs {
                account: flag(args, "--account").map(String::from),
            };
            ask_app("list_drafts", to_json(&a)?).await?
        }
        ["delete", id] => {
            let a = DraftIdArgs {
                account_id: flag(args, "--account").map(String::from),
                draft_id: id.to_string(),
            };
            ask_app("delete_draft", to_json(&a)?).await?
        }
        _ => return Err(Failure::usage(USAGE_DRAFT)),
    };
    report(args, &v)
}

async fn send(args: &[String]) -> Outcome {
    let pos = positionals(args, &DRAFT_FLAGS);
    let v = match pos.as_slice() {
        [id] => {
            let a = DraftIdArgs {
                account_id: flag(args, "--account").map(String::from),
                draft_id: id.to_string(),
            };
            ask_app("send_draft", to_json(&a)?).await?
        }
        [] => ask_app("send_message", to_json(&draft_args(args)?)?).await?,
        _ => {
            return Err(Failure::usage(
                "usage: penguin-cli send <draftId> [--account <email>] | send --to … --subject … --body …",
            ))
        }
    };
    report(args, &v)
}

/// The organizing tool a command names (`mark-read` → `mark_read`).
fn organize_tool(cmd: &str) -> Option<&'static str> {
    let tool = cmd.replace('-', "_");
    ORGANIZE_TOOLS.iter().copied().find(|t| *t == tool)
}

/// Conversations from `<account> <threadId>…`, or with --stdin one
/// `<account> <threadId>` per line (blank lines and # comments skipped).
fn targets(pos: &[&str], stdin: bool) -> Result<Vec<ThreadTarget>, Failure> {
    let mut out = Vec::new();
    if let [account, threads @ ..] = pos {
        if threads.is_empty() {
            return Err(Failure::usage("name the threads after the account"));
        }
        out.extend(threads.iter().map(|t| ThreadTarget {
            account_id: account.to_string(),
            thread_id: t.to_string(),
        }));
    }
    if stdin {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
            .map_err(|e| format!("reading targets from stdin: {e}"))?;
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut words = line.split_whitespace();
            match (words.next(), words.next(), words.next()) {
                (Some(a), Some(t), None) => out.push(ThreadTarget {
                    account_id: a.to_string(),
                    thread_id: t.to_string(),
                }),
                _ => {
                    return Err(Failure::usage(format!(
                        "stdin line {}: expected \"<account> <threadId>\"",
                        n + 1
                    )))
                }
            }
        }
    }
    if out.is_empty() {
        return Err(Failure::usage(
            "name the conversations: <account> <threadId>…, or --stdin with \"<account> <threadId>\" lines",
        ));
    }
    Ok(out)
}

/// archive, add-label, snooze, trash…: asked of the running app, which
/// does them through its own action path at "Read, organize and draft".
async fn organize(cmd: &str, args: &[String]) -> Outcome {
    let tool = organize_tool(cmd).expect("checked by the caller");
    let pos = positionals(args, &["--until"]);
    let stdin = args.iter().any(|a| a == "--stdin");
    let v = match tool {
        "add_label" | "remove_label" => {
            let Some((label, rest)) = pos.split_first() else {
                return Err(Failure::usage(format!(
                    "usage: penguin-cli {cmd} <label> <account> <threadId>… [--stdin] [--json]"
                )));
            };
            let a = LabelArgs {
                targets: targets(rest, stdin)?,
                label: label.to_string(),
            };
            ask_app(tool, to_json(&a)?).await?
        }
        "snooze" => {
            let Some(until) = flag(args, "--until") else {
                return Err(Failure::usage(
                    "usage: penguin-cli snooze --until <RFC 3339 time | Unix ms> <account> <threadId>… [--stdin] [--json]",
                ));
            };
            let until = match until.parse::<i64>() {
                Ok(ms) => Until::Ms(ms),
                Err(_) => Until::Iso(until.to_string()),
            };
            let a = SnoozeArgs {
                targets: targets(&pos, stdin)?,
                until,
            };
            ask_app(tool, to_json(&a)?).await?
        }
        _ => {
            let a = TargetsArgs {
                targets: targets(&pos, stdin)?,
            };
            ask_app(tool, to_json(&a)?).await?
        }
    };
    report(args, &v)?;
    let d = &v["data"];
    let count = |k: &str| d[k].as_array().map_or(0, Vec::len);
    if count("failed") > 0 {
        return Err(Failure {
            exit: EXIT_ERROR,
            code: "other",
            message: format!(
                "the provider refused {} of them; Penguin put those back",
                count("failed")
            ),
            payload_written: true,
        });
    }
    if count("changed") + count("unchanged") == 0 {
        return Err(Failure::nothing_after_output(
            "none of those conversations are in Penguin",
        ));
    }
    Ok(())
}

/// The shell commands that run an `organized` result's undo calls.
fn undo_commands(d: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    for step in d["undo"].as_array().into_iter().flatten() {
        let tool = step["tool"].as_str().unwrap_or_default().replace('_', "-");
        let a = &step["arguments"];
        let mut by_account: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
        for t in a["targets"].as_array().into_iter().flatten() {
            by_account
                .entry(t["accountId"].as_str().unwrap_or_default())
                .or_default()
                .push(t["threadId"].as_str().unwrap_or_default());
        }
        for (account, threads) in by_account {
            let mut cmd = format!("penguin-cli {tool}");
            if let Some(l) = a["label"].as_str() {
                cmd.push_str(&format!(" {}", shell_word(l)));
            }
            if let Some(u) = a["until"].as_i64() {
                cmd.push_str(&format!(" --until {u}"));
            }
            cmd.push_str(&format!(" {}", shell_word(account)));
            for t in threads {
                cmd.push_str(&format!(" {}", shell_word(t)));
            }
            out.push(cmd);
        }
    }
    out
}

/// `s` quoted for a POSIX shell when it needs it.
fn shell_word(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "@._-+:/=".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// A share link for one attachment or embedded (cid:) picture, made by the
/// running app with the user's own storage. This process never sees the
/// storage or its secret.
async fn share_link(args: &[String]) -> Outcome {
    let pos = positionals(args, &[]);
    let [account, message_id, attachment_id] = pos.as_slice() else {
        return Err(Failure::usage(
            "usage: penguin-cli share-link <account> <messageId> <attachmentId> [--json]",
        ));
    };
    let a = ShareLinkArgs {
        account_id: account.to_string(),
        message_id: message_id.to_string(),
        attachment_id: attachment_id.to_string(),
    };
    let v = ask_app("create_share_link", to_json(&a)?).await?;
    report(args, &v)
}

/// stdio MCP server. stdout is the JSON-RPC channel: nothing else may print
/// there, so every message here goes to stderr.
async fn mcp() -> Outcome {
    let paths = Paths::resolve()?;
    let settings = penguin_desktop_lib::settings::Settings::load(
        &paths
            .config_dir
            .join(penguin_desktop_lib::settings::SETTINGS_FILE),
    );
    let level = settings.mcp.access;
    if level == AgentAccess::Off {
        return Err(Failure::from(
            "Penguin's MCP server is turned off. Enable it in Penguin → Settings → Developer → Agents, then restart your MCP client."
                .to_string(),
        ));
    }
    let ctx = AgentCtx::open(paths)?;
    let audit = Arc::new(AuditLog::open(agent::log_dir().as_deref()));
    if let Some(p) = audit.path() {
        eprintln!(
            "penguin mcp: agent level \"{}\"; tool calls are logged to {}",
            level.label(),
            p.display()
        );
    }
    agent::mcp::serve_stdio(ctx, audit).await?;
    Ok(())
}

// ---------- account and sync commands ----------

fn set_client(args: &[String]) -> CliResult {
    let file = args
        .first()
        .ok_or("usage: penguin-cli set-client <google-oauth-client.json>")?;
    let json = std::fs::read_to_string(file).map_err(|e| format!("reading {file}: {e}"))?;
    let paths = paths()?;
    OAuthClientConfig::save(&paths.config_dir, &json).map_err(|e| e.to_string())?;
    eprintln!(
        "saved OAuth client to {}",
        paths.config_dir.join(ops::OAUTH_CLIENT_FILE).display()
    );
    Ok(())
}

async fn add_account() -> CliResult {
    let paths = paths()?;
    let store = open_store(&paths)?;
    let auth = auth(&paths)?;
    let signed = auth
        .sign_in(|url: &str| {
            eprintln!("Opening your browser to sign in. If it doesn't open, visit:\n\n{url}\n");
            if let Err(e) = std::process::Command::new("open").arg(url).status() {
                eprintln!("could not launch the browser: {e}");
            }
        })
        .await
        .map_err(|e| e.to_string())?;
    let existing = store.list_accounts().map_err(|e| e.to_string())?;
    let account = ops::account_for_sign_in(&signed, &existing, ops::now_ms());
    store.upsert_account(&account).map_err(|e| e.to_string())?;
    eprintln!("added {} ({})", account.email, account.color);
    eprintln!("next: penguin-cli sync {}", account.email);
    Ok(())
}

/// Progress for `sync`: status lines on stderr.
struct PrintObserver {
    started: Instant,
}

impl SyncObserver for PrintObserver {
    fn status(&self, s: SyncStatus) {
        let total = s
            .total_estimate
            .map(|t| format!("/{t}"))
            .unwrap_or_default();
        let err = s.error.map(|e| format!("  error: {e}")).unwrap_or_default();
        eprintln!(
            "[{:>7.1}s] {:?}: {}{} indexed{}",
            self.started.elapsed().as_secs_f64(),
            s.phase,
            s.indexed,
            total,
            err
        );
    }
    fn mail_changed(&self, _account_id: &str, thread_ids: Vec<String>) {
        eprintln!(
            "[{:>7.1}s] {} threads changed",
            self.started.elapsed().as_secs_f64(),
            thread_ids.len()
        );
    }
}

/// Runs sync passes until the backfill is complete (resumable: Ctrl-C and
/// rerun picks up from the saved page token), or one pass with --once.
async fn sync(args: &[String]) -> CliResult {
    let email = args
        .first()
        .ok_or("usage: penguin-cli sync <email> [--once]")?;
    let once = args.iter().any(|a| a == "--once");
    let account_id = email.trim().to_lowercase();
    let paths = paths()?;
    let store = open_store(&paths)?;
    let Some(account) = store
        .list_accounts()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|a| a.id == account_id)
    else {
        return Err(format!(
            "unknown account {email}; run `penguin-cli add-account` first"
        ));
    };
    // The CLI's sync drives Gmail's engine directly (a developer tool);
    // other providers sync in the app.
    if account.provider != penguin_core::AccountProvider::Gmail {
        return Err(format!(
            "{email} is a {} account; `penguin-cli sync` only syncs Gmail accounts",
            account.provider.as_str()
        ));
    }
    let started = Instant::now();
    let engine = SyncEngine::new(
        store.clone(),
        auth(&paths)?,
        Arc::new(PrintObserver { started }),
    );
    let mut pass = 0u32;
    loop {
        pass += 1;
        engine
            .sync_once(&account_id)
            .await
            .map_err(|e| format!("sync pass {pass} failed: {e}"))?;
        let cursor = store
            .get_sync_cursor(&account_id)
            .map_err(|e| e.to_string())?;
        let count = store
            .count_messages(Some(&account_id))
            .map_err(|e| e.to_string())?;
        let rate = count as f64 / started.elapsed().as_secs_f64().max(0.001);
        eprintln!(
            "pass {pass}: {count} messages stored ({rate:.0}/s overall), backfill {}",
            if cursor.backfill_done {
                "done"
            } else {
                "continuing"
            }
        );
        if once || cursor.backfill_done {
            break;
        }
    }
    eprintln!("finished in {:.1}s", started.elapsed().as_secs_f64());
    Ok(())
}

/// Quota-cost experiment (see penguin_gmail::probe): fetches the same recent
/// messages once per format, one format per wall-clock minute, printing the
/// minutes so Cloud Monitoring's GetMessage total_query_cost can be matched.
/// Quit the app first; nothing is stored.
async fn probe_cost(args: &[String]) -> CliResult {
    let email = args
        .first()
        .ok_or("usage: penguin-cli probe-cost <email> [--n <count>]")?;
    let n = match flag(args, "--n") {
        Some(v) => v
            .parse()
            .map_err(|_| format!("--n expects a number, got {v}"))?,
        None => 60,
    };
    let only: Vec<String> = flag(args, "--variants")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if let Some(bad) = only
        .iter()
        .find(|v| !probe::variant_names().contains(&v.as_str()))
    {
        return Err(format!(
            "unknown variant {bad}; one of {}",
            probe::variant_names().join(", ")
        ));
    }
    let paths = paths()?;
    let client = GmailClient::new(auth(&paths)?, &email.trim().to_lowercase());
    println!("probing {n} messages, one variant per minute (~15 min); keep the Penguin app quit");
    probe::cost_probe(&client, n, &only, |r| {
        println!(
            "{:<18} minute_start_unix {}  calls {:>3}  messages {:>3}  failed {:>2}  avg {:>8} bytes/call",
            r.variant,
            r.minute_start_unix,
            r.calls,
            r.messages,
            r.failed,
            r.bytes / r.calls.max(1) as u64
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
