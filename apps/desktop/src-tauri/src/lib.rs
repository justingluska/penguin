//! Penguin desktop: Tauri glue between the React UI and the penguin-* crates.
// serde_json's `json!` expands recursively per key; the settings snapshot
// test (settings.rs) spells out every setting and passes the default 128.
#![recursion_limit = "256"]

/// Penguin's version, the one number shown everywhere: About (menu and
/// Settings), Developer, debug info, logs, the MCP server and user agents.
/// Set by build.rs from `PENGUIN_VERSION` (release builds) or tauri.conf.json;
/// never Cargo.toml's.
pub const VERSION: &str = env!("PENGUIN_APP_VERSION");

/// Archive, label, star, trash, snooze, Reply Later: the one optimistic
/// action path the UI, rules and agents share.
pub mod actions;
pub mod agent;
/// The app side of agent access: the agent socket, Settings → Developer → Agents.
pub mod agent_app;
/// The macOS menu bar.
pub mod app_menu;
pub mod applog;
pub mod ask;
pub mod attachments;
pub mod avatars;
pub mod calendar;
pub mod cli_install;
pub mod commands;
pub mod diagnostics;
pub mod error;
pub mod extract;
pub mod file_export;
pub mod image_viewer;
pub mod inline_images;
pub mod logging;
#[cfg(target_os = "macos")]
mod mac;
pub mod me;
pub mod memory;
pub mod message_details;
pub mod notify;
pub mod ops;
pub mod outbox;
pub mod outgoing;
pub mod providers;
pub mod qos;
pub mod receipts;
pub mod reply_later;
pub mod rules;
pub mod semantic;
pub mod settings;
/// Share links: upload to the user's own S3-compatible storage, copy a presigned link.
pub mod share;
pub mod sign_in;
pub mod smart_views;
pub mod snippet_files;
pub mod snooze;
pub mod spelling;
pub mod startup;
pub mod state;
pub mod summary;
pub mod sync_window;
pub mod text_services;
pub mod unsubscribe;
pub mod updater;
pub mod views;
/// Conversation and compose windows next to the main one.
pub mod windows;
pub mod writing;

use std::sync::Arc;

use tauri::{Manager, RunEvent, WindowEvent};

use crate::state::AppState;

/// Verification codes in mail stored before code detection existed: scan
/// the last 30 days once (later starts find nothing to do) and refresh the
/// rows that gained a code.
fn spawn_otp_backfill(state: Arc<AppState>) {
    const WINDOW_MS: i64 = 30 * 86_400_000;
    tauri::async_runtime::spawn_blocking(move || {
        let since = crate::ops::now_ms() - WINDOW_MS;
        // Nobody waits on it: utility QoS (qos.rs).
        match qos::utility(|| state.store.backfill_otp(since)) {
            Ok(changed) => {
                let mut by_account: std::collections::HashMap<String, Vec<String>> =
                    std::collections::HashMap::new();
                for (account, thread) in changed {
                    by_account.entry(account).or_default().push(thread);
                }
                for (account, threads) in by_account {
                    state.emit_mail_changed(&account, threads);
                }
            }
            Err(e) => tracing::warn!(error = %e, "verification-code backfill failed"),
        }
    });
}

/// Attachments older builds stored as inline and never showed (a PDF sent
/// `Content-Disposition: inline`): list them (penguin-core store_inline_repair.rs).
fn spawn_inline_repair(state: Arc<AppState>) {
    tauri::async_runtime::spawn_blocking(move || {
        // Nobody waits on it: utility QoS (qos.rs).
        match qos::utility(|| state.store.repair_inline_attachments()) {
            Ok(changed) => {
                if !changed.is_empty() {
                    tracing::info!(
                        threads = changed.len(),
                        "listed attachments stored as inline"
                    );
                }
                let mut by_account: std::collections::HashMap<String, Vec<String>> =
                    std::collections::HashMap::new();
                for (account, thread) in changed {
                    by_account.entry(account).or_default().push(thread);
                }
                for (account, threads) in by_account {
                    state.emit_mail_changed(&account, threads);
                }
            }
            Err(e) => tracing::warn!(error = %e, "inline attachment repair failed"),
        }
    });
}

/// The app's own page origin: tauri://localhost (macOS/Linux),
/// http(s)://tauri.localhost (Windows), and the Vite dev server (devUrl in
/// tauri.conf.json) in debug builds only.
fn is_app_origin(url: &tauri::Url) -> bool {
    let host = url.host_str();
    match url.scheme() {
        "tauri" => host == Some("localhost"),
        "http" | "https" if host == Some("tauri.localhost") => true,
        "http" => cfg!(debug_assertions) && host == Some("localhost") && url.port() == Some(1420),
        _ => false,
    }
}

