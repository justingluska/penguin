//! The native menu bar (macOS): Penguin, File, Edit, View, Mailbox, Message,
//! Window, Help.
//!
//! Routing. Standard items (Edit's Undo…Select All, Hide, Quit, Minimize,
//! Full Screen, Services) are AppKit's own predefined items, so text fields
//! keep working. Everything Penguin-specific carries an id and is emitted to
//! the UI as `penguin://menu {id}`; `src/app/menu.ts` runs the matching entry
//! of the shortcut registry (same code, same `when()` gate as the key).
//! About, zoom, Check for Updates (src/updater.rs), Help → GitHub and Edit ▸
//! Spelling and Grammar (src/spelling.rs: two responder actions and two
//! settings) are handled here. Copy Debug Info goes to the UI, which gathers the text and
//! copies it natively (text_services::copy_text).
//!
//! No double firing. WKWebView hands a ⌘-key to the page before the menu:
//! when the page's keyboard handler claims it (preventDefault), AppKit never
//! looks at the menu; when it doesn't (the shortcut's `when()` is false, or
//! focus is in a text field for a key that isn't `allowInInput`), the key
//! falls through to the menu once. So every accelerator here is either
//! menu-only or also registered in the page, and runs exactly once.
//! Single-key shortcuts (E, #, R…) stay page-only: a menu key equivalent
//! without ⌘/⌃ would swallow typing.
//!
//! Accelerators follow Apple Mail where Penguin had nothing: ⌃⌘A Archive,
//! ⌘⌫ Trash, ⇧⌘U Read/Unread, ⇧⌘L Star, ⌘R / ⇧⌘R Reply / Reply All,
//! ⇧⌘N Check for New Mail, ⌘1–⌘9 mailboxes. Floe keeps its ⇧⌘F, so Forward
//! is ⌥⌘F.
//!
//! Enabled state comes from the UI (`set_menu_context`: is there a selected
//! thread, is a modal open…); check marks for theme, density, Floe and key
//! hints follow `Settings` on every settings-changed.
//!
//! Several windows (src/windows.rs). Each window reports its own context,
//! and an item goes to the window it acts on: the Message items, Open in New
//! Window and ⌘W to the focused window (a conversation window replies to and
//! archives its own conversation; ⌘W closes the window in front), zoom to the
//! focused window's page, New Message in New Window straight to a new window,
//! and everything else (mailboxes, New Message, Search, Settings, Keyboard
//! Shortcuts…) to the main window, which is brought up for it. Enabled state
//! follows the same split: the focused window's context for its items, the
//! main window's for the rest.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::menu::{
    AboutMetadata, CheckMenuItem, CheckMenuItemBuilder, Menu, MenuEvent, MenuItem, MenuItemBuilder,
    PredefinedMenuItem, Submenu, SubmenuBuilder,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::error::CmdResult;
use crate::settings::{Density, Settings, ThemeSetting};
use crate::spelling;

/// Payload `{id}`: a menu item the UI runs.
pub const EVENT_MENU: &str = "penguin://menu";
pub const GITHUB_URL: &str = "https://github.com/justingluska/penguin";

