//! Secondary windows: one conversation (`thread-<hash>`) or one message
//! being written (`compose-<n>`), next to the main window.
//!
//! Every window loads the same index.html; the query names what it shows
//! (`?window=thread&label=…&account=…&thread=…`, `?window=compose&label=…
//! [&account=…][&draft=…]`) and the UI renders a small shell for it instead of
//! the mail shell (src/app/windowShell.tsx). The capabilities give these
//! labels only the commands a conversation or a composer needs
//! (capabilities/windows.json); the CSP and the sandboxed message iframe are
//! the same as the main window's.
//!
//! A composer that moves out of the main window (Pop out, Undo of a send
//! from a compose window) hands its whole editor state over as a seed: it is
//! kept here, in memory only, for the new window's label and taken once by
//! that window (`take_window_seed`, which answers only the calling window).
//!
//! All Penguin windows share one tabbing identifier, so macOS offers its own
//! tab bar, Window → Merge All Windows / Move Tab to New Window, and follows
//! the "Prefer tabs" system setting. The main window gets it from
//! tauri.conf.json (`tabbingIdentifier`), these from the builder.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::error::{CmdError, CmdResult};

/// The label of the one main window (tauri.conf.json).
pub const MAIN: &str = "main";
/// Shared by every Penguin window (and tauri.conf.json's main window), so
/// macOS groups them as tabs of one app.
pub const TABBING_ID: &str = "co.gluska.penguin.windows";
pub const THREAD_PREFIX: &str = "thread-";
pub const COMPOSE_PREFIX: &str = "compose-";

/// Longest account, thread or draft id accepted (real ones are far shorter).
const MAX_ID: usize = 512;
/// Longest window title kept (characters).
const MAX_TITLE: usize = 200;
/// Largest composer seed (JSON bytes): a draft's files are capped at 25 MB,
/// which is ~34 MB as base64.
const MAX_SEED_BYTES: usize = 64 * 1024 * 1024;
/// A seed nobody took (the window never loaded) is dropped after this.
const SEED_TTL: Duration = Duration::from_secs(300);
/// Each new window steps this far right and down from the main window.
const CASCADE_STEP: f64 = 28.0;

/// `open_window`: what to show in a window of its own.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WindowRequest {
    /// One conversation. A second request for the same one focuses its window.
    #[serde(rename_all = "camelCase")]
    Thread {
        account_id: String,
        thread_id: String,
        /// The subject, for the title until the thread has loaded.
        #[serde(default)]
        title: Option<String>,
    },
    /// A composer: a new message (from `account_id` when given), a saved
    /// draft (`account_id` + `draft_id`), and/or the editor state handed
    /// over by another window (`seed`).
    #[serde(rename_all = "camelCase")]
    Compose {
        #[serde(default)]
        account_id: Option<String>,
        #[serde(default)]
        draft_id: Option<String>,
        #[serde(default)]
        seed: Option<serde_json::Value>,
        #[serde(default)]
        title: Option<String>,
    },
}

/// `open_window`'s answer.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenedWindow {
    pub label: String,
    /// The conversation already had a window; it was brought to the front.
    pub existing: bool,
}

/// Which kind of window a label names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Main,
    Thread,
    Compose,
    Other,
}

pub fn kind_of(label: &str) -> Kind {
    if label == MAIN {
        Kind::Main
    } else if label.starts_with(THREAD_PREFIX) {
        Kind::Thread
    } else if label.starts_with(COMPOSE_PREFIX) {
        Kind::Compose
    } else {
        Kind::Other
    }
}

/// A conversation window or a composer window.
pub fn is_secondary(label: &str) -> bool {
    matches!(kind_of(label), Kind::Thread | Kind::Compose)
}

fn check_id(what: &str, id: &str) -> CmdResult<()> {
    if id.is_empty() || id.len() > MAX_ID || id.chars().any(char::is_control) {
        return Err(CmdError::invalid(format!("open_window: invalid {what}")));
    }
    Ok(())
}

fn check_opt_id(what: &str, id: &Option<String>) -> CmdResult<()> {
    match id {
        Some(id) => check_id(what, id),
        None => Ok(()),
    }
}

