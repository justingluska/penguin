// Every app command must be listed here: tauri-build generates an
// `allow-<command>` permission for each, and capabilities/default.json grants
// exactly those. A command missing from either list is unreachable from JS.
const COMMANDS: &[&str] = &[
    // Self-update (src/updater.rs).
    "restart_to_update",
    "check_for_updates",
    // Settings → Developer → View log, and UI-side errors (src/applog.rs).
    "read_log",
    "log_client_event",
    "oauth_client_status",
    "set_oauth_client",
    "list_accounts",
    "add_account",
    "remove_account",
    "sync_status",
    "sync_now",
    "list_labels",
    "update_label",
    "delete_label",
    // Context-menu text services (src/text_services.rs).
    "native_paste",
    "copy_text",
    "spell_check",
    "learn_spelling",
    "look_up",
    "list_threads",
    "get_thread",
    "load_remote_images",
    "modify_threads",
    "send_message",
    "search",
    "open_external",
    "sanitize_compose_html",
    "save_attachment",
    "preview_attachment",
    "get_message_details",
    // Unsubscribe button (src/unsubscribe.rs).
    "unsubscribe_check",
    "unsubscribe",
    "get_message_source",
    "person_summary",
    "ask",
    "ask_understand",
    "ask_query",
    "open_path",
    // Image viewer Save / Copy (src/image_viewer.rs).
    "fetch_message_image",
    "save_message_image",
    "save_message_images",
    // Drag out, Save As, Show in Finder (src/file_export.rs).
    "prepare_image_drag",
    "prepare_attachment_drag",
    "start_file_drag",
    "save_image_as",
    "save_attachment_as",
    "reveal_saved_path",
    "save_draft",
    "quote_sources",
    "delete_draft",
    "get_draft",
    "get_settings",
    "update_settings",
    "diagnostics",
    "diagnostics_table_sizes",
    "reveal_path",
    "optimize_index",
    // Search by meaning: indexing progress (src/semantic/).
    "semantic_status",
    "start_model_download",
    "reconnect_account",
    "cancel_sign_in",
    // Browser sign-in fallback: "Open it again" / "Copy link" (src/sign_in.rs).
    "sign_in_link",
    "reopen_sign_in",
    "set_ios_oauth_client",
    "clear_ios_oauth_client",
    // Add account: provider detection, the Microsoft client and non-Google
    // sign-in (src/providers/).
    "detect_provider",
    "connect_account",
    "microsoft_client_status",
    "set_microsoft_client",
    "retry_account_sync",
    "update_account",
    "mcp_info",
    "cli_install_status",
    "install_cli",
    "sync_window_estimate",
    "sync_coverage",
    "free_up_space",
    "search_server",
    "schedule_send",
    "cancel_scheduled_send",
    "list_scheduled_sends",
    "set_reminder",
    "cancel_reminder",
    "list_reminders",
    "snooze_threads",
    "unsnooze_threads",
    "list_snoozes",
    "reply_later",
    "triage_counts",
    "dismiss_follow_ups",
    // Smart views: header figures and sidebar counts (src/smart_views.rs).
    "smart_view_info",
    "smart_counts",
    // Split Inbox tab counts (src/smart_views.rs).
    "split_counts",
    "avatar_lookup",
    "avatar_status",
    "connect_contact_photos",
    "clear_avatar_cache",
    "account_photo",
    "list_rules",
    "save_rule",
    "delete_rule",
    "set_rule_enabled",
    "reorder_rules",
    "set_allow_hooks",
    "preview_rule",
    "run_rule",
    "rule_history",
    "undo_rule_actions",
    "calendar_status",
    "connect_calendar",
    "set_calendar_selected",
    "list_events",
    "get_event",
    "event_invite",
    "person_meetings",
    "respond_to_event",
    "respond_to_invite",
    "calendar_sync_now",
    "set_menu_context",
    "set_me_photo",
    "set_me_photo_from_google",
    "clear_me_photo",
    "me_photo",
    // New-mail notifications (src/notify.rs).
    "set_notify_context",
    "notification_permission",
    "request_notification_permission",
    "test_notification",
    // Thread summaries with Apple's on-device model (src/summary/).
    "summary_availability",
    "cached_summary",
    "summarize_thread",
    "cancel_summary",
    "prewarm_summarizer",
    // Writing in the composer with the same model (src/writing/), and files
    // kept with snippets (src/snippet_files.rs).
    "write_with_ai",
    "cancel_write",
    "prewarm_writer",
    "suggest_replies",
    "save_snippet_file",
    "read_snippet_file",
    "read_clipboard",
    "spell_check",
    "learn_spelling",
    "look_up",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
    app_version();
    summaries_bridge();
}