const ZOOM_STEPS: &[f64] = &[0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// (id, title, accelerator); a literal `&` in a title is written `&&`
/// (muda reads a single one as a mnemonic marker). Ids that match a shortcut-registry id run that
/// shortcut in the UI.
type Spec = (&'static str, &'static str, Option<&'static str>);

const MESSAGE_ITEMS: &[Spec] = &[
    ("compose.reply", "Reply", Some("CmdOrCtrl+R")),
    ("compose.replyAll", "Reply All", Some("CmdOrCtrl+Shift+R")),
    ("compose.forward", "Forward", Some("CmdOrCtrl+Alt+F")),
    ("triage.done", "Archive", Some("Ctrl+CmdOrCtrl+A")),
    ("triage.trash", "Move to Trash", Some("CmdOrCtrl+Backspace")),
    // One of the two is live at a time (the UI's `when`: Not Spam for spam).
    ("triage.spam", "Report Spam", None),
    ("triage.notSpam", "Not Spam", None),
    ("triage.read", "Mark as Read", Some("CmdOrCtrl+Shift+U")),
    ("triage.star", "Star", Some("CmdOrCtrl+Shift+L")),
    ("triage.snooze", "Snooze…", None),
    ("triage.label", "Label…", None),
];

const GO_ITEMS: &[Spec] = &[
    ("go.inbox", "Inbox", Some("CmdOrCtrl+1")),
    ("go.starred", "Starred", Some("CmdOrCtrl+2")),
    ("go.sent", "Sent", Some("CmdOrCtrl+3")),
    ("go.drafts", "Drafts", Some("CmdOrCtrl+4")),
    ("go.done", "Done", Some("CmdOrCtrl+5")),
    ("go.all", "All Mail", Some("CmdOrCtrl+6")),
    ("go.trash", "Trash", Some("CmdOrCtrl+7")),
    ("go.snoozed", "Snoozed", Some("CmdOrCtrl+8")),
    ("go.spam", "Spam", Some("CmdOrCtrl+9")),
];

/// Edit ▸ Spelling and Grammar, in order (a separator after the first two).
/// Apple's ⌘: for the panel (here ⇧⌘;, the same keys on a US layout). Not
/// Apple's ⌘; for Check Document Now: in the composer that's Insert snippet.
/// Literal ids (the spelling.rs constants), so the shortcut sheet's
/// keyCatalog.ts test finds the keys.
const SPELLING_ITEMS: &[Spec] = &[
    (
        "edit.spelling.panel",
        "Show Spelling and Grammar",
        Some("CmdOrCtrl+Shift+;"),
    ),
    ("edit.spelling.checkNow", "Check Document Now", None),
    (
        "edit.spelling.whileTyping",
        "Check Spelling While Typing",
        None,
    ),
    ("edit.spelling.grammar", "Check Grammar With Spelling", None),
];

/// The Spelling and Grammar items that are settings (check marks).
const SPELLING_CHECKS: &[&str] = &[spelling::MENU_WHILE_TYPING, spelling::MENU_GRAMMAR];

/// Items that act on the focused window rather than the main one (besides
/// the Message items).
const LOCAL_ITEMS: &[&str] = &["window.close", "thread.openWindow"];

fn is_local(id: &str) -> bool {
    LOCAL_ITEMS.contains(&id) || MESSAGE_ITEMS.iter().any(|(m, ..)| *m == id)
}

/// Ids the UI handles only while the mail UI is up and no modal is open.
const MAIL_ITEMS: &[&str] = &[
    "compose.new",
    "search.open",
    "app.sync",
    "app.sidebar",
    "floe.toggle",
    "list.unread",
];

/// What the UI reports about itself (`set_menu_context`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct MenuContext {
    /// The mail UI is showing (not onboarding / loading).
    pub mail: bool,
    /// A modal is up (compose, search, Settings, a dialog): mail actions off.
    pub blocked: bool,
    /// A thread is selected (list or open).
    pub selection: bool,
    pub selection_unread: bool,
    pub selection_starred: bool,
    /// The selection is spam: Not Spam is live, Report Spam isn't.
    pub selection_spam: bool,
    pub sidebar_visible: bool,
    pub floe: bool,
    /// The list's Unread filter is on.
    pub unread_only: bool,
    /// A window that shows one conversation or one composer (src/windows.rs),
    /// not the mail shell.
    pub detached: bool,
}

#[derive(Serialize, Clone)]
struct MenuPayload {
    id: String,
}

/// Handles to the items whose state changes, managed as Tauri state.
pub struct AppMenu<R: Runtime> {
    items: HashMap<&'static str, MenuItem<R>>,
    checks: HashMap<&'static str, CheckMenuItem<R>>,
    windows: Mutex<Windows>,
}

/// What each window reported, which one is in front, and each page's zoom.
#[derive(Default)]
struct Windows {
    contexts: HashMap<String, MenuContext>,
    focused: Option<String>,
    zoom: HashMap<String, f64>,
}

impl Windows {
    /// The window local items go to: the focused one, else the main window.
    fn target(&self) -> String {
        self.focused
            .clone()
            .unwrap_or_else(|| crate::windows::MAIN.to_string())
    }
    fn context(&self, label: &str) -> MenuContext {
        self.contexts.get(label).cloned().unwrap_or_default()
    }
}

fn item<R: Runtime>(
    app: &AppHandle<R>,
    items: &mut HashMap<&'static str, MenuItem<R>>,
    (id, title, accel): Spec,
) -> tauri::Result<MenuItem<R>> {
    let mut b = MenuItemBuilder::with_id(id, title);
    if let Some(a) = accel {
        b = b.accelerator(a);
    }
    let it = b.build(app)?;
    items.insert(id, it.clone());
    Ok(it)
}

fn check<R: Runtime>(
    app: &AppHandle<R>,
    checks: &mut HashMap<&'static str, CheckMenuItem<R>>,
    (id, title, accel): Spec,
) -> tauri::Result<CheckMenuItem<R>> {
    let mut b = CheckMenuItemBuilder::with_id(id, title);
    if let Some(a) = accel {
        b = b.accelerator(a);
    }
    let it = b.build(app)?;
    checks.insert(id, it.clone());
    Ok(it)
}

/// Build the menu bar, install it app-wide and manage the handles.
pub fn install<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> tauri::Result<()> {
    let mut items = HashMap::new();
    let mut checks = HashMap::new();

    // The standard about panel; with no icon given it shows the app icon
    // from the bundle (icons/icon.icns, Tuxedo split A).
    let about = PredefinedMenuItem::about(
        app,
        Some("About Penguin"),
        Some(AboutMetadata {
            name: Some("Penguin".into()),
            version: Some(crate::VERSION.into()),
            short_version: Some(crate::VERSION.into()),
            copyright: Some("Fast, local-first email. No Penguin servers.".into()),
            ..Default::default()
        }),
    )?;
    let app_menu = SubmenuBuilder::new(app, "Penguin")
        .item(&about)
        .item(&item(
            app,
            &mut items,
            ("app.checkUpdates", "Check for Updates…", None),
        )?)
        .item(&item(
            app,
            &mut items,
            ("app.copyDebug", "Copy Debug Info", None),
        )?)
        .separator()
        .item(&item(
            app,
            &mut items,
            ("app.settings", "Settings…", Some("CmdOrCtrl+,")),
        )?)
        .item(&item(
            app,
            &mut items,
            ("app.accounts", "Accounts && Sync…", None),
        )?)
        .separator()
        .item(&PredefinedMenuItem::services(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::hide(app, Some("Hide Penguin"))?)
        .item(&PredefinedMenuItem::hide_others(app, None)?)
        .item(&PredefinedMenuItem::show_all(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::quit(app, Some("Quit Penguin"))?)
        .build()?;

    let file = SubmenuBuilder::new(app, "File")
        .item(&item(
            app,
            &mut items,
            ("compose.new", "New Message", Some("CmdOrCtrl+N")),
        )?)
        .item(&item(
            app,
            &mut items,
            (
                "compose.newWindow",
                "New Message in New Window",
                Some("CmdOrCtrl+Alt+N"),
            ),
        )?)
        .separator()
        .item(&item(
            app,
            &mut items,
            ("window.close", "Close", Some("CmdOrCtrl+W")),
        )?)
        .build()?;

    // Edit ▸ Spelling and Grammar, as in Mail and Safari. muda has no
    // predefined items for these: the two actions go down the responder
    // chain to the focused web view, and the two switches are settings
    // (src/spelling.rs).
    let mut spelling = SubmenuBuilder::new(app, "Spelling and Grammar");
    for (i, spec) in SPELLING_ITEMS.iter().enumerate() {
        if i == 2 {
            spelling = spelling.separator();
        }
        spelling = if SPELLING_CHECKS.contains(&spec.0) {
            spelling.item(&check(app, &mut checks, *spec)?)
        } else {
            spelling.item(&item(app, &mut items, *spec)?)
        };
    }
    let spelling = spelling.build()?;

    // Titled "Edit", so AppKit adds Emoji & Symbols and Dictation itself.
    let edit = SubmenuBuilder::new(app, "Edit")
        .item(&PredefinedMenuItem::undo(app, None)?)
        .item(&PredefinedMenuItem::redo(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .separator()
        .item(&spelling)
        .separator()
        .item(&item(
            app,
            &mut items,
            ("search.open", "Search Mail…", Some("CmdOrCtrl+F")),
        )?)
        .build()?;

    let theme = SubmenuBuilder::new(app, "Theme")
        .item(&check(
            app,
            &mut checks,
            ("view.theme.system", "Match System", None),
        )?)
        .item(&check(
            app,
            &mut checks,
            ("view.theme.light", "Light", None),
        )?)
        .item(&check(app, &mut checks, ("view.theme.dark", "Dark", None))?)
        .build()?;
    let density = SubmenuBuilder::new(app, "Density")
        .item(&check(
            app,
            &mut checks,
            ("view.density.compact", "Compact", None),
        )?)
        .item(&check(
            app,
            &mut checks,
            ("view.density.comfortable", "Comfortable", None),
        )?)
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item(
            app,
            &mut items,
            ("app.sidebar", "Hide Sidebar", Some("CmdOrCtrl+\\")),
        )?)
        .item(&check(
            app,
            &mut checks,
            ("floe.toggle", "Floe Mode", Some("CmdOrCtrl+Shift+F")),
        )?)
        .item(&check(
            app,
            &mut checks,
            ("list.unread", "Show Only Unread", None),
        )?)
        .separator()
        .item(&theme)
        .item(&density)
        .item(&check(
            app,
            &mut checks,
            ("view.hints", "Show Keyboard Hints", None),
        )?)
        .separator()
        .item(&item(
            app,
            &mut items,
            ("view.zoom.reset", "Actual Size", Some("CmdOrCtrl+0")),
        )?)
        .item(&item(
            app,
            &mut items,
            ("view.zoom.in", "Zoom In", Some("CmdOrCtrl+=")),
        )?)
        .item(&item(
            app,
            &mut items,
            ("view.zoom.out", "Zoom Out", Some("CmdOrCtrl+-")),
        )?)
        .separator()
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .build()?;

    let mut mailbox = SubmenuBuilder::new(app, "Mailbox")
        .item(&item(
            app,
            &mut items,
            ("app.sync", "Check for New Mail", Some("CmdOrCtrl+Shift+N")),
        )?)
        .separator();
    for spec in GO_ITEMS {
        mailbox = mailbox.item(&item(app, &mut items, *spec)?);
    }
    let mailbox = mailbox.build()?;

    let mut message = SubmenuBuilder::new(app, "Message")
        .item(&item(
            app,
            &mut items,
            ("thread.openWindow", "Open in New Window", None),
        )?)
        .separator();
    for (i, spec) in MESSAGE_ITEMS.iter().enumerate() {
        // Reply · Reply All · Forward | Archive · Trash | Report Spam · Not Spam | Read · Star · Label
        if i == 3 || i == 5 || i == 7 {
            message = message.separator();
        }
        message = message.item(&item(app, &mut items, *spec)?);
    }
    let message = message.build()?;

    // AppKit adds the tab items itself (Show Previous/Next Tab, Move Tab to
    // New Window, Merge All Windows) to this menu, and Show Tab Bar / Show
    // All Tabs to View (next to Enter Full Screen): every window shares one
    // tabbing identifier (src/windows.rs). Nothing to add here.
    let window = SubmenuBuilder::new(app, "Window")
        .item(&PredefinedMenuItem::minimize(app, None)?)
        .item(&PredefinedMenuItem::maximize(app, Some("Zoom"))?)
        .separator()
        .item(&PredefinedMenuItem::bring_all_to_front(app, None)?)
        .build()?;
    let help = SubmenuBuilder::new(app, "Help")
        .item(&item(
            app,
            &mut items,
            ("app.shortcuts", "Keyboard Shortcuts", None),
        )?)
        .separator()
        .item(&item(
            app,
            &mut items,
            ("help.github", "Penguin on GitHub", None),
        )?)
        .build()?;

    let menu = Menu::with_items(
        app,
        &[
            &app_menu, &file, &edit, &view, &mailbox, &message, &window, &help,
        ],
    )?;
    app.set_menu(menu)?;
    // macOS: the window list goes into Window; Help gets the search field.
    as_nsapp_menus(&window, &help);

    let state = Arc::new(AppMenu {
        items,
        checks,
        windows: Mutex::new(Windows::default()),
    });
    state.apply();
    state.apply_settings(settings);
    app.manage(state);
    Ok(())
}

#[cfg(target_os = "macos")]
fn as_nsapp_menus<R: Runtime>(window: &Submenu<R>, help: &Submenu<R>) {
    if let Err(e) = window.set_as_windows_menu_for_nsapp() {
        tracing::warn!(error = %e, "could not set the Window menu");
    }
    if let Err(e) = help.set_as_help_menu_for_nsapp() {
        tracing::warn!(error = %e, "could not set the Help menu");
    }
}

#[cfg(not(target_os = "macos"))]
fn as_nsapp_menus<R: Runtime>(_window: &Submenu<R>, _help: &Submenu<R>) {}

fn warn(r: tauri::Result<()>) {
    if let Err(e) = r {
        tracing::warn!(error = %e, "menu update failed");
    }
}

impl<R: Runtime> AppMenu<R> {
    fn enable(&self, id: &str, on: bool) {
        if let Some(i) = self.items.get(id) {
            warn(i.set_enabled(on));
        } else if let Some(c) = self.checks.get(id) {
            warn(c.set_enabled(on));
        }
    }

    fn text(&self, id: &str, text: &str) {
        if let Some(i) = self.items.get(id) {
            warn(i.set_text(text));
        }
    }

    fn checked(&self, id: &str, on: bool) {
        if let Some(c) = self.checks.get(id) {
            warn(c.set_checked(on));
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Windows> {
        self.windows.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Enable and name items from the main window's context and, for the
    /// items that act on it, the focused window's.
    fn apply(&self) {
        let (main, local) = {
            let w = self.lock();
            (w.context(crate::windows::MAIN), w.context(&w.target()))
        };
        self.apply_contexts(&main, &local);
    }

    fn apply_contexts(&self, main: &MenuContext, local: &MenuContext) {
        let mail = main.mail && !main.blocked;
        for id in MAIL_ITEMS {
            self.enable(id, mail);
        }
        for (id, ..) in GO_ITEMS {
            self.enable(id, mail);
        }
        self.enable("compose.newWindow", main.mail);
        let acts = local.mail && !local.blocked && local.selection;
        for (id, ..) in MESSAGE_ITEMS {
            self.enable(id, acts);
        }
        self.enable("triage.spam", acts && !local.selection_spam);
        self.enable("triage.notSpam", acts && local.selection_spam);
        self.enable("thread.openWindow", acts && !local.detached);
        self.enable("app.sidebar", mail && !main.floe);
        self.checked("list.unread", main.unread_only);
        self.text(
            "app.sidebar",
            if main.sidebar_visible {
                "Hide Sidebar"
            } else {
                "Show Sidebar"
            },
        );
        self.text(
            "triage.read",
            if local.selection_unread {
                "Mark as Read"
            } else {
                "Mark as Unread"
            },
        );
        self.text(
            "triage.star",
            if local.selection_starred {
                "Unstar"
            } else {
                "Star"
            },
        );
        self.enable("app.accounts", main.mail);
    }

    /// A window reported its context (`set_menu_context`).
    fn set_context(&self, label: &str, ctx: MenuContext) {
        self.lock().contexts.insert(label.to_string(), ctx);
        self.apply();
    }

    /// A window came to the front: its context drives the local items.
    fn focused(&self, label: &str) {
        let zoom = {
            let mut w = self.lock();
            w.focused = Some(label.to_string());
            w.zoom.get(label).copied().unwrap_or(1.0)
        };
        self.apply();
        self.zoom_items(zoom);
    }

    /// A window closed for good.
    fn gone(&self, label: &str) {
        {
            let mut w = self.lock();
            w.contexts.remove(label);
            w.zoom.remove(label);
            if w.focused.as_deref() == Some(label) {
                w.focused = None;
            }
        }
        self.apply();
    }

    pub fn apply_settings(&self, s: &Settings) {
        self.checked("floe.toggle", s.floe_mode);
        self.checked("view.hints", s.show_shortcut_hints);
        self.checked(spelling::MENU_WHILE_TYPING, s.check_spelling);
        self.checked(spelling::MENU_GRAMMAR, s.check_grammar);
        self.checked("view.theme.system", s.theme == ThemeSetting::System);
        self.checked("view.theme.light", s.theme == ThemeSetting::Light);
        self.checked("view.theme.dark", s.theme == ThemeSetting::Dark);
        self.checked("view.density.compact", s.density == Density::Compact);
        self.checked(
            "view.density.comfortable",
            s.density == Density::Comfortable,
        );
    }

    /// Zoom the focused window's page; each window keeps its own level.
    fn zoom(&self, app: &AppHandle<R>, dir: i8) {
        let (label, cur) = {
            let w = self.lock();
            let label = w.target();
            let cur = w.zoom.get(&label).copied().unwrap_or(1.0);
            (label, cur)
        };
        let i = ZOOM_STEPS
            .iter()
            .position(|s| (s - cur).abs() < 1e-6)
            .unwrap_or(5);
        let next = match dir {
            0 => 1.0,
            d if d > 0 => ZOOM_STEPS[(i + 1).min(ZOOM_STEPS.len() - 1)],
            _ => ZOOM_STEPS[i.saturating_sub(1)],
        };
        let mut now = cur;
        if let Some(w) = app.get_webview_window(&label) {
            match w.set_zoom(next) {
                Ok(()) => {
                    self.lock().zoom.insert(label, next);
                    now = next;
                }
                Err(e) => tracing::warn!(error = %e, "zoom failed"),
            }
        }
        self.zoom_items(now);
    }

    fn zoom_items(&self, z: f64) {
        self.enable("view.zoom.reset", (z - 1.0).abs() > 1e-6);
        self.enable("view.zoom.in", z < ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        self.enable("view.zoom.out", z > ZOOM_STEPS[0]);
    }
}

/// Keep the check marks in step with settings (called on every change).
pub fn sync_settings<R: Runtime>(app: &AppHandle<R>, settings: &Settings) {
    if let Some(m) = app.try_state::<Arc<AppMenu<R>>>() {
        m.apply_settings(settings);
    }
}

/// Bring the main window back (Dock click, a menu action while hidden).
pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        warn(w.show());
        warn(w.unminimize());
        warn(w.set_focus());
    }
}

/// Show the main window and make Penguin the active app, e.g. after a
/// browser sign-in finishes (the browser has focus then). Safe from any thread.
pub fn bring_to_front<R: Runtime>(app: &AppHandle<R>) {
    show_main(app);
    activate_app(app);
}

#[cfg(target_os = "macos")]
fn activate_app<R: Runtime>(app: &AppHandle<R>) {
    let r = app.run_on_main_thread(|| {
        use objc2::runtime::NSObjectProtocol;
        use objc2::{sel, MainThreadMarker};
        use objc2_app_kit::NSApplication;
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let ns = NSApplication::sharedApplication(mtm);
        // macOS 14 has `activate` (cooperative); older systems only the
        // deprecated `activateIgnoringOtherApps:`.
        if ns.respondsToSelector(sel!(activate)) {
            ns.activate();
        } else {
            #[allow(deprecated)]
            ns.activateIgnoringOtherApps(true);
        }
    });
    if let Err(e) = r {
        tracing::warn!(error = %e, "could not activate the app");
    }
}

#[cfg(not(target_os = "macos"))]
fn activate_app<R: Runtime>(_app: &AppHandle<R>) {}

/// The window a menu item goes to (see the module comment): the focused
/// window for local items, the main window for everything else.
fn route(focused: Option<&str>, id: &str) -> String {
    match focused {
        Some(label) if is_local(id) => label.to_string(),
        _ => crate::windows::MAIN.to_string(),
    }
}

/// A window came to the front (lib.rs, `WindowEvent::Focused`).
pub fn window_focused<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(m) = app.try_state::<Arc<AppMenu<R>>>() {
        m.focused(label);
    }
}

/// A window was closed for good (lib.rs, `WindowEvent::Destroyed`).
pub fn window_gone<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(m) = app.try_state::<Arc<AppMenu<R>>>() {
        m.gone(label);
    }
}

pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id().as_ref();
    let Some(menu) = app.try_state::<Arc<AppMenu<R>>>() else {
        return;
    };
    match id {
        "view.zoom.in" => menu.zoom(app, 1),
        "view.zoom.out" => menu.zoom(app, -1),
        "view.zoom.reset" => menu.zoom(app, 0),
        // For whatever text field has focus, in whichever window.
        spelling::MENU_SHOW_PANEL | spelling::MENU_CHECK_NOW => spelling::menu_action(app, id),
        // Settings (Settings → Compose shows the same switch): saving one
        // updates the check mark, every window's checking and every UI.
        spelling::MENU_WHILE_TYPING | spelling::MENU_GRAMMAR => {
            let Some(state) = app.try_state::<Arc<crate::state::AppState>>() else {
                return;
            };
            let state = state.inner().clone();
            let current = state.settings.get();
            // AppKit flipped the mark on click; the saved setting decides it.
            menu.apply_settings(&current);
            let Some(patch) = spelling::menu_patch(id, &current) else {
                return;
            };
            tauri::async_runtime::spawn(async move {
                if let Err(e) = crate::commands::save_settings(&state, patch).await {
                    tracing::warn!(error = %e.message, "could not save the spelling setting");
                }
            });
        }
        // Answered in the UI (toasts), so bring the window up.
        "app.checkUpdates" => {
            show_main(app);
            let app = app.clone();
            tauri::async_runtime::spawn(async move { crate::updater::check(&app, true).await });
        }
        "help.github" => {
            use tauri_plugin_opener::OpenerExt;
            if let Err(e) = app.opener().open_url(GITHUB_URL, None::<&str>) {
                tracing::warn!(error = %e, "could not open GitHub");
            }
        }
        // A composer of its own: nothing to ask the main window (the new
        // window picks its From account the way the main composer does).
        "compose.newWindow" => {
            let req = crate::windows::WindowRequest::Compose {
                account_id: None,
                draft_id: None,
                seed: None,
                title: None,
            };
            if let Err(e) = crate::windows::open(app, req) {
                tracing::warn!(error = %e.message, "could not open a compose window");
            }
        }
        // AppKit toggles a check item's mark on click; settings decide it.
        _ => {
            if menu.checks.contains_key(id) {
                if let Some(state) = app.try_state::<Arc<crate::state::AppState>>() {
                    menu.apply_settings(&state.settings.get());
                }
            }
            // The focused window may have closed since it last reported.
            let focused = menu
                .lock()
                .focused
                .clone()
                .filter(|l| app.get_webview_window(l).is_some());
            let target = route(focused.as_deref(), id);
            // ⌘W on a hidden window has nothing to show; anything else
            // (⌘N, Settings…) is for a window the user can see.
            if target == crate::windows::MAIN && id != "window.close" {
                show_main(app);
            }
            if let Err(e) = app.emit_to(
                target.as_str(),
                EVENT_MENU,
                MenuPayload { id: id.to_string() },
            ) {
                tracing::warn!(error = %e, "could not send a menu action to the UI");
            }
        }
    }
}

/// The calling window's state for enabling items (see [`MenuContext`]).
#[tauri::command]
pub fn set_menu_context(
    app: AppHandle,
    window: tauri::WebviewWindow,
    context: MenuContext,
) -> CmdResult<()> {
    if let Some(m) = app.try_state::<Arc<AppMenu<tauri::Wry>>>() {
        m.set_context(window.label(), context);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_and_go_ids_match_the_shortcut_registry() {
        // The UI runs these by shortcut-registry id (src/app/shortcuts.ts).
        let shortcuts = include_str!("../../src/app/shortcuts.ts");
        for (id, ..) in MESSAGE_ITEMS.iter().chain(GO_ITEMS) {
            assert!(shortcuts.contains(&format!("id: \"{id}\"")), "{id}");
        }
        for id in [
            "compose.new",
            "search.open",
            "app.sync",
            "app.sidebar",
            "app.shortcuts",
            "list.unread",
            "compose.newWindow",
            "thread.openWindow",
        ] {
            assert!(shortcuts.contains(&format!("id: \"{id}\"")), "{id}");
        }
    }

    #[test]
    fn spelling_and_grammar_is_mails_submenu() {
        let ids: Vec<&str> = SPELLING_ITEMS.iter().map(|(id, ..)| *id).collect();
        assert_eq!(
            ids,
            [
                spelling::MENU_SHOW_PANEL,
                spelling::MENU_CHECK_NOW,
                spelling::MENU_WHILE_TYPING,
                spelling::MENU_GRAMMAR,
            ]
        );
        let titles: Vec<&str> = SPELLING_ITEMS.iter().map(|(_, t, _)| *t).collect();
        assert_eq!(
            titles,
            [
                "Show Spelling and Grammar",
                "Check Document Now",
                "Check Spelling While Typing",
                "Check Grammar With Spelling",
            ]
        );
        // The two switches are settings; the two actions aren't.
        let s = Settings::default();
        for (id, ..) in SPELLING_ITEMS {
            assert_eq!(
                SPELLING_CHECKS.contains(id),
                spelling::menu_patch(id, &s).is_some(),
                "{id}"
            );
        }
        // Menu-only keys: no page shortcut claims them first (⌘; is the
        // composer's Insert snippet, so Check Document Now has no key).
        for page in [
            include_str!("../../src/app/shortcuts.ts"),
            include_str!("../../src/features/compose/commands.ts"),
        ] {
            assert!(!page.contains("\"mod+shift+;\""));
        }
        assert_eq!(SPELLING_ITEMS[1].2, None);
        // Handled here, not sent to a window.
        for (id, ..) in SPELLING_ITEMS {
            assert!(!is_local(id), "{id}");
        }
    }

    #[test]
    fn menu_context_wire_format() {
        let ctx: MenuContext = serde_json::from_value(serde_json::json!({
            "mail": true, "selection": true, "selectionUnread": true, "sidebarVisible": true,
            "unreadOnly": true, "detached": true
        }))
        .unwrap();
        assert!(ctx.mail && ctx.selection && ctx.selection_unread && !ctx.blocked);
        assert!(ctx.unread_only && !ctx.floe && ctx.detached);
    }

    #[test]
    fn items_go_to_the_window_they_act_on() {
        // The Message items, Open in New Window and ⌘W: the focused window.
        assert_eq!(route(Some("thread-01"), "triage.done"), "thread-01");
        assert_eq!(route(Some("thread-01"), "compose.reply"), "thread-01");
        assert_eq!(route(Some("compose-2"), "window.close"), "compose-2");
        assert_eq!(route(Some("thread-01"), "thread.openWindow"), "thread-01");
        // Mailboxes, New Message, Settings: always the main window.
        assert_eq!(route(Some("thread-01"), "go.inbox"), "main");
        assert_eq!(route(Some("compose-2"), "compose.new"), "main");
        assert_eq!(route(Some("thread-01"), "app.settings"), "main");
        // Nothing focused (yet): the main window.
        assert_eq!(route(None, "triage.done"), "main");
    }
}
