//! Organizing through the agent socket's service, against the fake
//! provider: every tool changes Penguin's copy and the provider's through
//! the app's action path, the counts follow, the answer's `undo` puts it
//! back exactly, the level gates it, the trash and spam caps hold, a
//! provider refusal is reported and reverted, and the audit keeps counts
//! only.

use std::sync::atomic::Ordering;

use penguin_core::{Address, Label, Message};
use serde_json::{json, Value};

use super::*;
use crate::agent::ipc::Handler;
use crate::agent::writes::tests::{TestHost, ADA, WORK};
use crate::agent::writes::AgentService;
use crate::error::ErrorCode;
use crate::settings::AgentAccess;

const SUBJECT: &str = "Quarterly walrus census";
const SENDER: &str = "dana@walrus.example";
const DAY_MS: i64 = 86_400_000;

fn message(account: &str, thread: &str, labels: &[&str]) -> Message {
    Message {
        account_id: account.into(),
        id: format!("{thread}-m"),
        thread_id: thread.into(),
        date: 1_767_261_600_000,
        from: Address {
            name: Some("Dana Reyes".into()),
            email: SENDER.into(),
        },
        to: vec![Address {
            name: None,
            email: account.into(),
        }],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: SUBJECT.into(),
        snippet: "The census".into(),
        body_text: "The census is in.".into(),
        body_html: None,
        label_ids: labels.iter().map(|s| s.to_string()).collect(),
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    }
}

/// A conversation with `labels`, in Penguin and on the provider.
fn seed(host: &TestHost, account: &str, thread: &str, labels: &[&str]) {
    let m = message(account, thread, labels);
    host.store
        .upsert_messages(std::slice::from_ref(&m))
        .unwrap();
    host.fakes[account].seed(m);
}

fn label(account: &str, id: &str, name: &str, kind: &str) -> Label {
    Label {
        account_id: account.into(),
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
        color: None,
        unread_count: None,
        hidden: false,
    }
}

