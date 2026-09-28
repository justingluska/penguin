//! Tracing setup. Logs carry ids, counts, phases and error text only — never
//! message bodies, subjects, or tokens (see CLAUDE.md).

use std::fs::OpenOptions;
use std::path::Path;
use std::sync::Mutex;

use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::EnvFilter;

pub const LOG_FILE: &str = "penguin.log";
/// Past this size the log is rotated to penguin.log.1 at startup.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const DEFAULT_FILTER: &str =
    "warn,penguin_desktop_lib=info,penguin_desktop=info,penguin_cli=info,penguin_core=info,penguin_gmail=info,penguin_render=info";

fn filter() -> EnvFilter {
    EnvFilter::try_from_env("PENGUIN_LOG").unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
}

/// App logging: `<log dir>/penguin.log`, plus stderr in debug builds.
/// Falls back to stderr only if the log file can't be opened.
pub fn init_app(log_dir: Option<&Path>) {
    let file = log_dir.and_then(|dir| {
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join(LOG_FILE);
        if std::fs::metadata(&path)
            .map(|m| m.len() > MAX_LOG_BYTES)
            .unwrap_or(false)
        {
            let _ = std::fs::rename(&path, dir.join(format!("{LOG_FILE}.1")));
        }
        OpenOptions::new().create(true).append(true).open(path).ok()
    });
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter())
        .with_ansi(false);
    let result = match file {
        Some(file) if cfg!(debug_assertions) => builder
            .with_writer(Mutex::new(file).and(std::io::stderr))
            .try_init(),
        Some(file) => builder.with_writer(Mutex::new(file)).try_init(),
        None => builder.with_writer(std::io::stderr).try_init(),
    };
    if result.is_err() {
        eprintln!("penguin: tracing was already initialised");
    }
    install_panic_hook();
}

/// CLI logging: stderr, only when PENGUIN_LOG is set (stdout stays clean).
pub fn init_cli() {
    if std::env::var_os("PENGUIN_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_writer(std::io::stderr)
            .try_init();
    }
    install_panic_hook();
}

/// Log every panic (payload text + file:line + thread) through tracing so it
/// lands in penguin.log even when something upstream catches the unwind,
/// then run the default hook. Payloads are panic messages, not mail content.
fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
            tracing::error!(
                panic = %penguin_gmail::panic_message(info.payload()),
                location = location.as_deref().unwrap_or("unknown"),
                thread = std::thread::current().name().unwrap_or("unnamed"),
                "panic"
            );
            default(info);
        }));
    });
}
