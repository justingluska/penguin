//! The native menu bar (macOS): Penguin, File, Edit, View, Mailbox, Message,
//! Window, Help.
//!
//! Routing. Standard items (Edit's Undo…Select All, Hide, Quit, Minimize,
//! Full Screen, Services) are AppKit's own predefined items, so text fields
//! keep working. Everything Penguin-specific carries an id and is emitted to
//! the UI as `penguin://menu {id}`; `src/app/menu.ts` runs the matching entry
//! of the shortcut registry (same code, same `when()` gate as the key).
//! About, zoom, Check for Updates (src/updater.rs) and Help → GitHub are
//! handled here. Copy Debug Info goes to the UI, which gathers the text and
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
//! ⇧⌘N Check for New Mail, ⌘1–⌘8 mailboxes. Floe keeps its ⇧⌘F, so Forward
//! is ⌥⌘F.
//!
//! Enabled state comes from the UI (`set_menu_context`: is there a selected
//! thread, is a modal open…); check marks for theme, density, Floe and key
//! hints follow `Settings` on every settings-changed.

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
];

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
    pub sidebar_visible: bool,
    pub floe: bool,
    /// The list's Unread filter is on.
    pub unread_only: bool,
}

#[derive(Serialize, Clone)]
struct MenuPayload {
    id: String,
}

/// Handles to the items whose state changes, managed as Tauri state.
pub struct AppMenu<R: Runtime> {
    items: HashMap<&'static str, MenuItem<R>>,
    checks: HashMap<&'static str, CheckMenuItem<R>>,
    zoom: Mutex<f64>,
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
        .separator()
        .item(&item(
            app,
            &mut items,
            ("window.close", "Close", Some("CmdOrCtrl+W")),
        )?)
        .build()?;

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

    let mut message = SubmenuBuilder::new(app, "Message");
    for (i, spec) in MESSAGE_ITEMS.iter().enumerate() {
        // Reply · Reply All · Forward | Archive · Trash | Read · Star · Label
        if i == 3 || i == 5 {
            message = message.separator();
        }
        message = message.item(&item(app, &mut items, *spec)?);
    }
    let message = message.build()?;

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
        zoom: Mutex::new(1.0),
    });
    state.apply_context(&MenuContext::default());
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

    pub fn apply_context(&self, ctx: &MenuContext) {
        let mail = ctx.mail && !ctx.blocked;
        for id in MAIL_ITEMS {
            self.enable(id, mail);
        }
        for (id, ..) in GO_ITEMS {
            self.enable(id, mail);
        }
        let acts = mail && ctx.selection;
        for (id, ..) in MESSAGE_ITEMS {
            self.enable(id, acts);
        }
        self.enable("app.sidebar", mail && !ctx.floe);
        self.checked("list.unread", ctx.unread_only);
        self.text(
            "app.sidebar",
            if ctx.sidebar_visible {
                "Hide Sidebar"
            } else {
                "Show Sidebar"
            },
        );
        self.text(
            "triage.read",
            if ctx.selection_unread {
                "Mark as Read"
            } else {
                "Mark as Unread"
            },
        );
        self.text(
            "triage.star",
            if ctx.selection_starred {
                "Unstar"
            } else {
                "Star"
            },
        );
        self.enable("app.accounts", ctx.mail);
    }

    pub fn apply_settings(&self, s: &Settings) {
        self.checked("floe.toggle", s.floe_mode);
        self.checked("view.hints", s.show_shortcut_hints);
        self.checked("view.theme.system", s.theme == ThemeSetting::System);
        self.checked("view.theme.light", s.theme == ThemeSetting::Light);
        self.checked("view.theme.dark", s.theme == ThemeSetting::Dark);
        self.checked("view.density.compact", s.density == Density::Compact);
        self.checked(
            "view.density.comfortable",
            s.density == Density::Comfortable,
        );
    }

    fn zoom(&self, app: &AppHandle<R>, dir: i8) {
        let mut z = self.zoom.lock().unwrap_or_else(|p| p.into_inner());
        let i = ZOOM_STEPS
            .iter()
            .position(|s| (s - *z).abs() < 1e-6)
            .unwrap_or(5);
        let next = match dir {
            0 => 1.0,
            d if d > 0 => ZOOM_STEPS[(i + 1).min(ZOOM_STEPS.len() - 1)],
            _ => ZOOM_STEPS[i.saturating_sub(1)],
        };
        if let Some(w) = app.get_webview_window("main") {
            match w.set_zoom(next) {
                Ok(()) => *z = next,
                Err(e) => tracing::warn!(error = %e, "zoom failed"),
            }
        }
        self.enable("view.zoom.reset", (*z - 1.0).abs() > 1e-6);
        self.enable("view.zoom.in", *z < ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        self.enable("view.zoom.out", *z > ZOOM_STEPS[0]);
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

pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id().as_ref();
    let Some(menu) = app.try_state::<Arc<AppMenu<R>>>() else {
        return;
    };
    match id {
        "view.zoom.in" => menu.zoom(app, 1),
        "view.zoom.out" => menu.zoom(app, -1),
        "view.zoom.reset" => menu.zoom(app, 0),
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
        // AppKit toggles a check item's mark on click; settings decide it.
        _ => {
            if menu.checks.contains_key(id) {
                if let Some(state) = app.try_state::<Arc<crate::state::AppState>>() {
                    menu.apply_settings(&state.settings.get());
                }
            }
            // ⌘W on a hidden window has nothing to show; anything else
            // (⌘N, Settings…) is for a window the user can see.
            if id != "window.close" {
                show_main(app);
            }
            if let Err(e) = app.emit_to("main", EVENT_MENU, MenuPayload { id: id.to_string() }) {
                tracing::warn!(error = %e, "could not send a menu action to the UI");
            }
        }
    }
}

/// The UI's state for enabling items (see [`MenuContext`]).
#[tauri::command]
pub fn set_menu_context(app: AppHandle, context: MenuContext) -> CmdResult<()> {
    if let Some(m) = app.try_state::<Arc<AppMenu<tauri::Wry>>>() {
        m.apply_context(&context);
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
        ] {
            assert!(shortcuts.contains(&format!("id: \"{id}\"")), "{id}");
        }
    }

    #[test]
    fn menu_context_wire_format() {
        let ctx: MenuContext = serde_json::from_value(serde_json::json!({
            "mail": true, "selection": true, "selectionUnread": true, "sidebarVisible": true,
            "unreadOnly": true
        }))
        .unwrap();
        assert!(ctx.mail && ctx.selection && ctx.selection_unread && !ctx.blocked);
        assert!(ctx.unread_only && !ctx.floe);
    }
}