/// The system mailboxes and one label of the user's, "Walrus Project".
fn seed_labels(host: &TestHost, account: &str) {
    let mut labels: Vec<Label> = ["INBOX", "TRASH", "SPAM", "STARRED", "UNREAD"]
        .iter()
        .map(|id| label(account, id, id, "system"))
        .collect();
    labels.push(label(account, "Label_7", "Walrus Project", "user"));
    host.store.replace_labels(account, &labels).unwrap();
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn local(host: &TestHost, account: &str, thread: &str) -> Vec<String> {
    sorted(
        host.store
            .get_thread(account, thread)
            .unwrap()
            .map(|t| t.label_ids)
            .unwrap_or_default(),
    )
}

fn remote(host: &TestHost, account: &str, thread: &str) -> Vec<String> {
    let server = host.fakes[account].server.lock().unwrap();
    let m = server
        .messages
        .values()
        .find(|m| m.thread_id == thread)
        .unwrap();
    sorted(m.label_ids.clone())
}

fn unread_in(host: &TestHost, account: &str, label_id: &str) -> u32 {
    host.store
        .list_labels(Some(account))
        .unwrap()
        .into_iter()
        .find(|l| l.id == label_id)
        .and_then(|l| l.unread_count)
        .unwrap_or(0)
}

fn strs(v: &[&str]) -> Vec<String> {
    sorted(v.iter().map(|s| s.to_string()).collect())
}

fn target(account: &str, thread: &str) -> Value {
    json!({"accountId": account, "threadId": thread})
}

async fn call(svc: &AgentService<TestHost>, tool: &str, args: Value) -> CmdResult<Value> {
    svc.handle("mcp", tool, args).await
}

async fn organized(svc: &AgentService<TestHost>, tool: &str, args: Value) -> OrganizedOut {
    let v = call(svc, tool, args)
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    assert_eq!(v["kind"], "organized", "{v}");
    serde_json::from_value(v["data"].clone()).unwrap()
}

/// Run an answer's undo calls as an agent would (each is a tool call).
async fn undo(svc: &AgentService<TestHost>, out: &OrganizedOut) {
    for step in &out.undo {
        let args = serde_json::to_value(&step.arguments).unwrap();
        organized(svc, &step.tool, args).await;
    }
}

/// Every tool on a conversation in a given state: Penguin's copy and the
/// provider's change the same way, the answer lists it as changed with its
/// previous labels, and running the answer's undo calls puts it back
/// exactly (Penguin and provider), however many steps that takes.
#[tokio::test]
async fn every_tool_changes_penguin_and_the_provider_and_its_undo_restores_it() {
    let host = TestHost::new("org-all", AgentAccess::Draft);
    seed_labels(&host, ADA);
    let svc = AgentService::new(host.clone());
    let rl = penguin_provider::ids::label_id_for_name("f:", penguin_core::REPLY_LATER_LABEL);
    let rl = rl.as_str();
    let later = host.now + DAY_MS;
    // (tool, extra args, labels before, labels after)
    let cases: Vec<(&str, Value, Vec<&str>, Vec<&str>)> = vec![
        (
            "archive",
            json!({}),
            vec!["INBOX", "UNREAD"],
            vec!["UNREAD"],
        ),
        (
            "unarchive",
            json!({}),
            vec!["UNREAD"],
            vec!["INBOX", "UNREAD"],
        ),
        (
            "mark_read",
            json!({}),
            vec!["INBOX", "UNREAD"],
            vec!["INBOX"],
        ),
        (
            "mark_unread",
            json!({}),
            vec!["INBOX"],
            vec!["INBOX", "UNREAD"],
        ),
        ("star", json!({}), vec!["INBOX"], vec!["INBOX", "STARRED"]),
        ("unstar", json!({}), vec!["INBOX", "STARRED"], vec!["INBOX"]),
        (
            "add_label",
            json!({"label": "walrus project"}),
            vec!["INBOX"],
            vec!["INBOX", "Label_7"],
        ),
        (
            "remove_label",
            json!({"label": "Label_7"}),
            vec!["Label_7", "UNREAD"],
            vec!["UNREAD"],
        ),
        // Archived before it was trashed: the undo untrashes and archives.
        ("trash", json!({}), vec!["UNREAD"], vec!["TRASH", "UNREAD"]),
        ("untrash", json!({}), vec!["TRASH"], vec!["INBOX"]),
        (
            "report_spam",
            json!({}),
            vec!["INBOX", "UNREAD"],
            vec!["SPAM", "UNREAD"],
        ),
        ("not_spam", json!({}), vec!["SPAM"], vec!["INBOX"]),
        ("reply_later", json!({}), vec!["INBOX", "UNREAD"], vec![rl]),
        ("clear_reply_later", json!({}), vec![rl], vec![]),
        ("snooze", json!({"until": later}), vec!["INBOX"], vec![]),
    ];
    for (i, (tool, extra, before, after)) in cases.into_iter().enumerate() {
        let thread = format!("t-{tool}");
        seed(&host, ADA, &thread, &before);
        if tool == "clear_reply_later" {
            // An account that has used Reply Later knows its label.
            host.store
                .upsert_label(&label(ADA, rl, "Reply Later", "user"))
                .unwrap();
        }
        let mut args = extra.clone();
        args["targets"] = json!([target(ADA, &thread)]);
        let events = host.organized.lock().unwrap().len();
        let out = organized(&svc, tool, args).await;
        let case = format!("#{i} {tool}");
        assert_eq!(local(&host, ADA, &thread), strs(&after), "{case} local");
        assert_eq!(remote(&host, ADA, &thread), strs(&after), "{case} provider");
        assert_eq!(out.changed.len(), 1, "{case}: {out:?}");
        assert!(out.unchanged.is_empty() && out.not_found.is_empty() && out.failed.is_empty());
        assert_eq!(
            sorted(out.previous[0].label_ids.clone()),
            strs(&before),
            "{case}"
        );
        assert!(!out.undo.is_empty(), "{case}");
        // The user is told, with the same undo (the toast's Undo).
        {
            let ev = host.organized.lock().unwrap();
            assert_eq!(ev.len(), events + 1, "{case}");
            assert_eq!(ev.last().unwrap().undo, out.undo, "{case}");
            assert_eq!(ev.last().unwrap().count, 1);
        }
        if tool == "snooze" {
            let s = host.store.get_snooze(ADA, &thread).unwrap().unwrap();
            assert_eq!(s.wake_at, later);
        }

        undo(&svc, &out).await;
        assert_eq!(
            local(&host, ADA, &thread),
            strs(&before),
            "{case} undone locally: {:?}",
            out.undo
        );
        assert_eq!(
            remote(&host, ADA, &thread),
            strs(&before),
            "{case} undone on the provider"
        );
        if tool == "snooze" {
            assert!(host.store.get_snooze(ADA, &thread).unwrap().is_none());
        }
    }
    // Trash's undo is untrash, then archive (it wasn't in the inbox).
    assert!(host.failures.lock().unwrap().is_empty());
}

/// unsnooze ends a snooze and brings the conversation back; its undo
/// snoozes it again until the same time. One that isn't snoozed is left
/// alone.
#[tokio::test]
async fn unsnooze_brings_back_only_snoozed_conversations_and_undoes_to_the_same_time() {
    let host = TestHost::new("org-unsnooze", AgentAccess::Draft);
    let svc = AgentService::new(host.clone());
    let wake = host.now + 3 * DAY_MS;
    seed(&host, ADA, "t-snoozed", &[]);
    seed(&host, ADA, "t-archived", &[]);
    host.store
        .snooze_threads(&[(ADA.into(), "t-snoozed".into())], wake, host.now)
        .unwrap();
    let out = organized(
        &svc,
        "unsnooze",
        json!({"targets": [target(ADA, "t-snoozed"), target(ADA, "t-archived")]}),
    )
    .await;
    assert_eq!(out.changed.len(), 1);
    assert_eq!(out.changed[0].thread_id, "t-snoozed");
    assert_eq!(
        out.unchanged,
        vec![ThreadTarget {
            account_id: ADA.into(),
            thread_id: "t-archived".into()
        }]
    );
    assert_eq!(out.previous[0].snoozed_until, Some(wake));
    assert_eq!(local(&host, ADA, "t-snoozed"), strs(&["INBOX"]));
    assert_eq!(
        local(&host, ADA, "t-archived"),
        Vec::<String>::new(),
        "left alone"
    );
    assert!(host.store.get_snooze(ADA, "t-snoozed").unwrap().is_none());
    assert_eq!(out.undo[0].tool, "snooze");
    assert_eq!(out.undo[0].arguments.until, Some(wake));
    undo(&svc, &out).await;
    assert_eq!(
        host.store
            .get_snooze(ADA, "t-snoozed")
            .unwrap()
            .unwrap()
            .wake_at,
        wake
    );
    assert_eq!(local(&host, ADA, "t-snoozed"), Vec::<String>::new());
}

/// The sidebar's counts follow at once: archiving unread inbox mail takes
/// it off the inbox's unread count, trash moves it, the undo puts it back,
/// and the UI hears mail-changed for exactly those conversations.
#[tokio::test]
async fn counts_follow_the_change_and_the_ui_is_told() {
    let host = TestHost::new("org-counts", AgentAccess::Draft);
    seed_labels(&host, ADA);
    let svc = AgentService::new(host.clone());
    for t in ["c1", "c2", "c3"] {
        seed(&host, ADA, t, &["INBOX", "UNREAD"]);
    }
    assert_eq!(unread_in(&host, ADA, "INBOX"), 3);
    host.changed.lock().unwrap().clear();
    let out = organized(
        &svc,
        "archive",
        json!({"targets": [target(ADA, "c1"), target(ADA, "c2")]}),
    )
    .await;
    assert_eq!(unread_in(&host, ADA, "INBOX"), 1);
    let changed = host.changed.lock().unwrap().clone();
    assert_eq!(
        changed,
        vec![(ADA.to_string(), vec!["c1".to_string(), "c2".to_string()])]
    );
    undo(&svc, &out).await;
    assert_eq!(unread_in(&host, ADA, "INBOX"), 3);
    organized(&svc, "trash", json!({"targets": [target(ADA, "c3")]})).await;
    assert_eq!(unread_in(&host, ADA, "INBOX"), 2);
    assert_eq!(unread_in(&host, ADA, "TRASH"), 1);
}

/// Each target is answered for: changed, already so, or not found (an
/// unknown thread or account is never touched); duplicates count once;
/// more than 100, none, or a system label are refused before anything
/// changes; a label missing in one account changes nothing anywhere.
#[tokio::test]
async fn every_target_is_accounted_for_and_bad_requests_change_nothing() {
    let host = TestHost::new("org-targets", AgentAccess::Draft);
    seed_labels(&host, ADA);
    let svc = AgentService::new(host.clone());
    seed(&host, ADA, "a1", &["INBOX"]);
    seed(&host, ADA, "a2", &[]);
    seed(&host, WORK, "w1", &["INBOX"]);
    let out = organized(
        &svc,
        "archive",
        json!({"targets": [
            target(ADA, "a1"), target(ADA, "a1"), target(ADA, "a2"), target(ADA, "nope"),
            target("stranger@elsewhere.example", "a1"), target(WORK, "w1"),
        ]}),
    )
    .await;
    let ids = |v: &[ThreadTarget]| v.iter().map(|t| t.thread_id.clone()).collect::<Vec<_>>();
    assert_eq!(
        out.changed
            .iter()
            .map(|c| c.thread_id.as_str())
            .collect::<Vec<_>>(),
        ["a1", "w1"]
    );
    assert_eq!(out.changed[0].removed, vec!["INBOX".to_string()]);
    assert_eq!(ids(&out.unchanged), ["a2"]);
    assert_eq!(ids(&out.not_found), ["nope", "a1"]);
    assert_eq!(out.not_found[1].account_id, "stranger@elsewhere.example");
    assert_eq!(out.previous.len(), 3);
    // The undo names only what changed, across both accounts.
    assert_eq!(out.undo.len(), 1);
    assert_eq!(out.undo[0].tool, "unarchive");
    assert_eq!(out.undo[0].arguments.targets.len(), 2);

    let err = |r: CmdResult<Value>| r.unwrap_err();
    let many: Vec<Value> = (0..101).map(|i| target(ADA, &format!("x{i}"))).collect();
    let e = err(call(&svc, "star", json!({"targets": many})).await);
    assert_eq!(e.code, ErrorCode::InvalidInput);
    assert!(e.message.contains("at most 100"), "{}", e.message);
    let e = err(call(&svc, "star", json!({"targets": []})).await);
    assert_eq!(e.code, ErrorCode::InvalidInput);
    let e = err(call(&svc, "star", json!({"targets": [{"accountId": ADA}]})).await);
    assert_eq!(e.code, ErrorCode::InvalidInput);
    // Inbox, Trash, Spam… only through their own tools (and their caps).
    for system in ["TRASH", "spam", "INBOX", "CATEGORY_PROMOTIONS"] {
        let e = err(call(
            &svc,
            "add_label",
            json!({"targets": [target(ADA, "a1")], "label": system}),
        )
        .await);
        assert_eq!(e.code, ErrorCode::InvalidInput, "{system}: {}", e.message);
    }
    // "Walrus Project" exists in Ada's account only: nothing changes.
    let e = err(call(
        &svc,
        "add_label",
        json!({"targets": [target(ADA, "a1"), target(WORK, "w1")], "label": "Walrus Project"}),
    )
    .await);
    assert_eq!(e.code, ErrorCode::NotFound);
    assert!(e.message.contains("list_labels"), "{}", e.message);
    assert_eq!(local(&host, ADA, "a1"), Vec::<String>::new());
    assert!(!remote(&host, ADA, "a1").contains(&"Label_7".to_string()));
    // Snooze needs a real future time.
    let e = err(call(
        &svc,
        "snooze",
        json!({"targets": [target(ADA, "a1")], "until": "tomorrow"}),
    )
    .await);
    assert_eq!(e.code, ErrorCode::InvalidInput);
    let e = err(call(
        &svc,
        "snooze",
        json!({"targets": [target(ADA, "a1")], "until": host.now - 1}),
    )
    .await);
    assert_eq!(e.code, ErrorCode::InvalidInput);
    let iso_until = iso(host.now + DAY_MS);
    organized(
        &svc,
        "snooze",
        json!({"targets": [target(ADA, "a1")], "until": iso_until}),
    )
    .await;
    assert_eq!(
        host.store.get_snooze(ADA, "a1").unwrap().unwrap().wake_at,
        host.now + DAY_MS
    );
}

/// Off and Read only can't organize; Read, organize and draft and the
/// send level can. A refused call changes nothing.
#[tokio::test]
async fn organizing_needs_read_organize_and_draft() {
    let host = TestHost::new("org-level", AgentAccess::Off);
    seed(&host, ADA, "l1", &["INBOX"]);
    for level in [
        AgentAccess::Off,
        AgentAccess::Read,
        AgentAccess::Draft,
        AgentAccess::Send,
    ] {
        host.set_level(level);
        for tool in ORGANIZE_TOOLS {
            let svc = AgentService::new(host.clone());
            let mut args = json!({"targets": [target(ADA, "l1")]});
            if tool.ends_with("_label") {
                args["label"] = json!("Anything");
            }
            if tool == "snooze" {
                args["until"] = json!(host.now + DAY_MS);
            }
            let r = call(&svc, tool, args).await;
            if level >= AgentAccess::Draft {
                assert!(
                    !matches!(&r, Err(e) if e.code == ErrorCode::PermissionDenied),
                    "{level:?} {tool}: {r:?}"
                );
            } else {
                let e = r.unwrap_err();
                assert_eq!(e.code, ErrorCode::PermissionDenied, "{level:?} {tool}");
                assert!(
                    e.message.starts_with("Organizing mail isn't allowed"),
                    "{}",
                    e.message
                );
                assert!(
                    e.message.contains("\"Read, organize and draft\""),
                    "{}",
                    e.message
                );
                assert_eq!(
                    local(&host, ADA, "l1"),
                    strs(&["INBOX"]),
                    "{level:?} {tool}"
                );
            }
        }
        // Put it back for the next level.
        seed(&host, ADA, "l1", &["INBOX"]);
        let _ = host.store.unsnooze(ADA, "l1");
    }
}

/// trash and report_spam: 25 conversations per call, 200 an hour each
/// (rolling); a refused call changes nothing; untrash isn't capped.
#[tokio::test]
async fn trash_and_spam_are_capped_per_call_and_per_hour() {
    let host = TestHost::new("org-caps", AgentAccess::Draft);
    let svc = AgentService::new(host.clone());
    for i in 0..230 {
        seed(&host, ADA, &format!("k{i}"), &["INBOX"]);
    }
    let batch = |from: usize, n: usize| -> Value {
        json!({"targets": (from..from + n).map(|i| target(ADA, &format!("k{i}"))).collect::<Vec<_>>()})
    };
    for tool in ["trash", "report_spam"] {
        let e = call(&svc, tool, batch(0, MAX_CAPPED_PER_CALL + 1))
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidInput, "{tool}");
        assert!(e.message.contains("at most 25"), "{}", e.message);
    }
    assert_eq!(local(&host, ADA, "k0"), strs(&["INBOX"]), "nothing moved");
    // Eight full calls: the hour's 200.
    for n in 0..8 {
        let out = organized(&svc, "trash", batch(n * 25, 25)).await;
        assert_eq!(out.changed.len(), 25);
    }
    let e = call(&svc, "trash", batch(200, 1)).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(
        e.message.contains("at most 200 conversations an hour"),
        "{}",
        e.message
    );
    assert_eq!(
        local(&host, ADA, "k200"),
        strs(&["INBOX"]),
        "refused: untouched"
    );
    // Spam reports have their own allowance; restoring isn't capped.
    organized(&svc, "report_spam", batch(200, 5)).await;
    let out = organized(&svc, "untrash", batch(0, 100)).await;
    assert_eq!(out.changed.len(), 100);
    // An hour later there is room again.
    host.later.store(HOUR_MS + 1, Ordering::Relaxed);
    organized(&svc, "trash", batch(205, 25)).await;
    assert_eq!(local(&host, ADA, "k205"), strs(&["TRASH"]));
}

