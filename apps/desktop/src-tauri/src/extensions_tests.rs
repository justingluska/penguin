use super::*;

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(name: &str) -> Dir {
    let dir = std::env::temp_dir().join(format!("penguin-ext-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Dir(dir)
}

/// `<config>/extensions/<folder>/` with `manifest` and the given files.
fn install(config: &Path, folder: &str, manifest: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = extensions_dir(config).join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(MANIFEST), manifest).unwrap();
    for (name, text) in files {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    dir
}

const TASKS: &str = r#"{
  "penguinExtension": 1,
  "id": "tasks",
  "name": "Tasks",
  "description": "Make a task from an email",
  "icon": "icon.svg",
  "actions": [{
    "id": "create",
    "title": "Create task",
    "program": "run.sh",
    "args": ["--from-penguin"],
    "form": [
      {"id": "title", "label": "Title", "default": "{{subject}}", "required": true},
      {"id": "board", "label": "Board", "type": "select", "options": [{"value": "A", "label": "Alpha"}, {"value": "B", "label": "Beta"}]},
      {"id": "includeEmail", "label": "Include the email", "type": "checkbox"}
    ]
  }]
}"#;

#[test]
fn finds_valid_extensions_and_explains_the_rest() {
    let d = temp("scan");
    install(
        &d.0,
        "tasks",
        TASKS,
        &[
            ("run.sh", "#!/bin/sh\n"),
            ("icon.svg", "<svg xmlns='http://www.w3.org/2000/svg'/>"),
        ],
    );
    install(
        &d.0,
        "twin",
        &TASKS.replace("\"Tasks\"", "\"Twin\""),
        &[("run.sh", "")],
    );
    install(&d.0, "bad-json", "{ nope", &[]);
    install(
        &d.0,
        "escape",
        &TASKS
            .replace("\"tasks\"", "\"escape\"")
            .replace("run.sh", "../run.sh"),
        &[],
    );
    install(
        &d.0,
        "future",
        &TASKS
            .replace("\"penguinExtension\": 1", "\"penguinExtension\": 2")
            .replace("\"tasks\"", "\"future\""),
        &[],
    );
    install(
        &d.0,
        "extra",
        &TASKS
            .replace("\"tasks\"", "\"extra\"")
            .replace("\"icon\":", "\"shell\": true, \"icon\":"),
        &[],
    );
    std::fs::create_dir_all(extensions_dir(&d.0).join("empty")).unwrap();

    let o = overview(&d.0);
    assert_eq!(o.extensions.len(), 1);
    let t = &o.extensions[0];
    assert_eq!(
        (t.id.as_str(), t.folder.as_str(), t.state),
        ("tasks", "tasks", ExtensionState::Off)
    );
    assert!(t
        .icon_svg
        .as_deref()
        .unwrap()
        .starts_with("data:image/svg+xml;base64,"));
    assert_eq!(
        t.actions[0].icon_svg, t.icon_svg,
        "an action without an icon uses the extension's"
    );
    assert_eq!(t.actions[0].command, ["run.sh", "--from-penguin"]);

    let why = |folder: &str| {
        o.problems
            .iter()
            .find(|p| p.folder == folder)
            .map(|p| p.error.clone())
            .unwrap()
    };
    assert!(why("twin").contains("already uses the id"));
    assert!(why("bad-json").contains(MANIFEST));
    assert!(why("escape").contains("inside the extension's folder"));
    assert!(why("future").contains("version 1"));
    assert!(why("extra").contains("unknown field"));
    assert!(why("empty").contains("no extension.json"));
}

#[test]
fn allowing_covers_the_folder_as_it_is_now() {
    let d = temp("allow");
    let dir = install(&d.0, "tasks", TASKS, &[("run.sh", "#!/bin/sh\necho one\n")]);
    let o = set_allowed(&d.0, "tasks", true).unwrap();
    assert_eq!(o.extensions[0].state, ExtensionState::On);

    // Editing the script (or the manifest) needs a new approval.
    std::fs::write(dir.join("run.sh"), "#!/bin/sh\necho two\n").unwrap();
    assert_eq!(overview(&d.0).extensions[0].state, ExtensionState::Changed);
    set_allowed(&d.0, "tasks", true).unwrap();
    std::fs::write(dir.join("notes.txt"), "a new file").unwrap();
    assert_eq!(overview(&d.0).extensions[0].state, ExtensionState::Changed);

    // .git and other dot-folders don't count.
    set_allowed(&d.0, "tasks", true).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".git/HEAD"), "ref").unwrap();
    assert_eq!(overview(&d.0).extensions[0].state, ExtensionState::On);

    // A symlinked folder (a checkout elsewhere) is found and fingerprinted
    // through the link.
    #[cfg(unix)]
    {
        let elsewhere = d.0.join("checkout");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(
            elsewhere.join(MANIFEST),
            TASKS.replace("\"tasks\"", "\"linked\""),
        )
        .unwrap();
        std::fs::write(elsewhere.join("run.sh"), "#!/bin/sh\n").unwrap();
        std::os::unix::fs::symlink(&elsewhere, extensions_dir(&d.0).join("linked")).unwrap();
        let o = set_allowed(&d.0, "linked", true).unwrap();
        let linked = o.extensions.iter().find(|e| e.id == "linked").unwrap();
        assert_eq!(linked.state, ExtensionState::On);
        std::fs::write(elsewhere.join("run.sh"), "#!/bin/sh\necho changed\n").unwrap();
        let o = overview(&d.0);
        let linked = o.extensions.iter().find(|e| e.id == "linked").unwrap();
        assert_eq!(linked.state, ExtensionState::Changed);
    }

    let o = set_allowed(&d.0, "tasks", false).unwrap();
    let tasks = o.extensions.iter().find(|e| e.id == "tasks").unwrap();
    assert_eq!(tasks.state, ExtensionState::Off);
    assert!(set_allowed(&d.0, "missing", true).is_err());
}

