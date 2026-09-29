//! Spell checking while typing, in every window (Settings → Compose "Check
//! spelling while typing", Edit ▸ Spelling and Grammar).
//!
//! Why it didn't underline. `spellcheck="true"` on the composer only makes a
//! field *eligible*; whether WKWebView checks as you type is WebKit's
//! app-wide continuous-spell-checking state, which it reads once from the
//! app's user default `WebContinuousSpellCheckingEnabled` (TextCheckerMac.mm).
//! Safari and Mail set it and offer Edit ▸ Spelling and Grammar ▸ Check
//! Spelling While Typing; Penguin (like any Tauri/wry app) did neither, so
//! the default was absent, WebKit read NO, and nothing could turn it on.
//!
//! What Penguin does:
//! - At launch, before the first web view, [`prime`] writes the default on
//!   (grammar left off) when the app has none yet, so WebKit starts with
//!   checking on and every window's web process is created with it.
//! - The setting is the source of truth. On every settings change [`apply`]
//!   compares it with WebKit's state and, when they differ, pushes the new
//!   state to every web view (src/mac/spelling.rs `push`: WebKit sends a
//!   change only to the process of the view it was made on). WebKit writes
//!   the default back itself, so the next launch starts right.
//! - The Edit menu's check items change the setting (src/app_menu.rs), so
//!   menu and Settings stay in step; Show Spelling and Grammar and Check
//!   Document Now go down the responder chain to the focused web view.
//! - A page that loads after a live change (a new compose window whose web
//!   process may predate it) gets the state pushed too ([`page_loaded`]).
//!
//! Elsewhere these are no-ops (the Linux build only runs tests).

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Runtime, Webview};

use crate::settings::Settings;

/// The setting changed WebKit's state after launch: web processes created
/// before that may hold the old one, so new pages get it pushed.
static CHANGED_LIVE: AtomicBool = AtomicBool::new(false);

/// What the settings ask for: (spelling while typing, grammar with spelling).
pub fn wanted(s: &Settings) -> (bool, bool) {
    (s.check_spelling, s.check_grammar)
}

/// At launch, before any web view exists: give WebKit's text checker its
/// starting state (on) when the app has never stored one.
pub fn prime() {
    #[cfg(target_os = "macos")]
    {
        use crate::mac::spelling::{has_default, set_default, Check};
        if !has_default(Check::Spelling) {
            set_default(Check::Spelling, crate::settings::CHECK_SPELLING_DEFAULT);
        }
    }
}

/// Bring WebKit's state in line with the settings, in every window. Cheap
/// when nothing changed (two user-default reads on the main thread).
pub fn apply<R: Runtime>(app: &AppHandle<R>, s: &Settings) {
    #[cfg(target_os = "macos")]
    {
        let (spelling, grammar) = wanted(s);
        use tauri::Manager;
        let handle = app.clone();
        let r = app.run_on_main_thread(move || {
            use crate::mac::spelling::{enabled, Check};
            for (check, want) in [(Check::Spelling, spelling), (Check::Grammar, grammar)] {
                if enabled(check) == want {
                    continue;
                }
                CHANGED_LIVE.store(true, Ordering::Relaxed);
                tracing::info!(?check, on = want, "spell checking changed");
                for webview in handle.webviews().into_values() {
                    push(&webview, check, want);
                }
            }
        });
        if let Err(e) = r {
            tracing::warn!(error = %e, "could not update spell checking");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, s);
}

/// A page finished loading (lib.rs `on_page_load`).
pub fn page_loaded<R: Runtime>(webview: &Webview<R>) {
    if !CHANGED_LIVE.load(Ordering::Relaxed) {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager;
        let Some(state) = webview.try_state::<std::sync::Arc<crate::state::AppState>>() else {
            return;
        };
        let (spelling, grammar) = wanted(&state.settings.get());
        use crate::mac::spelling::Check;
        push(webview, Check::Spelling, spelling);
        push(webview, Check::Grammar, grammar);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = webview;
}

#[cfg(target_os = "macos")]
fn push<R: Runtime>(webview: &Webview<R>, check: crate::mac::spelling::Check, want: bool) {
    let r = webview.with_webview(move |view| {
        // SAFETY: wry hands us its live WKWebView, on the main thread.
        unsafe { crate::mac::spelling::push(view.inner(), check, want) }
    });
    if let Err(e) = r {
        tracing::warn!(error = %e, "could not update a window's spell checking");
    }
}

/// Edit ▸ Spelling and Grammar ▸ Show Spelling and Grammar / Check Document
/// Now: sent to whatever has focus in the key window.
pub fn menu_action<R: Runtime>(app: &AppHandle<R>, id: &str) {
    #[cfg(target_os = "macos")]
    {
        use crate::mac::spelling::{send, Action};
        let action = match id {
            MENU_SHOW_PANEL => Action::ShowPanel,
            MENU_CHECK_NOW => Action::CheckNow,
            _ => return,
        };
        let r = app.run_on_main_thread(move || {
            if !send(action) {
                tracing::debug!(?action, "no text field took the spelling action");
            }
        });
        if let Err(e) = r {
            tracing::warn!(error = %e, "could not send the spelling action");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, id);
}

pub const MENU_SHOW_PANEL: &str = "edit.spelling.panel";
pub const MENU_CHECK_NOW: &str = "edit.spelling.checkNow";
pub const MENU_WHILE_TYPING: &str = "edit.spelling.whileTyping";
pub const MENU_GRAMMAR: &str = "edit.spelling.grammar";

/// The settings change a click on one of the check items asks for.
pub fn menu_patch(id: &str, s: &Settings) -> Option<crate::settings::SettingsPatch> {
    let mut patch = crate::settings::SettingsPatch::default();
    match id {
        MENU_WHILE_TYPING => patch.check_spelling = Some(!s.check_spelling),
        MENU_GRAMMAR => patch.check_grammar = Some(!s.check_grammar),
        _ => return None,
    }
    Some(patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_check_items_flip_their_setting() {
        let s = Settings::default();
        let p = menu_patch(MENU_WHILE_TYPING, &s).unwrap();
        assert_eq!(p.check_spelling, Some(false));
        assert_eq!(p.check_grammar, None);
        let p = menu_patch(MENU_GRAMMAR, &s).unwrap();
        assert_eq!(p.check_grammar, Some(true));
        assert_eq!(p.check_spelling, None);
        let off = Settings {
            check_spelling: false,
            ..Settings::default()
        };
        assert_eq!(
            menu_patch(MENU_WHILE_TYPING, &off).unwrap().check_spelling,
            Some(true)
        );
        assert!(menu_patch(MENU_SHOW_PANEL, &s).is_none());
        assert!(menu_patch("view.hints", &s).is_none());
    }

    #[test]
    fn spelling_on_and_grammar_off_by_default() {
        assert_eq!(wanted(&Settings::default()), (true, false));
    }
}