/// The provider refuses: Penguin puts the conversation back, the answer
/// lists it under failed (not changed), its undo leaves it out, and the
/// user sees the usual action-failed.
#[tokio::test]
async fn a_refusal_is_reported_and_put_back() {
    let host = TestHost::new("org-fail", AgentAccess::Draft);
    let svc = AgentService::new(host.clone());
    seed(&host, ADA, "f1", &["INBOX", "UNREAD"]);
    seed(&host, ADA, "f2", &["INBOX", "UNREAD"]);
    host.fake().fail_next(
        "modify_thread",
        penguin_provider::Error::Other("the server said no".into()),
    );
    let out = organized(
        &svc,
        "archive",
        json!({"targets": [target(ADA, "f1"), target(ADA, "f2")]}),
    )
    .await;
    assert_eq!(out.failed.len(), 1, "{out:?}");
    assert!(out.failed[0].error.contains("the server said no"));
    assert_eq!(out.changed.len(), 1);
    let refused = &out.failed[0].thread_id;
    assert_eq!(
        local(&host, ADA, refused),
        strs(&["INBOX", "UNREAD"]),
        "put back"
    );
    assert_eq!(out.undo[0].arguments.targets.len(), 1);
    assert_ne!(&out.undo[0].arguments.targets[0].thread_id, refused);
    assert_eq!(host.failures.lock().unwrap().len(), 1);
}