#[test]
fn form_answers_are_checked_against_the_fields() {
    let m: Manifest = serde_json::from_str(TASKS).unwrap();
    let fields = &m.actions[0].form;
    let form = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>()
    };

    let ok = check_form(
        fields,
        &form(&[
            ("title", "Fix it"),
            ("board", "B"),
            ("includeEmail", "true"),
        ]),
    )
    .unwrap();
    assert_eq!(
        ok,
        form(&[
            ("title", "Fix it"),
            ("board", "B"),
            ("includeEmail", "true")
        ])
    );
    // Missing optional fields come through empty; a checkbox is true or false.
    let ok = check_form(
        fields,
        &form(&[("title", "Fix it"), ("includeEmail", "yes")]),
    )
    .unwrap();
    assert_eq!(ok["board"], "");
    assert_eq!(ok["includeEmail"], "false");

    assert!(check_form(fields, &form(&[("title", " ")]))
        .unwrap_err()
        .contains("required"));
    assert!(check_form(fields, &form(&[("title", "x"), ("board", "Z")]))
        .unwrap_err()
        .contains("choices"));
    assert!(
        check_form(fields, &form(&[("title", "x"), ("sneaky", "1")]))
            .unwrap_err()
            .contains("unknown")
    );
}

#[test]
fn answers_are_json_or_a_line_and_links_must_be_web() {
    assert_eq!(
        parse_answer(br#"{"message": "Created FOR-12", "url": "https://tasks.example/FOR-12"}"#)
            .unwrap(),
        ExtensionResult {
            message: Some("Created FOR-12".into()),
            url: Some("https://tasks.example/FOR-12".into()),
            confirm: None,
        }
    );
    assert_eq!(
        parse_answer(br#"{"message": "x", "url": "file:///etc/passwd"}"#)
            .unwrap()
            .url,
        None
    );
    assert_eq!(
        parse_answer(br#"{"url": "javascript:alert(1)"}"#).unwrap(),
        ExtensionResult::default()
    );
    assert_eq!(
        parse_answer(b"\n  Done: saved\nmore\n")
            .unwrap()
            .message
            .as_deref(),
        Some("Done: saved")
    );
    assert_eq!(parse_answer(b"").unwrap().message, None);
    let long = "a".repeat(5_000);
    assert_eq!(
        parse_answer(long.as_bytes())
            .unwrap()
            .message
            .unwrap()
            .len(),
        MAX_ANSWER_CHARS
    );
}

#[test]
fn gmail_links_only_for_plain_ids() {
    assert_eq!(
        gmail_url("sam@northwind.example", "18c2f0a1b2"),
        Some("https://mail.google.com/mail/u/sam@northwind.example/#all/18c2f0a1b2".into())
    );
    assert_eq!(gmail_url("a&b@x.example", "18c"), None);
    assert_eq!(gmail_url("a@x.example", "gt:18c"), None);
}

/// The program gets the manifest's argv, runs in its folder, and reads the
/// message only from stdin (no body unless asked); a subject full of shell
/// metacharacters is never executed.
#[cfg(unix)]
#[tokio::test]
async fn the_action_runs_in_its_folder_with_the_message_on_stdin() {
    let d = temp("run");
    let canary = d.0.join("pwned");
    let dir = install(
        &d.0,
        "tasks",
        TASKS,
        &[(
            "run.sh",
            "#!/bin/sh\nprintf '%s\\n' \"$#\" \"$1\" \"$PWD\" \"$PENGUIN_EXTENSION_ID\" > got\ncat >> got\necho '{\"message\": \"Created T-1\", \"url\": \"https://tasks.example/T-1\"}'\n",
        )],
    );
    let (found, _) = scan(&extensions_dir(&d.0));
    let l = &found[0];
    let a = &l.manifest.actions[0];
    let mut m = crate::rules::engine::tests::message("sam@northwind.example", "m1", "t1");
    m.subject = format!("$(touch {c}) `touch {c}`", c = canary.display());
    m.body_text = "secret body".into();
    let form = BTreeMap::from([("title".to_string(), "Fix it".to_string())]);
    let body = serde_json::to_vec(&payload(
        l,
        a,
        &m,
        None,
        &[],
        "sam@northwind.example",
        gmail_url("sam@northwind.example", "t1"),
        form,
        None,
    ))
    .unwrap();
    let env = hooks::hook_env(&[("PENGUIN_EXTENSION_ID", "tasks".into())]);
    let program = program_path(&l.folder, &a.program).unwrap();
    let out = hooks::run_program(
        &program,
        &a.args,
        &env,
        &body,
        Duration::from_secs(10),
        &l.folder,
        Some(MAX_STDOUT),
    )
    .await
    .unwrap();
    assert_eq!(
        parse_answer(&out).unwrap().url.as_deref(),
        Some("https://tasks.example/T-1")
    );

    let got = std::fs::read_to_string(dir.join("got")).unwrap();
    let mut lines = got.lines();
    assert_eq!(lines.next(), Some("1"));
    assert_eq!(lines.next(), Some("--from-penguin"));
    assert_eq!(
        std::fs::canonicalize(lines.next().unwrap()).unwrap(),
        std::fs::canonicalize(&dir).unwrap()
    );
    assert_eq!(lines.next(), Some("tasks"));
    let json: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(json["kind"], "extensionAction");
    assert_eq!(json["data"]["message"]["subject"], m.subject);
    assert_eq!(
        json["data"]["message"]["text"], "",
        "no body unless the action asks"
    );
    assert_eq!(json["data"]["form"]["title"], "Fix it");
    assert_eq!(
        json["data"]["webUrl"],
        "https://mail.google.com/mail/u/sam@northwind.example/#all/t1"
    );
    assert!(!canary.exists(), "the subject was executed");
}

const THREADED: &str = r#"{
  "penguinExtension": 1,
  "id": "notes",
  "name": "Notes",
  "actions": [
    {"id": "thread", "title": "Save conversation", "program": "run.sh", "includeThread": true},
    {"id": "one", "title": "Save message", "program": "run.sh", "includeBody": true}
  ]
}"#;

/// `notes` with THREADED, loaded from a temp config dir.
fn threaded(name: &str) -> (Dir, Loaded) {
    let d = temp(name);
    install(&d.0, "notes", THREADED, &[("run.sh", "#!/bin/sh\n")]);
    let (mut found, problems) = scan(&extensions_dir(&d.0));
    assert!(problems.is_empty(), "{problems:?}");
    (d, found.remove(0))
}

/// Three messages, oldest first; each quotes the one before.
fn conversation() -> ThreadDetail {
    let mut msgs = Vec::new();
    for (i, (id, text)) in [
        ("m1", "Can we move the launch?"),
        ("m2", "Yes, to Friday.\n\n> Can we move the launch?"),
        (
            "m3",
            "Friday works.\n\n> Yes, to Friday.\n> > Can we move the launch?",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut m = crate::rules::engine::tests::message("sam@northwind.example", id, "t1");
        m.date = 1_758_000_000_000 + i as i64 * 60_000;
        m.body_text = text.into();
        msgs.push(m);
    }
    ThreadDetail {
        account_id: "sam@northwind.example".into(),
        thread_id: "t1".into(),
        subject: "Launch".into(),
        label_ids: vec![],
        messages: msgs,
    }
}

fn payload_json(
    l: &Loaded,
    action: &str,
    clicked: usize,
    t: &ThreadDetail,
    pending: &[String],
) -> serde_json::Value {
    let a = l.manifest.actions.iter().find(|a| a.id == action).unwrap();
    serde_json::to_value(payload(
        l,
        a,
        &t.messages[clicked],
        Some(t),
        pending,
        "sam@northwind.example",
        None,
        BTreeMap::new(),
        None,
    ))
    .unwrap()
}

#[test]
fn include_thread_is_a_manifest_field_settings_can_show() {
    let (_d, l) = threaded("thread-info");
    let i = info(&l, &HashMap::new());
    assert!(i.actions[0].include_thread && !i.actions[0].include_body);
    assert!(!i.actions[1].include_thread && i.actions[1].include_body);
    // A misspelling is still an error, not ignored.
    let m = serde_json::from_str::<Manifest>(&THREADED.replace("includeThread", "includeThreads"));
    assert!(m.unwrap_err().to_string().contains("unknown field"));
}

#[test]
fn the_whole_conversation_goes_along_oldest_first() {
    let (_d, l) = threaded("thread-shape");
    let t = conversation();
    // Clicked the middle message; the thread still has all three.
    let json = payload_json(&l, "thread", 1, &t, &[]);
    let data = &json["data"];
    assert_eq!(data["includesThread"], true);
    assert_eq!(data["includesBody"], false);
    assert_eq!(data["message"]["id"], "m2");
    assert_eq!(data["message"]["text"], "", "includeBody is separate");
    let thread = &data["thread"];
    assert_eq!(thread["truncated"], false);
    let msgs = thread["messages"].as_array().unwrap();
    let ids: Vec<&str> = msgs.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["m1", "m2", "m3"]);
    // Quoted history is cut, as for includeBody.
    assert_eq!(msgs[1]["text"], "Yes, to Friday.");
    assert_eq!(msgs[2]["text"], "Friday works.");
    let keys: Vec<&str> = msgs[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "bodyPending",
            "cc",
            "dateIso",
            "from",
            "id",
            "subject",
            "text",
            "to",
            "truncated"
        ]
    );
    assert_eq!(msgs[0]["from"]["email"], "receipts@uber.example");
    assert_eq!(msgs[0]["to"][0]["email"], "sam@northwind.example");
    assert_eq!(msgs[0]["dateIso"], output::iso(t.messages[0].date));
    assert!(msgs
        .iter()
        .all(|m| m["bodyPending"] == false && m["truncated"] == false));
}

#[test]
fn without_include_thread_there_is_no_thread_key() {
    let (_d, l) = threaded("thread-off");
    let t = conversation();
    let json = payload_json(&l, "one", 2, &t, &[]);
    let data = json["data"].as_object().unwrap();
    assert_eq!(data["includesThread"], false);
    assert!(!data.contains_key("thread"));
    assert_eq!(data["message"]["text"], "Friday works.");
}

#[test]
fn a_message_without_its_body_is_marked_pending() {
    let (_d, l) = threaded("thread-pending");
    let mut t = conversation();
    // Headers-only rows have no body text; even stray text isn't sent.
    t.messages[0].body_text = "stale".into();
    let json = payload_json(&l, "thread", 2, &t, &["m1".to_string()]);
    let msgs = json["data"]["thread"]["messages"].as_array().unwrap();
    assert_eq!(
        (msgs[0]["text"].as_str(), msgs[0]["bodyPending"].as_bool()),
        (Some(""), Some(true))
    );
    assert_eq!(msgs[1]["bodyPending"], false);
    assert_eq!(json["data"]["message"]["bodyPending"], false);
    // The clicked message's own flag follows the same list.
    let json = payload_json(&l, "one", 0, &t, &["m1".to_string()]);
    assert_eq!(json["data"]["message"]["bodyPending"], true);
}

#[test]
fn a_long_conversation_drops_its_oldest_messages_first() {
    let t = conversation();
    let mut msgs = t.messages.clone();
    let third = MAX_BODY_CHARS / 3;
    for m in &mut msgs {
        m.body_text = "x".repeat(third + 10);
    }
    // 3 × (a third + 10) doesn't fit: the oldest goes, the newest two stay.
    let out = thread_payload(&msgs, &[]);
    assert!(out.truncated);
    let ids: Vec<&str> = out.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["m2", "m3"]);
    assert!(out.messages.iter().all(|m| !m.truncated));
    let total: usize = out.messages.iter().map(|m| m.text.chars().count()).sum();
    assert!(total <= MAX_BODY_CHARS);

    // Once a message doesn't fit, older ones are left out even if short:
    // what's sent is always the latest part of the conversation.
    msgs[0].body_text = "short".into();
    msgs[1].body_text = "y".repeat(MAX_BODY_CHARS);
    let out = thread_payload(&msgs, &[]);
    let ids: Vec<&str> = out.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!((ids, out.truncated), (vec!["m3"], true));

    // The newest alone is too long: it's cut, and nothing older comes.
    msgs[2].body_text = "z".repeat(MAX_BODY_CHARS + 5);
    let out = thread_payload(&msgs, &[]);
    assert_eq!(out.messages.len(), 1);
    assert!(out.truncated && out.messages[0].truncated);
    assert_eq!(out.messages[0].text.chars().count(), MAX_BODY_CHARS);

    // Exactly at the cap fits.
    let mut two = msgs[..2].to_vec();
    two[0].body_text = "a".repeat(MAX_BODY_CHARS / 2);
    two[1].body_text = "b".repeat(MAX_BODY_CHARS / 2);
    let out = thread_payload(&two, &[]);
    assert_eq!((out.messages.len(), out.truncated), (2, false));
}