/// Nothing may navigate the app webview or an email iframe away from the app
/// origin / about:srcdoc. Links clicked inside emails can't be caught in JS
/// (the sandboxed iframe runs no scripts and parent listeners don't see its
/// clicks), so on macOS they arrive here as navigations; web and mail links
/// go to the system browser and every navigation off-app is cancelled.
fn link_guard() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    use tauri_plugin_opener::OpenerExt;
    tauri::plugin::Builder::<tauri::Wry>::new("link-guard")
        .on_navigation(|webview, url| {
            match url.scheme() {
                // A click on a picture in a message (image_viewer.rs): hand a
                // well-formed request to the UI, never load anything.
                image_viewer::SCHEME => {
                    if let Some(req) = image_viewer::parse_open(url) {
                        use tauri::Emitter;
                        if let Err(e) =
                            webview.emit_to(webview.label(), image_viewer::EVENT_IMAGE_OPEN, req)
                        {
                            tracing::warn!(error = %e, "could not open the image viewer");
                        }
                    } else {
                        tracing::warn!("blocked a malformed image-viewer request");
                    }
                    false
                }
                "about" => matches!(url.as_str(), "about:blank" | "about:srcdoc"),
                // Object URLs our own scripts mint (e.g. PDF preview), only
                // when the embedded origin is the app's.
                "blob" => tauri::Url::parse(url.path())
                    .map(|inner| is_app_origin(&inner))
                    .unwrap_or(false),
                _ if is_app_origin(url) => true,
                _ => {
                    match commands::check_external_url(url.as_str()) {
                        Ok(u) => {
                            if let Err(e) = webview.opener().open_url(u.as_str(), None::<&str>) {
                                tracing::warn!(error = %e, "could not open link in the browser");
                            }
                        }
                        Err(_) => tracing::warn!(scheme = url.scheme(), "blocked navigation"),
                    }
                    false
                }
            }
        })
        .build()
}

