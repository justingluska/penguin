//! WebKit's spelling and grammar checking in WKWebView (the crate root's
//! spelling.rs says why Penguin drives it).
//!
//! WebKit keeps one app-wide state in the UI process (`TextChecker`,
//! TextCheckerMac.mm), read once from the app's user defaults
//! `WebContinuousSpellCheckingEnabled` / `WebGrammarCheckingEnabled` and
//! written back to them on every change. WKWebView's public way to change it
//! is `toggleContinuousSpellChecking:` / `toggleGrammarChecking:`, and each
//! toggle sends the new state to *that* view's web process only
//! (WebViewImpl.mm: `legacyMainFrameProcess()->updateTextCheckerState()`;
//! WebKit PR 74998 would broadcast it to every process). Hence [`push`].

use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel, MainThreadMarker};
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSString, NSUserDefaults};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Check Spelling While Typing (continuous spell checking).
    Spelling,
    /// Check Grammar With Spelling.
    Grammar,
}

impl Check {
    pub fn default_key(self) -> &'static str {
        match self {
            Check::Spelling => "WebContinuousSpellCheckingEnabled",
            Check::Grammar => "WebGrammarCheckingEnabled",
        }
    }
}

fn key(c: Check) -> Retained<NSString> {
    NSString::from_str(c.default_key())
}

/// WebKit's app-wide state for `c`, as its user default mirrors it.
pub fn enabled(c: Check) -> bool {
    NSUserDefaults::standardUserDefaults().boolForKey(&key(c))
}

/// Whether the app's defaults hold a value for `c` at all.
pub fn has_default(c: Check) -> bool {
    NSUserDefaults::standardUserDefaults()
        .objectForKey(&key(c))
        .is_some()
}

/// Write the default WebKit reads its initial state from. It only decides
/// the state when written before WebKit's text checker first reads it
/// (before the first web view).
pub fn set_default(c: Check, on: bool) {
    NSUserDefaults::standardUserDefaults().setBool_forKey(on, &key(c));
}

/// Flip WebKit's app-wide state for `c` and send it to this view's web process.
///
/// # Safety
/// `webview` is a live WKWebView; call on the main thread.
unsafe fn toggle(webview: *mut c_void, c: Check) {
    // SAFETY: the caller's contract.
    let view = unsafe { &*(webview as *const AnyObject) };
    let sender: Option<&AnyObject> = None;
    // SAFETY: both are IBActions WKWebView implements on macOS
    // (WKWebViewMac.mm), taking a sender and returning void.
    unsafe {
        match c {
            Check::Spelling => {
                let _: () = msg_send![view, toggleContinuousSpellChecking: sender];
            }
            Check::Grammar => {
                let _: () = msg_send![view, toggleGrammarChecking: sender];
            }
        }
    }
}

/// Leave WebKit's app-wide state for `c` at `want` and make sure this view's
/// web process has it too: one toggle when the state differs, else two (a
/// process that missed an earlier change, made from another view, still has
/// the old state, and a toggle is the only way to send it the new one).
///
/// # Safety
/// `webview` is a live WKWebView; call on the main thread.
pub unsafe fn push(webview: *mut c_void, c: Check, want: bool) {
    // SAFETY: the caller's contract.
    unsafe {
        if enabled(c) != want {
            toggle(webview, c);
        } else {
            toggle(webview, c);
            toggle(webview, c);
        }
    }
}

/// Which responder action a Spelling and Grammar menu item sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Show Spelling and Grammar: the shared spelling panel.
    ShowPanel,
    /// Check Document Now: mark the focused field's misspellings once.
    CheckNow,
}

/// Send `action` down the key window's responder chain, where the focused
/// WKWebView takes it, as AppKit's own Edit menu items do. False when
/// nothing handled it (off the main thread, or nothing in the chain does).
pub fn send(action: Action) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let action = match action {
        Action::ShowPanel => sel!(showGuessPanel:),
        Action::CheckNow => sel!(checkSpelling:),
    };
    // SAFETY: standard responder actions; nil target and sender send them to
    // the key window's first responder.
    unsafe { NSApplication::sharedApplication(mtm).sendAction_to_from(action, None, None) }
}