/// One audit line per call: tool, client, account, counts, outcome. No
/// subject, sender, label name or thread id.
#[tokio::test]
async fn the_audit_line_has_counts_and_no_content() {
    let host = TestHost::new("org-audit", AgentAccess::Draft);
    seed_labels(&host, ADA);
    let svc = AgentService::new(host.clone());
    seed(&host, ADA, "secret-thread-1", &["INBOX"]);
    seed(&host, ADA, "secret-thread-2", &["INBOX", "Label_7"]);
    organized(
        &svc,
        "add_label",
        json!({"label": "Walrus Project", "targets": [
            target(ADA, "secret-thread-1"), target(ADA, "secret-thread-2"), target(ADA, "secret-thread-3"),
        ]}),
    )
    .await;
    host.set_level(AgentAccess::Read);
    let _ = call(
        &svc,
        "trash",
        json!({"targets": [target(ADA, "secret-thread-1")]}),
    )
    .await;
    let lines = host.audit_lines();
    assert_eq!(lines.len(), 2);
    let l = &lines[0];
    assert_eq!(l["tool"], "add_label");
    assert_eq!(l["via"], "mcp");
    assert_eq!(l["ok"], true);
    assert_eq!(l["account"], ADA);
    assert_eq!(l["threadCount"], 3);
    assert_eq!(l["changedCount"], 1);
    assert_eq!(l["accountCount"], 1);
    assert_eq!(lines[1]["tool"], "trash");
    assert_eq!(lines[1]["errorCode"], "permissionDenied");
    let text = serde_json::to_string(&lines).unwrap();
    for secret in [SUBJECT, SENDER, "Walrus Project", "secret-thread", "Dana"] {
        assert!(!text.contains(secret), "audit has {secret:?}: {text}");
    }
}

/// Undo steps as the UI's Undo runs them (`execute`, no gate): a trash of
/// archived mail comes back archived, in order.
#[tokio::test]
async fn the_toast_undo_runs_the_steps_in_order() {
    let host = TestHost::new("org-toast", AgentAccess::Draft);
    let svc = AgentService::new(host.clone());
    seed(&host, ADA, "u1", &["UNREAD"]);
    let out = organized(&svc, "trash", json!({"targets": [target(ADA, "u1")]})).await;
    let tools: Vec<&str> = out.undo.iter().map(|s| s.tool.as_str()).collect();
    assert_eq!(tools, ["untrash", "archive"]);
    let event = host.organized.lock().unwrap().last().cloned().unwrap();
    for step in event.undo {
        let targets = step.arguments.targets.iter().map(ThreadRef::from).collect();
        for o in execute(
            host.clone(),
            &step.tool,
            targets,
            step.arguments.label.as_deref(),
            step.arguments.until,
            host.now,
        )
        .await
        .unwrap()
        {
            o.pushed().await;
        }
    }
    assert_eq!(local(&host, ADA, "u1"), strs(&["UNREAD"]));
    assert_eq!(remote(&host, ADA, "u1"), strs(&["UNREAD"]));
}