/// Reject malformed requests before anything is built.
pub fn validate(req: &WindowRequest) -> CmdResult<()> {
    match req {
        WindowRequest::Thread {
            account_id,
            thread_id,
            ..
        } => {
            check_id("account id", account_id)?;
            check_id("thread id", thread_id)
        }
        WindowRequest::Compose {
            account_id,
            draft_id,
            seed,
            ..
        } => {
            check_opt_id("account id", account_id)?;
            check_opt_id("draft id", draft_id)?;
            if draft_id.is_some() && account_id.is_none() {
                return Err(CmdError::invalid(
                    "open_window: a draft needs its account id",
                ));
            }
            if let Some(seed) = seed {
                if !seed.is_object() {
                    return Err(CmdError::invalid("open_window: the seed must be an object"));
                }
                let size = serde_json::to_vec(seed)
                    .map(|v| v.len())
                    .unwrap_or(usize::MAX);
                if size > MAX_SEED_BYTES {
                    return Err(CmdError::invalid(
                        "open_window: the draft is too large to move",
                    ));
                }
            }
            Ok(())
        }
    }
}

/// A title safe to show: one line, no control characters, at most 200 chars.
pub fn clean_title(title: Option<&str>, fallback: &str) -> String {
    let t: String = title
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if t.is_empty() {
        return fallback.to_string();
    }
    t.chars().take(MAX_TITLE).collect()
}

/// 64-bit FNV-1a: a stable, dependency-free hash for window labels (not
/// security relevant; a label only has to be unique per conversation).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The one window label for a conversation (`thread-<16 hex>`). Must match
/// `threadWindowLabel` in src/lib/windowRoute.ts (the browser mock).
pub fn thread_label(account_id: &str, thread_id: &str) -> String {
    let mut key = Vec::with_capacity(account_id.len() + thread_id.len() + 1);
    key.extend_from_slice(account_id.as_bytes());
    key.push(0);
    key.extend_from_slice(thread_id.as_bytes());
    format!("{THREAD_PREFIX}{:016x}", fnv1a(&key))
}

/// The page a window loads: index.html with the query the UI reads at boot
/// (src/lib/windowRoute.ts `parseWindowRoute`).
pub fn page_path(label: &str, req: &WindowRequest) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    match req {
        WindowRequest::Thread {
            account_id,
            thread_id,
            ..
        } => {
            q.append_pair("window", "thread");
            q.append_pair("label", label);
            q.append_pair("account", account_id);
            q.append_pair("thread", thread_id);
        }
        WindowRequest::Compose {
            account_id,
            draft_id,
            ..
        } => {
            q.append_pair("window", "compose");
            q.append_pair("label", label);
            // With a draft: the draft's account. Without: write from it.
            if let Some(a) = account_id {
                q.append_pair("account", a);
            }
            if let Some(d) = draft_id {
                q.append_pair("draft", d);
            }
        }
    }
    format!("index.html?{}", q.finish())
}

/// A rectangle in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Where the `n`-th open secondary window goes: stepped down and right from
/// the main window (eight steps, then round again), kept inside the screen.
pub fn cascade(anchor: Rect, screen: Option<Rect>, size: (f64, f64), n: usize) -> (f64, f64) {
    let step = CASCADE_STEP * (1 + n % 8) as f64;
    let (mut x, mut y) = (anchor.x + step, anchor.y + step);
    if let Some(s) = screen {
        x = x.min(s.x + s.w - size.0).max(s.x);
        y = y.min(s.y + s.h - size.1).max(s.y);
    }
    (x, y)
}

/// Seeds for composer windows, by label (see the module comment).
#[derive(Default)]
pub struct Seeds {
    inner: Mutex<HashMap<String, (Instant, serde_json::Value)>>,
}

impl Seeds {
    pub fn put(&self, label: &str, seed: serde_json::Value) {
        let mut m = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        m.retain(|_, (at, _)| at.elapsed() < SEED_TTL);
        m.insert(label.to_string(), (Instant::now(), seed));
    }

    pub fn take(&self, label: &str) -> Option<serde_json::Value> {
        let mut m = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        m.remove(label)
            .filter(|(at, _)| at.elapsed() < SEED_TTL)
            .map(|(_, v)| v)
    }

    pub fn forget(&self, label: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(label);
    }
}

/// Numbers compose windows.
static COMPOSE_SEQ: AtomicU64 = AtomicU64::new(1);

fn logical_rect<R: Runtime>(w: &WebviewWindow<R>) -> Option<Rect> {
    let scale = w.scale_factor().ok()?;
    let pos = w.outer_position().ok()?.to_logical::<f64>(scale);
    let size = w.outer_size().ok()?.to_logical::<f64>(scale);
    Some(Rect {
        x: pos.x,
        y: pos.y,
        w: size.width,
        h: size.height,
    })
}

fn screen_of<R: Runtime>(w: &WebviewWindow<R>) -> Option<Rect> {
    let m = w.current_monitor().ok()??;
    let scale = m.scale_factor();
    let area = m.work_area();
    let pos = area.position.to_logical::<f64>(scale);
    let size = area.size.to_logical::<f64>(scale);
    Some(Rect {
        x: pos.x,
        y: pos.y,
        w: size.width,
        h: size.height,
    })
}