fn check_schema(name: &str, schema: rmcp::schemars::Schema) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/cli-schemas");
    let path = dir.join(format!("{name}.v{}.schema.json", output::SCHEMA_VERSION));
    let generated = serde_json::to_string_pretty(&schema).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMAS").is_some() {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing {}; run with UPDATE_SCHEMAS=1", path.display()));
    assert_eq!(on_disk, generated, "{name} schema changed");
}

/// The payload and the confirm answer are the documented, versioned
/// contract for extensions.
#[test]
fn schema_matches_docs() {
    check_schema(
        PAYLOAD_KIND,
        rmcp::schemars::schema_for!(Envelope<ExtensionActionOut>),
    );
    check_schema(
        "extensionConfirm",
        rmcp::schemars::schema_for!(ConfirmAnswer),
    );
}

const CONFIRM: &str = r##"{"confirm": {
  "title": "Close FOR-123 with a comment",
  "detail": "On the Northwind board",
  "steps": [
    {"text": "Comment on FOR-123: “Lost the deal.”", "url": "https://tasks.example/FOR-123"},
    {"text": "Move FOR-123 to Done"}
  ],
  "choices": [
    {"value": "41", "label": "Northwind blog brief", "detail": "#41 · todo · Riley"},
    {"value": "42", "label": "Harbor landing page"}
  ],
  "choiceLabel": "Which task?",
  "confirmLabel": "Close task",
  "state": "{\"task\":123,\"nonce\":\"a1\"}"
}}"##;