/// The app's one version number, as `PENGUIN_APP_VERSION` (read through
/// `crate::VERSION`): `PENGUIN_VERSION` when the build sets it (CI:
/// `0.1.<run>`, the same value it passes as Tauri's `version`, so the bundle,
/// the updater and everything the app prints agree), else tauri.conf.json's.
/// Cargo.toml's package version is never shown anywhere.
fn app_version() {
    println!("cargo:rerun-if-env-changed=PENGUIN_VERSION");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    let version = match std::env::var("PENGUIN_VERSION") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => {
            let conf = std::fs::read_to_string("tauri.conf.json").expect("read tauri.conf.json");
            let json: serde_json::Value =
                serde_json::from_str(&conf).expect("parse tauri.conf.json");
            json["version"]
                .as_str()
                .expect("tauri.conf.json has a version")
                .to_string()
        }
    };
    let semver = version.split('.').count() == 3
        && version
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    assert!(semver, "app version {version:?} isn't MAJOR.MINOR.PATCH");
    println!("cargo:rustc-env=PENGUIN_APP_VERSION={version}");
}

/// Thread summaries (src/summary/apple.rs, docs/SUMMARIES.md): build the
/// Swift bridge to Apple's Foundation Models framework
/// (swift/PenguinAI) with swift-rs and link it. Mac only; other targets use
/// `summary::engine::Unavailable`.
///
/// The deployment target stays what the Rust build targets (rustc's own
/// default: 11.0 on Apple silicon, 10.15 as swift-rs's floor on Intel), so
/// the app keeps launching on macOS older than 26: the bridge checks
/// `#available(macOS 26.0, *)` before touching the framework, and the
/// framework is linked weak. An SDK without FoundationModels (Xcode before
/// 26) still builds; the bridge then reports "not built".
fn summaries_bridge() {
    println!("cargo:rerun-if-changed=swift/PenguinAI/Package.swift");
    println!("cargo:rerun-if-changed=swift/PenguinAI/Sources");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let min = std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| {
        if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
            "11.0".into()
        } else {
            "10.15".into()
        }
    });
    swift_rs::SwiftLinker::new(&min)
        .with_package("PenguinAI", "swift/PenguinAI")
        .link();
    if sdk_has_foundation_models() {
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,FoundationModels");
    }
    // Swift concurrency (the bridge's Task) lives in libswift_Concurrency,
    // which the compiler references as @rpath/libswift_Concurrency.dylib
    // when the deployment target is below macOS 12. Xcode apps get
    // /usr/lib/swift on their run path for that; a rustc link doesn't, so
    // add it (the OS copy, macOS 12+). Weak, as Xcode 26 makes it: macOS 11
    // has no copy, and the bridge only runs a Task on macOS 26.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-weak-lswift_Concurrency");
}

fn sdk_has_foundation_models() -> bool {
    let Ok(out) = std::process::Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
    else {
        return false;
    };
    let sdk = String::from_utf8_lossy(&out.stdout).trim().to_string();
    std::path::Path::new(&sdk)
        .join("System/Library/Frameworks/FoundationModels.framework")
        .exists()
}