fn focus<R: Runtime>(w: &WebviewWindow<R>) {
    for r in [w.show(), w.unminimize(), w.set_focus()] {
        if let Err(e) = r {
            tracing::warn!(error = %e, "could not bring a window forward");
        }
    }
}

/// Open (or, for a conversation that has one, focus) a window.
pub fn open<R: Runtime>(app: &AppHandle<R>, req: WindowRequest) -> CmdResult<OpenedWindow> {
    validate(&req)?;
    let (label, title, size, min) = match &req {
        WindowRequest::Thread {
            account_id,
            thread_id,
            title,
        } => (
            thread_label(account_id, thread_id),
            clean_title(title.as_deref(), "Conversation"),
            (920.0, 840.0),
            (560.0, 420.0),
        ),
        WindowRequest::Compose { title, .. } => {
            let mut label;
            loop {
                label = format!(
                    "{COMPOSE_PREFIX}{}",
                    COMPOSE_SEQ.fetch_add(1, Ordering::Relaxed)
                );
                if app.get_webview_window(&label).is_none() {
                    break;
                }
            }
            (
                label,
                clean_title(title.as_deref(), "New Message"),
                (780.0, 720.0),
                (520.0, 420.0),
            )
        }
    };
    if let Some(w) = app.get_webview_window(&label) {
        focus(&w);
        return Ok(OpenedWindow {
            label,
            existing: true,
        });
    }
    if let WindowRequest::Compose {
        seed: Some(seed), ..
    } = &req
    {
        if let Some(seeds) = app.try_state::<Seeds>() {
            seeds.put(&label, seed.clone());
        }
    }

    let open_now = app
        .webview_windows()
        .keys()
        .filter(|l| is_secondary(l))
        .count();
    let main = app.get_webview_window(MAIN);
    let anchor = main.as_ref().and_then(logical_rect);
    let screen = main.as_ref().and_then(screen_of);

    let mut b =
        WebviewWindowBuilder::new(app, &label, WebviewUrl::App(page_path(&label, &req).into()))
            .title(&title)
            .inner_size(size.0, size.1)
            .min_inner_size(min.0, min.1)
            // Like the main window (tauri.conf.json dragDropEnabled: false): the
            // composer takes dropped files as File objects.
            .disable_drag_drop_handler()
            .focused(true);
    if let Some(a) = anchor {
        let (x, y) = cascade(a, screen, size, open_now);
        b = b.position(x, y);
    } else {
        b = b.center();
    }
    #[cfg(target_os = "macos")]
    {
        b = b
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .tabbing_identifier(TABBING_ID);
    }
    b.build().map_err(|e| {
        if let Some(seeds) = app.try_state::<Seeds>() {
            seeds.forget(&label);
        }
        tracing::warn!(error = %e, "could not open a window");
        CmdError::other(format!("Couldn't open the window: {e}"))
    })?;
    Ok(OpenedWindow {
        label,
        existing: false,
    })
}

/// Open a conversation or a composer in a window of its own (async: building
/// a window from a synchronous command can deadlock the event loop).
#[tauri::command]
pub async fn open_window(app: AppHandle, request: WindowRequest) -> CmdResult<OpenedWindow> {
    open(&app, request)
}