#[test]
fn a_confirm_answer_is_parsed_whole() {
    let r = parse_answer(CONFIRM.as_bytes()).unwrap();
    assert_eq!((r.message, r.url), (None, None));
    let c = r.confirm.unwrap();
    assert_eq!(c.title, "Close FOR-123 with a comment");
    assert_eq!(c.detail.as_deref(), Some("On the Northwind board"));
    assert_eq!(c.steps.len(), 2);
    assert_eq!(
        c.steps[0].url.as_deref(),
        Some("https://tasks.example/FOR-123")
    );
    assert_eq!(c.steps[1].url, None);
    assert_eq!(c.choices[0].detail.as_deref(), Some("#41 · todo · Riley"));
    assert_eq!(c.choices[1].detail, None);
    assert_eq!(c.choice_label.as_deref(), Some("Which task?"));
    assert_eq!(c.confirm_label.as_deref(), Some("Close task"));
    assert_eq!(c.state.as_deref(), Some(r#"{"task":123,"nonce":"a1"}"#));

    // The smallest confirm: a title. Blank optional lines read as absent.
    let r = parse_answer(
        br#"{"confirm": {"title": "Archive 3 tasks?", "detail": " ", "confirmLabel": ""}}"#,
    )
    .unwrap();
    let c = r.confirm.unwrap();
    assert_eq!((c.detail, c.confirm_label, c.state), (None, None, None));
    assert!(c.steps.is_empty() && c.choices.is_empty());

    // The UI gets it as camelCase JSON.
    let ui = serde_json::to_value(parse_answer(CONFIRM.as_bytes()).unwrap()).unwrap();
    assert_eq!(ui["confirm"]["choiceLabel"], "Which task?");
    assert_eq!(ui["confirm"]["confirmLabel"], "Close task");
}

#[test]
fn a_confirm_that_breaks_a_rule_is_an_error() {
    let with = |f: &dyn Fn(&mut serde_json::Value)| {
        let mut v: serde_json::Value = serde_json::from_str(CONFIRM).unwrap();
        f(&mut v);
        parse_answer(v.to_string().as_bytes()).unwrap_err()
    };
    assert!(with(&|v| v["confirm"]["colour"] = "red".into()).contains("unknown field `colour`"));
    assert!(
        with(&|v| v["confirm"]["steps"][0]["icon"] = "x".into()).contains("unknown field `icon`")
    );
    assert!(with(&|v| v["confirm"]["choices"][0]["id"] = "x".into()).contains("unknown field `id`"));
    assert!(with(&|v| v["message"] = "also this".into()).contains("unknown field `message`"));
    assert!(with(&|v| v["confirm"] = "yes?".into()).contains("expected struct"));
    assert!(with(&|v| v["confirm"]
        .as_object_mut()
        .unwrap()
        .remove("title")
        .map(|_| ())
        .unwrap())
    .contains("missing field `title`"));
    assert!(with(&|v| v["confirm"]["title"] = "t".repeat(201).into()).contains("title"));
    assert!(with(&|v| v["confirm"]["title"] = " ".into()).contains("title is empty"));
    assert!(with(&|v| v["confirm"]["detail"] = "a\nb".into()).contains("detail"));
    assert!(
        with(&|v| v["confirm"]["steps"] = vec![serde_json::json!({"text": "x"}); 13].into())
            .contains("at most 12 steps")
    );
    assert!(with(&|v| v["confirm"]["steps"][0]["text"] = "s".repeat(301).into()).contains("step"));
    assert!(
        with(&|v| v["confirm"]["steps"][0]["url"] = "file:///etc/passwd".into())
            .contains("http or https")
    );
    assert!(
        with(&|v| v["confirm"]["steps"][0]["url"] = "javascript:alert(1)".into())
            .contains("http or https")
    );
    assert!(with(&|v| {
        v["confirm"]["choices"] = (0..21)
            .map(|i| serde_json::json!({"value": i.to_string(), "label": "x"}))
            .collect::<Vec<_>>()
            .into()
    })
    .contains("at most 20 choices"));
    assert!(with(&|v| v["confirm"]["choices"][1]["value"] = "41".into()).contains("repeated"));
    assert!(with(&|v| v["confirm"]["choices"][0]["value"] = "".into()).contains("value is empty"));
    assert!(
        with(&|v| v["confirm"]["confirmLabel"] = "c".repeat(41).into()).contains("confirmLabel")
    );
    assert!(with(&|v| v["confirm"]["state"] = "s".repeat(8 * 1024 + 1).into()).contains("8 KB"));
    // Exactly at the limits is fine.
    let mut v: serde_json::Value = serde_json::from_str(CONFIRM).unwrap();
    v["confirm"]["title"] = "t".repeat(200).into();
    v["confirm"]["confirmLabel"] = "c".repeat(40).into();
    v["confirm"]["state"] = "s".repeat(8 * 1024).into();
    v["confirm"]["steps"] = vec![serde_json::json!({"text": "x"}); 12].into();
    assert!(parse_answer(v.to_string().as_bytes()).is_ok());

    // Run through the round check, the reason reaches the toast.
    let e =
        answer_for_round(br#"{"confirm": {"title": "x", "colour": 1}}"#, 0, "Tasks").unwrap_err();
    assert!(
        e.message.starts_with(
            "Tasks asked to confirm, but its answer isn't valid: unknown field `colour`"
        ),
        "{}",
        e.message
    );
}

#[test]
fn a_click_confirms_at_most_three_times() {
    let confirm = br#"{"confirm": {"title": "Sure?"}}"#;
    for round in 0..MAX_CONFIRM_ROUNDS {
        assert!(answer_for_round(confirm, round, "Tasks")
            .unwrap()
            .confirm
            .is_some());
    }
    let e = answer_for_round(confirm, MAX_CONFIRM_ROUNDS, "Tasks").unwrap_err();
    assert!(e.message.contains("more than 3 times"), "{}", e.message);
    // The last round may still answer as usual.
    assert_eq!(
        answer_for_round(b"Done", MAX_CONFIRM_ROUNDS, "Tasks")
            .unwrap()
            .message
            .as_deref(),
        Some("Done")
    );
    // And the UI can't claim a round outside 1..=3.
    let c = |round: u32| ContinuationIn {
        state: None,
        choice: None,
        round,
    };
    assert!(check_continuation(&c(0)).is_err());
    assert!(check_continuation(&c(4)).is_err());
    assert!(check_continuation(&c(3)).is_ok());
    let big = ContinuationIn {
        state: Some("s".repeat(8 * 1024 + 1)),
        choice: None,
        round: 1,
    };
    assert!(check_continuation(&big).is_err());
}

#[test]
fn confirming_runs_with_the_state_and_choice_in_the_payload() {
    let (_d, l) = threaded("continuation");
    let t = conversation();
    let a = &l.manifest.actions[0];
    let out = |continuation| {
        serde_json::to_value(payload(
            &l,
            a,
            &t.messages[2],
            Some(&t),
            &[],
            "sam@northwind.example",
            None,
            BTreeMap::from([("title".to_string(), "x".to_string())]),
            continuation,
        ))
        .unwrap()
    };
    let first = out(None);
    assert!(!first["data"]
        .as_object()
        .unwrap()
        .contains_key("continuation"));
    let again = out(Some(ContinuationOut {
        state: Some(r#"{"task":123}"#.into()),
        choice: Some("41".into()),
    }));
    assert_eq!(
        again["data"]["continuation"],
        serde_json::json!({"state": r#"{"task":123}"#, "choice": "41"})
    );
    // Everything else is the same payload as the first run.
    let (mut a1, mut a2) = (first["data"].clone(), again["data"].clone());
    a2.as_object_mut().unwrap().remove("continuation");
    a1.as_object_mut().unwrap().remove("continuation");
    assert_eq!(a1, a2);
    let none = out(Some(ContinuationOut {
        state: None,
        choice: None,
    }));
    assert_eq!(
        none["data"]["continuation"],
        serde_json::json!({"state": null, "choice": null})
    );
}

/// A real program: asks first, then acts when the payload carries the
/// continuation.
#[cfg(unix)]
#[tokio::test]
async fn a_program_asks_then_acts() {
    let d = temp("ask-then-act");
    let script = r#"#!/bin/sh
input=$(cat)
case "$input" in
  *'"continuation"'*'"choice":"41"'*) echo '{"message": "Closed #41", "url": "https://tasks.example/41"}' ;;
  *) echo '{"confirm": {"title": "Close it?", "choices": [{"value": "41", "label": "Brief"}], "state": "s1"}}' ;;
esac
"#;
    install(&d.0, "notes", THREADED, &[("run.sh", script)]);
    let (found, _) = scan(&extensions_dir(&d.0));
    let l = &found[0];
    let a = &l.manifest.actions[1];
    let m = crate::rules::engine::tests::message("sam@northwind.example", "m1", "t1");
    let run = |continuation: Option<ContinuationOut>| {
        let body = serde_json::to_vec(&payload(
            l,
            a,
            &m,
            None,
            &[],
            "sam@northwind.example",
            None,
            BTreeMap::new(),
            continuation,
        ))
        .unwrap();
        let program = program_path(&l.folder, &a.program).unwrap();
        let env = hooks::hook_env(&[]);
        async move {
            hooks::run_program(
                &program,
                &a.args,
                &env,
                &body,
                Duration::from_secs(10),
                &l.folder,
                Some(MAX_STDOUT),
            )
            .await
            .unwrap()
        }
    };
    let first = answer_for_round(&run(None).await, 0, "Notes").unwrap();
    let c = first.confirm.unwrap();
    assert_eq!(c.state.as_deref(), Some("s1"));
    let cont = check_continuation(&ContinuationIn {
        state: c.state,
        choice: Some("41".into()),
        round: 1,
    })
    .unwrap();
    let second = answer_for_round(&run(Some(cont)).await, 1, "Notes").unwrap();
    assert_eq!(
        (
            second.message.as_deref(),
            second.url.as_deref(),
            second.confirm
        ),
        (Some("Closed #41"), Some("https://tasks.example/41"), None)
    );
}

/// Check a real extension folder against this Penguin's rules:
/// `PENGUIN_CHECK_EXTENSION=/path/to/folder cargo test check_extension_folder -- --nocapture`.
/// Skipped without the variable.
#[test]
fn check_extension_folder() {
    let Some(folder) = std::env::var_os("PENGUIN_CHECK_EXTENSION") else {
        return;
    };
    match load_one(Path::new(&folder)) {
        Ok(l) => {
            let i = info(&l, &HashMap::new());
            println!(
                "ok: {} ({}), {} action(s), icon {}",
                i.name,
                i.id,
                i.actions.len(),
                if i.icon_svg.is_some() {
                    "svg"
                } else {
                    "built-in or none"
                }
            );
        }
        Err(e) => panic!("{}: {e}", Path::new(&folder).display()),
    }
}