pub fn run() {
    startup::begin();
    // Before the first web view: WebKit reads its spell-checking state once.
    spelling::prime();
    let context = tauri::generate_context!();
    // Only builds given an update channel (the release workflow) get the
    // updater plugin; see src/updater.rs.
    let updates = updater::configured(&context.config().plugins);
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init());
    if updates {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    let builder = builder
        .plugin(link_guard())
        .manage(sign_in::PendingSignIn::default())
        .manage(notify::Notifier::default())
        .manage(summary::Summarizer::system())
        .manage(writing::Writer::new())
        .manage(windows::Seeds::default())
        .setup(move |app| {
            logging::init_app(app.path().app_log_dir().ok().as_deref());
            tracing::info!(version = crate::VERSION, "penguin starting");
            penguin_gmail::set_app_version(crate::VERSION);
            // The bundle's version (Tauri config, what the updater compares)
            // must be this same number; build.rs derives both from one value.
            let bundled = app.package_info().version.to_string();
            if bundled != crate::VERSION {
                tracing::error!(bundled = %bundled, version = crate::VERSION, "app version mismatch: tauri config and PENGUIN_VERSION disagree");
            }
            startup::mark("setup started");
            let state = AppState::init(app.handle())?;
            startup::mark("database open");
            // Read receipts' own tables (src/receipts.rs), before any thread is read.
            if let Err(e) = state.store.migrate_receipts() {
                tracing::error!(error = %e, "could not set up the read receipts tables");
            }
            if let Err(e) = app_menu::install(app.handle(), &state.settings.get()) {
                tracing::error!(error = %e, "could not build the menu bar");
            }
            // Spell checking as the settings say (a no-op unless they
            // disagree with what WebKit stored; src/spelling.rs).
            spelling::apply(app.handle(), &state.settings.get());
            // Before any sync starts, so the first requests pace at the setting.
            penguin_gmail::api::set_units_per_min(state.settings.get().gmail_units_per_min);
            app.manage(state.clone());
            outbox::spawn(app.handle().clone(), state.clone());
            // Share links (src/share/): settings, uploads, expiry cleanup.
            let share = share::Share::init(&state);
            app.manage(share.clone());
            share::spawn_cleanup(share.clone());
            // The agent socket (penguin-cli draft/send/share-link, MCP write tools).
            agent_app::start(app.handle(), state.clone(), share);
            let rules = rules::Rules::init(app.handle(), &state);
            app.manage(rules.clone());
            rules::spawn(app.handle().clone(), state.clone(), rules);
            let avatars = avatars::commands::init(&state);
            app.manage(avatars.clone());
            avatars::commands::spawn(app.handle().clone(), state.clone(), avatars);
            let calendar = calendar::commands::init();
            app.manage(calendar.clone());
            calendar::commands::spawn(app.handle().clone(), state.clone(), calendar);
            calendar::invites::spawn_scanner(state.clone());
            receipts::spawn_scanner(state.clone());
            extract::spawn(state.clone());
            app.manage(providers::init());
            spawn_otp_backfill(state.clone());
            spawn_inline_repair(state.clone());
            snippet_files::spawn_prune(state.clone());
            // Drag-out files (src/file_export.rs); earlier sessions' are pruned.
            let exports = Arc::new(file_export::Exports::new(&state.paths.cache_dir));
            app.manage(exports.clone());
            file_export::spawn_prune(exports);
            // Search by meaning: model download/load and the background indexer.
            {
                // It fills the slot search and Ask read (AppState::semantic).
                let (a, b) = (Arc::downgrade(&state), Arc::downgrade(&state));
                state.semantic_indexer.spawn(
                    state.store.clone(),
                    semantic::Sink {
                        handles: Box::new(move |h| {
                            if let Some(s) = a.upgrade() {
                                s.set_semantic(h)
                            }
                        }),
                        progress: Box::new(move |p| {
                            if let Some(s) = b.upgrade() {
                                s.set_semantic_progress(p)
                            }
                        }),
                    },
                );
            }
            updater::init(app.handle(), updates);
            tauri::async_runtime::spawn(async move { state.start_all().await });
            startup::mark("setup done");
            Ok(())
        })
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                startup::mark("page loaded");
                spelling::page_loaded(webview);
            }
        })
        // Cached sender avatars (src/avatars/protocol.rs); the app CSP allows
        // this scheme in img-src and no remote image hosts.
        .register_asynchronous_uri_scheme_protocol(
            avatars::protocol::SCHEME,
            |ctx, request, responder| {
                let app = ctx.app_handle().clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let response = match app.try_state::<Arc<avatars::Avatars>>() {
                        Some(av) => avatars::protocol::respond(&av.resolver.cache, &request),
                        None => avatars::protocol::not_found(),
                    };
                    responder.respond(response);
                });
            },
        );
    let builder = builder.on_menu_event(app_menu::on_menu_event);
    builder
        .on_window_event(|window, event| {
            // Coming back to the app is the moment fresh mail matters most.
            if let WindowEvent::Focused(true) = event {
                if let Some(state) = window.try_state::<Arc<AppState>>() {
                    state.poke_all();
                }
                if let Some(cal) = window.try_state::<Arc<calendar::commands::Calendar>>() {
                    cal.on_focus();
                }
                // Menu items that act on a window go to this one now.
                app_menu::window_focused(window.app_handle(), window.label());
            }
            // A conversation or compose window closed for real.
            if let WindowEvent::Destroyed = event {
                app_menu::window_gone(window.app_handle(), window.label());
                if let Some(seeds) = window.try_state::<windows::Seeds>() {
                    seeds.forget(window.label());
                }
            }
            // macOS: closing the main window (red button, ⌘W) hides it like
            // Mail; sync, scheduled sends and reminders keep running. ⌘Q quits
            // and the Dock icon brings it back (RunEvent::Reopen). Conversation
            // and compose windows close for real.
            #[cfg(target_os = "macos")]
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == windows::MAIN {
                    api.prevent_close();
                    if let Err(e) = window.hide() {
                        tracing::warn!(error = %e, "could not hide the window");
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::oauth_client_status,
            commands::set_oauth_client,
            commands::list_accounts,
            commands::add_account,
            providers::connect::connect_account,
            commands::remove_account,
            commands::reconnect_account,
            commands::cancel_sign_in,
            sign_in::sign_in_link,
            sign_in::reopen_sign_in,
            commands::set_ios_oauth_client,
            commands::clear_ios_oauth_client,
            providers::detect_provider,
            providers::microsoft_client_status,
            providers::set_microsoft_client,
            commands::retry_account_sync,
            commands::update_account,
            commands::sync_status,
            commands::sync_now,
            commands::list_labels,
            commands::update_label,
            commands::delete_label,
            commands::list_threads,
            commands::get_thread,
            commands::load_remote_images,
            commands::modify_threads,
            commands::send_message,
            commands::search,
            commands::open_external,
            commands::sanitize_compose_html,
            commands::save_attachment,
            commands::preview_attachment,
            commands::preview_outgoing_file,
            commands::get_message_details,
            commands::get_message_source,
            commands::person_summary,
            commands::suggest_recipients,
            ask::ask,
            ask::ask_understand,
            ask::ask_query,
            commands::open_path,
            image_viewer::fetch_message_image,
            image_viewer::save_message_image,
            image_viewer::save_message_images,
            file_export::prepare_image_drag,
            file_export::prepare_attachment_drag,
            file_export::copy_attachment_file,
            file_export::start_file_drag,
            file_export::save_image_as,
            file_export::save_attachment_as,
            file_export::reveal_saved_path,
            share::share_link_config_get,
            share::share_link_config_set,
            share::share_link_config_clear,
            share::share_link_config_test,
            share::share_file,
            share::share_delete,
            commands::save_draft,
            commands::quote_sources,
            commands::delete_draft,
            commands::get_draft,
            commands::get_settings,
            commands::update_settings,
            commands::diagnostics,
            commands::diagnostics_table_sizes,
            commands::reveal_path,
            commands::optimize_index,
            semantic::semantic_status,
            semantic::start_model_download,
            commands::mcp_info,
            commands::cli_install_status,
            commands::install_cli,
            agent_app::enable_agent_send,
            agent_app::agent_activity,
            agent_app::agent_pending_sends,
            agent_app::agent_undo,
            sync_window::sync_window_estimate,
            sync_window::sync_coverage,
            sync_window::free_up_space,
            sync_window::search_server,
            outbox::schedule_send,
            outbox::cancel_scheduled_send,
            outbox::list_scheduled_sends,
            outbox::set_reminder,
            outbox::cancel_reminder,
            outbox::list_reminders,
            snooze::snooze_threads,
            snooze::unsnooze_threads,
            snooze::list_snoozes,
            reply_later::reply_later,
            reply_later::triage_counts,
            reply_later::dismiss_follow_ups,
            smart_views::smart_view_info,
            smart_views::smart_counts,
            smart_views::split_counts,
            avatars::commands::avatar_lookup,
            avatars::commands::avatar_status,
            avatars::commands::connect_contact_photos,
            avatars::commands::clear_avatar_cache,
            avatars::account::account_photo,
            rules::list_rules,
            rules::save_rule,
            rules::delete_rule,
            rules::set_rule_enabled,
            rules::reorder_rules,
            rules::set_allow_hooks,
            rules::preview_rule,
            rules::run_rule,
            rules::rule_history,
            rules::undo_rule_actions,
            calendar::commands::calendar_status,
            calendar::commands::connect_calendar,
            calendar::commands::set_calendar_selected,
            calendar::commands::list_events,
            calendar::commands::get_event,
            calendar::commands::event_invite,
            calendar::commands::person_meetings,
            calendar::commands::respond_to_event,
            calendar::invites::respond_to_invite,
            calendar::commands::calendar_sync_now,
            text_services::native_paste,
            text_services::copy_text,
            text_services::spell_check,
            text_services::learn_spelling,
            text_services::look_up,
            app_menu::set_menu_context,
            windows::open_window,
            windows::take_window_seed,
            updater::restart_to_update,
            updater::check_for_updates,
            applog::read_log,
            applog::log_client_event,
            me::set_me_photo,
            me::set_me_photo_from_google,
            me::clear_me_photo,
            me::me_photo,
            unsubscribe::unsubscribe_check,
            unsubscribe::unsubscribe,
            notify::set_notify_context,
            notify::notification_permission,
            notify::request_notification_permission,
            notify::test_notification,
            summary::summary_availability,
            summary::cached_summary,
            summary::summarize_thread,
            summary::cancel_summary,
            summary::prewarm_summarizer,
            writing::write_with_ai,
            writing::cancel_write,
            writing::prewarm_writer,
            writing::suggest_replies,
            snippet_files::save_snippet_file,
            snippet_files::read_snippet_file,
        ])
        .build(context)
        .expect("error while building Penguin")
        .run(|app, event| {
            // The Dock icon brings the main window back when it's hidden,
            // even while a conversation or compose window is open.
            #[cfg(target_os = "macos")]
            if let RunEvent::Reopen { .. } = event {
                let hidden = app
                    .get_webview_window(windows::MAIN)
                    .map(|w| !w.is_visible().unwrap_or(true) || w.is_minimized().unwrap_or(false))
                    .unwrap_or(false);
                if hidden {
                    app_menu::show_main(app);
                }
            }
            let _ = (app, event);
        });
}

#[cfg(test)]
mod tests {
    use super::is_app_origin;

    fn blob_allowed(s: &str) -> bool {
        let url = tauri::Url::parse(s).unwrap();
        tauri::Url::parse(url.path())
            .map(|inner| is_app_origin(&inner))
            .unwrap_or(false)
    }

    #[test]
    fn blob_urls_only_from_app_origin() {
        assert!(blob_allowed("blob:tauri://localhost/0b1c-uuid"));
        assert!(blob_allowed("blob:https://tauri.localhost/0b1c-uuid"));
        assert!(!blob_allowed("blob:https://evil.example/0b1c-uuid"));
        assert!(!blob_allowed("blob:null/0b1c-uuid"));
        assert!(!blob_allowed("blob:tauri://evil/0b1c-uuid"));
    }
}
