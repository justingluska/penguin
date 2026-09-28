//! macOS text services behind the app's own context menus
//! (apps/desktop/src/app/textMenu.ts), which replace WebKit's native menu:
//!
//! - `native_paste`: menu Paste. Page script can't read the clipboard in
//!   WKWebView without WebKit's "Paste" confirmation bubble, and handing the
//!   pasteboard to JS would make it a clipboard-reading API. Instead this
//!   sends AppKit's `paste:` action down the responder chain, exactly what
//!   Edit ▸ Paste / ⌘V do: WebKit pastes into the focused field with a
//!   trusted paste event, and the clipboard's contents never cross the IPC.
//! - `spell_check` / `learn_spelling`: NSSpellChecker, the same checker that
//!   draws WebKit's red underlines, for the suggestions WebKit's menu used to
//!   offer in the composer.
//! - `look_up`: Dictionary.app via a dict: URL.
//! - `copy_text`: write plain text to the general pasteboard. WebKit only
//!   lets the page write the clipboard during a click in the page, so a
//!   native menu item (Penguin ▸ Copy Debug Info) copies through here.
//!   Write-only: nothing is ever read back to JS.
//!
//! The sync commands run on the main thread (Tauri's default for non-async
//! commands), where AppKit expects to be called. Other platforms get empty
//! answers.

use serde::Serialize;
use tauri::AppHandle;

use crate::error::{CmdError, CmdResult};

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpellCheck {
    pub misspelled: bool,
    pub guesses: Vec<String>,
}

/// Longest word or phrase the spelling and Look Up commands accept.
const MAX_WORD: usize = 100;

fn check_word(word: &str) -> CmdResult<&str> {
    let w = word.trim();
    if w.is_empty() || w.chars().count() > MAX_WORD {
        return Err(CmdError::invalid("Not a word"));
    }
    Ok(w)
}

/// Edit ▸ Paste into whatever has keyboard focus in the window. Returns
/// false when nothing in the responder chain handled `paste:`.
#[tauri::command]
pub async fn native_paste(app: AppHandle) -> CmdResult<bool> {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            use objc2::{sel, MainThreadMarker};
            use objc2_app_kit::NSApplication;
            let handled = MainThreadMarker::new().is_some_and(|mtm| {
                // SAFETY: `paste:` is a standard responder action; nil target
                // and sender send it to the key window's first responder.
                unsafe {
                    NSApplication::sharedApplication(mtm).sendAction_to_from(
                        sel!(paste:),
                        None,
                        None,
                    )
                }
            });
            let _ = tx.send(handled);
        })
        .map_err(|e| CmdError::other(e.to_string()))?;
        rx.await.map_err(|e| CmdError::other(e.to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(false)
    }
}

/// Longest text `copy_text` accepts (debug info is a few KB).
const MAX_COPY: usize = 1 << 20;

/// Put plain text on the general pasteboard (macOS). Runs on the main
/// thread, where AppKit expects it.
#[tauri::command]
pub fn copy_text(text: String) -> CmdResult<()> {
    if text.len() > MAX_COPY {
        return Err(CmdError::invalid("Too much text to copy"));
    }
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
        use objc2_foundation::NSString;
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        // SAFETY: NSPasteboardTypeString is an AppKit-provided constant.
        let ok = pb.setString_forType(&NSString::from_str(&text), unsafe { NSPasteboardTypeString });
        if !ok {
            return Err(CmdError::other("The clipboard refused the text"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        Err(CmdError::other("Copying from the menu needs macOS"))
    }
}

#[tauri::command]
pub fn spell_check(word: String) -> CmdResult<SpellCheck> {
    let word = check_word(&word)?;
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSSpellChecker;
        use objc2_foundation::{NSNotFound, NSRange, NSString};
        let checker = NSSpellChecker::sharedSpellChecker();
        let s = NSString::from_str(word);
        let bad = checker.checkSpellingOfString_startingAt(&s, 0);
        if bad.location == NSNotFound as usize || bad.length == 0 {
            return Ok(SpellCheck::default());
        }
        let guesses = checker
            .guessesForWordRange_inString_language_inSpellDocumentWithTag(
                NSRange::new(0, s.length()),
                &s,
                None,
                0,
            )
            .map(|a| a.iter().map(|g| g.to_string()).collect())
            .unwrap_or_default();
        Ok(SpellCheck {
            misspelled: true,
            guesses,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = word;
        Ok(SpellCheck::default())
    }
}

#[tauri::command]
pub fn learn_spelling(word: String) -> CmdResult<()> {
    let word = check_word(&word)?;
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSSpellChecker;
        use objc2_foundation::NSString;
        NSSpellChecker::sharedSpellChecker().learnWord(&NSString::from_str(word));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = word;
    Ok(())
}

/// `dict://<percent-encoded text>`: every byte outside [A-Za-z0-9-_.~] is escaped.
fn dict_url(text: &str) -> String {
    let mut out = String::from("dict://");
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[tauri::command]
pub fn look_up(app: AppHandle, text: String) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let text = check_word(&text)?;
    app.opener()
        .open_url(dict_url(text), None::<&str>)
        .map_err(|e| CmdError::other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_url_escapes_everything_but_unreserved() {
        assert_eq!(dict_url("penguin"), "dict://penguin");
        assert_eq!(dict_url("ice floe"), "dict://ice%20floe");
        assert_eq!(dict_url("a/b?c#d"), "dict://a%2Fb%3Fc%23d");
        assert_eq!(dict_url("café"), "dict://caf%C3%A9");
    }

    #[test]
    fn words_are_bounded() {
        assert!(check_word("  ").is_err());
        assert!(check_word(&"x".repeat(MAX_WORD + 1)).is_err());
        assert_eq!(check_word(" floe ").unwrap(), "floe");
    }
}