/// The editor state handed to the calling composer window, once.
#[tauri::command]
pub fn take_window_seed(
    window: WebviewWindow,
    seeds: tauri::State<'_, Seeds>,
) -> Option<serde_json::Value> {
    seeds.take(window.label())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_parse_from_the_ui_shape() {
        let t: WindowRequest = serde_json::from_value(json!({
            "kind": "thread", "accountId": "a@x.example", "threadId": "t1", "title": "Hi"
        }))
        .unwrap();
        assert_eq!(
            t,
            WindowRequest::Thread {
                account_id: "a@x.example".into(),
                thread_id: "t1".into(),
                title: Some("Hi".into())
            }
        );
        let c: WindowRequest = serde_json::from_value(json!({ "kind": "compose" })).unwrap();
        assert!(matches!(
            c,
            WindowRequest::Compose {
                seed: None,
                draft_id: None,
                ..
            }
        ));
        assert!(serde_json::from_value::<WindowRequest>(json!({ "kind": "settings" })).is_err());
    }

    #[test]
    fn validation_rejects_bad_ids_and_seeds() {
        let thread = |a: &str, t: &str| WindowRequest::Thread {
            account_id: a.into(),
            thread_id: t.into(),
            title: None,
        };
        assert!(validate(&thread("a", "t")).is_ok());
        assert!(validate(&thread("", "t")).is_err());
        assert!(validate(&thread("a", "t\n1")).is_err());
        assert!(validate(&thread("a", &"x".repeat(MAX_ID + 1))).is_err());
        let compose = |a: Option<&str>, d: Option<&str>, seed: Option<serde_json::Value>| {
            WindowRequest::Compose {
                account_id: a.map(Into::into),
                draft_id: d.map(Into::into),
                seed,
                title: None,
            }
        };
        assert!(validate(&compose(None, None, None)).is_ok());
        assert!(validate(&compose(
            Some("a"),
            Some("r-1"),
            Some(json!({"key": "new"}))
        ))
        .is_ok());
        assert!(
            validate(&compose(None, Some("r-1"), None)).is_err(),
            "a draft needs its account"
        );
        assert!(validate(&compose(None, None, Some(json!("text")))).is_err());
    }

    #[test]
    fn titles_are_one_clean_line() {
        assert_eq!(
            clean_title(Some("  Lunch\non\tFriday  "), "x"),
            "Lunch on Friday"
        );
        assert_eq!(clean_title(Some("\u{7}"), "Conversation"), "Conversation");
        assert_eq!(clean_title(None, "New Message"), "New Message");
        assert_eq!(
            clean_title(Some(&"é".repeat(500)), "x").chars().count(),
            MAX_TITLE
        );
    }

    #[test]
    fn thread_labels_are_stable_and_distinct() {
        let a = thread_label("a@x.example", "t1");
        assert_eq!(a, thread_label("a@x.example", "t1"));
        assert_ne!(a, thread_label("a@x.example", "t2"));
        // The separator keeps ("ab", "c") and ("a", "bc") apart.
        assert_ne!(thread_label("ab", "c"), thread_label("a", "bc"));
        assert!(a.starts_with(THREAD_PREFIX) && a.len() == THREAD_PREFIX.len() + 16);
        // Same value as the UI's copy (tests/windows.test.ts pins it too).
        assert_eq!(
            thread_label("sam@northwind.example", "t-100"),
            "thread-efe6d0e3069c6e6b"
        );
    }

    #[test]
    fn page_paths_carry_the_route() {
        let t = WindowRequest::Thread {
            account_id: "a+b@x.example".into(),
            thread_id: "t 1".into(),
            title: None,
        };
        assert_eq!(
            page_path("thread-1", &t),
            "index.html?window=thread&label=thread-1&account=a%2Bb%40x.example&thread=t+1"
        );
        let c = WindowRequest::Compose {
            account_id: Some("a@x.example".into()),
            draft_id: Some("r-9".into()),
            seed: None,
            title: None,
        };
        assert_eq!(
            page_path("compose-2", &c),
            "index.html?window=compose&label=compose-2&account=a%40x.example&draft=r-9"
        );
        let blank = WindowRequest::Compose {
            account_id: Some("a".into()),
            draft_id: None,
            seed: None,
            title: None,
        };
        assert_eq!(
            page_path("compose-3", &blank),
            "index.html?window=compose&label=compose-3&account=a"
        );
    }

    #[test]
    fn windows_cascade_inside_the_screen() {
        let main = Rect {
            x: 100.0,
            y: 50.0,
            w: 1440.0,
            h: 900.0,
        };
        assert_eq!(cascade(main, None, (900.0, 800.0), 0), (128.0, 78.0));
        assert_eq!(cascade(main, None, (900.0, 800.0), 1), (156.0, 106.0));
        assert_eq!(cascade(main, None, (900.0, 800.0), 8), (128.0, 78.0));
        let screen = Rect {
            x: 0.0,
            y: 25.0,
            w: 1000.0,
            h: 800.0,
        };
        let (x, y) = cascade(main, Some(screen), (900.0, 800.0), 3);
        assert_eq!((x, y), (100.0, 25.0));
    }

    #[test]
    fn labels_have_kinds() {
        assert_eq!(kind_of("main"), Kind::Main);
        assert_eq!(kind_of("thread-00ff"), Kind::Thread);
        assert_eq!(kind_of("compose-3"), Kind::Compose);
        assert_eq!(kind_of("other"), Kind::Other);
        assert!(is_secondary("compose-3") && !is_secondary("main"));
    }

    #[test]
    fn the_main_window_tabs_with_the_others() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let main = &conf["app"]["windows"][0];
        assert_eq!(main["label"], MAIN);
        assert_eq!(main["tabbingIdentifier"], TABBING_ID);
    }

    #[test]
    fn seeds_are_taken_once() {
        let s = Seeds::default();
        s.put("compose-1", json!({"key": "new"}));
        assert_eq!(s.take("compose-2"), None);
        assert_eq!(s.take("compose-1"), Some(json!({"key": "new"})));
        assert_eq!(s.take("compose-1"), None);
    }
}
