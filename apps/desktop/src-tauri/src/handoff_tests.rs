use super::*;
use serde_json::json;

const ACCOUNT: &str = "Avery@Northwind.example";

fn conversation(subject: Option<&str>) -> HandoffActivity {
    HandoffActivity::Conversation {
        account_id: ACCOUNT.into(),
        thread_id: "18c2f0a1b2c3d4e5".into(),
        subject: subject.map(str::to_string),
    }
}

fn draft(thread: Option<&str>, subject: Option<&str>) -> HandoffActivity {
    HandoffActivity::Draft {
        account_id: ACCOUNT.into(),
        draft_id: "r-4417735219038857123".into(),
        thread_id: thread.map(str::to_string),
        subject: subject.map(str::to_string),
    }
}

// ---------- publishing ----------

#[test]
fn the_ui_sends_tagged_activities() {
    let a: HandoffActivity = serde_json::from_value(json!({
        "kind": "conversation", "accountId": ACCOUNT, "threadId": "t1", "subject": "Lunch"
    }))
    .unwrap();
    assert_eq!(
        a,
        HandoffActivity::Conversation {
            account_id: ACCOUNT.into(),
            thread_id: "t1".into(),
            subject: Some("Lunch".into())
        }
    );
    let d: HandoffActivity = serde_json::from_value(json!({
        "kind": "draft", "accountId": ACCOUNT, "draftId": "r-1", "threadId": null, "subject": null
    }))
    .unwrap();
    assert!(matches!(
        d,
        HandoffActivity::Draft {
            thread_id: None,
            ..
        }
    ));
    let n: HandoffActivity = serde_json::from_value(json!({ "kind": "none" })).unwrap();
    assert_eq!(n, HandoffActivity::None);
    // Unknown kinds and fields are refused (a body must never ride along).
    assert!(serde_json::from_value::<HandoffActivity>(json!({ "kind": "message" })).is_err());
    assert!(serde_json::from_value::<HandoffActivity>(json!({
        "kind": "conversation", "accountId": ACCOUNT, "threadId": "t1", "body": "hi"
    }))
    .is_err());
}

#[test]
fn a_conversation_carries_ids_and_the_subject_only() {
    let a = conversation(Some("Quarterly  plan\n review"))
        .advert()
        .unwrap()
        .unwrap();
    assert_eq!(a.activity_type, "co.gluska.penguin.conversation");
    assert_eq!(a.title, "Quarterly plan review");
    assert_eq!(
        a.user_info,
        json!({ "v": 1, "account": "avery@northwind.example", "thread": "18c2f0a1b2c3d4e5" })
    );
    assert_eq!(
        conversation(None).advert().unwrap().unwrap().title,
        "(no subject)"
    );
    assert_eq!(
        conversation(Some("  ")).advert().unwrap().unwrap().title,
        "(no subject)"
    );
}

#[test]
fn a_draft_carries_its_gmail_id_and_the_thread_it_answers() {
    let a = draft(Some("t9"), Some("Re: Lunch"))
        .advert()
        .unwrap()
        .unwrap();
    assert_eq!(a.activity_type, "co.gluska.penguin.draft");
    assert_eq!(a.title, "Re: Lunch");
    assert_eq!(
        a.user_info,
        json!({ "v": 1, "account": "avery@northwind.example", "draftId": "r-4417735219038857123", "thread": "t9" })
    );
    // A new message: no thread key at all, and "New message" as the title.
    let n = draft(None, None).advert().unwrap().unwrap();
    assert_eq!(n.title, "New message");
    assert!(n.user_info.get("thread").is_none());
}

#[test]
fn malformed_activities_are_refused() {
    assert_eq!(HandoffActivity::None.advert().unwrap(), None);
    let bad = [
        HandoffActivity::Conversation {
            account_id: "".into(),
            thread_id: "t".into(),
            subject: None,
        },
        HandoffActivity::Conversation {
            account_id: ACCOUNT.into(),
            thread_id: "t\n1".into(),
            subject: None,
        },
        HandoffActivity::Conversation {
            account_id: ACCOUNT.into(),
            thread_id: "x".repeat(513),
            subject: None,
        },
        draft(Some(""), None),
        HandoffActivity::Draft {
            account_id: ACCOUNT.into(),
            draft_id: "".into(),
            thread_id: None,
            subject: None,
        },
    ];
    for a in bad {
        assert!(a.advert().is_err(), "{a:?}");
    }
    // A long subject is cut, not refused.
    let long = conversation(Some(&"é".repeat(400)))
        .advert()
        .unwrap()
        .unwrap();
    assert_eq!(long.title.chars().count(), 200);
}

// ---------- which window's activity is advertised ----------

fn advert(thread: &str) -> Advert {
    HandoffActivity::Conversation {
        account_id: ACCOUNT.into(),
        thread_id: thread.into(),
        subject: None,
    }
    .advert()
    .unwrap()
    .unwrap()
}

#[test]
fn the_front_window_is_advertised() {
    let mut w = Windows::default();
    // Before any focus event the main window counts as in front.
    assert_eq!(
        w.set("main", Some(advert("a"))),
        Some(Change::Publish(advert("a")))
    );
    // The same again changes nothing.
    assert_eq!(w.set("main", Some(advert("a"))), None);
    // A conversation window reports while in the back: nothing changes yet.
    assert_eq!(w.set("thread-1", Some(advert("b"))), None);
    assert_eq!(w.focused("thread-1"), Some(Change::Publish(advert("b"))));
    // Back to the main window.
    assert_eq!(w.focused("main"), Some(Change::Publish(advert("a"))));
    // The main window hides its conversation (minimized, a new message).
    assert_eq!(w.set("main", None), Some(Change::Resign));
    assert_eq!(w.set("main", None), None);
    // A window with nothing to offer in front (Settings, the image viewer).
    assert_eq!(
        w.set("main", Some(advert("c"))),
        Some(Change::Publish(advert("c")))
    );
    assert_eq!(w.focused("viewer"), Some(Change::Resign));
}

#[test]
fn a_closed_window_stops_being_advertised() {
    let mut w = Windows::default();
    w.set("main", Some(advert("a")));
    w.set("compose-1", Some(advert("d")));
    assert_eq!(w.focused("compose-1"), Some(Change::Publish(advert("d"))));
    // Closing it falls back to the main window's until another comes forward.
    assert_eq!(w.gone("compose-1"), Some(Change::Publish(advert("a"))));
    assert!(!w.by_window.contains_key("compose-1"));
    // A window that closes in the back changes nothing.
    w.set("thread-2", Some(advert("e")));
    assert_eq!(w.gone("thread-2"), None);
}

// ---------- receiving ----------

fn incoming(kind: &str, info: serde_json::Value) -> Option<HandoffOpen> {
    parse_incoming(kind, info.to_string().as_bytes())
}

#[test]
fn a_conversation_from_the_iphone_opens() {
    assert_eq!(
        incoming(
            TYPE_CONVERSATION,
            json!({ "v": 1, "account": "Avery@Northwind.example", "thread": "t1" })
        ),
        Some(HandoffOpen::Conversation {
            account_id: "avery@northwind.example".into(),
            thread_id: "t1".into()
        })
    );
}

#[test]
fn a_draft_from_the_iphone_opens() {
    assert_eq!(
        incoming(
            TYPE_DRAFT,
            json!({ "v": 1, "account": ACCOUNT, "draftId": "r-1", "thread": "t1" })
        ),
        Some(HandoffOpen::Draft {
            account_id: "avery@northwind.example".into(),
            draft_id: "r-1".into(),
            thread_id: Some("t1".into())
        })
    );
    assert_eq!(
        incoming(
            TYPE_DRAFT,
            json!({ "v": 1, "account": ACCOUNT, "draftId": "r-1" })
        ),
        Some(HandoffOpen::Draft {
            account_id: "avery@northwind.example".into(),
            draft_id: "r-1".into(),
            thread_id: None
        })
    );
}

#[test]
fn what_the_mac_sends_reads_back() {
    for (activity, kind) in [
        (conversation(Some("Hi")), TYPE_CONVERSATION),
        (draft(Some("t9"), None), TYPE_DRAFT),
        (draft(None, None), TYPE_DRAFT),
    ] {
        let a = activity.advert().unwrap().unwrap();
        assert_eq!(a.activity_type, kind);
        let open = parse_incoming(a.activity_type, a.user_info.to_string().as_bytes()).unwrap();
        assert!(!matches!(open, HandoffOpen::Unreadable { .. }), "{open:?}");
    }
}

#[test]
fn other_activity_types_are_not_ours() {
    assert_eq!(incoming("NSUserActivityTypeBrowsingWeb", json!({})), None);
    assert_eq!(
        incoming("co.gluska.penguin.search", json!({ "v": 1 })),
        None
    );
}

#[test]
fn unreadable_activities_say_so() {
    let conv = Some(HandoffOpen::Unreadable {
        activity: ActivityKind::Conversation,
    });
    let dr = Some(HandoffOpen::Unreadable {
        activity: ActivityKind::Draft,
    });
    // A newer version, a missing or wrongly typed version.
    assert_eq!(
        incoming(
            TYPE_CONVERSATION,
            json!({ "v": 2, "account": ACCOUNT, "thread": "t" })
        ),
        conv
    );
    assert_eq!(
        incoming(
            TYPE_CONVERSATION,
            json!({ "account": ACCOUNT, "thread": "t" })
        ),
        conv
    );
    assert_eq!(
        incoming(
            TYPE_CONVERSATION,
            json!({ "v": "1", "account": ACCOUNT, "thread": "t" })
        ),
        conv
    );
    // Missing, empty, wrongly typed or control-character fields.
    assert_eq!(
        incoming(TYPE_CONVERSATION, json!({ "v": 1, "account": ACCOUNT })),
        conv
    );
    assert_eq!(
        incoming(
            TYPE_CONVERSATION,
            json!({ "v": 1, "account": "", "thread": "t" })
        ),
        conv
    );
    assert_eq!(
        incoming(
            TYPE_CONVERSATION,
            json!({ "v": 1, "account": ACCOUNT, "thread": 7 })
        ),
        conv
    );
    assert_eq!(
        incoming(TYPE_DRAFT, json!({ "v": 1, "account": ACCOUNT })),
        dr
    );
    assert_eq!(
        incoming(
            TYPE_DRAFT,
            json!({ "v": 1, "account": ACCOUNT, "draftId": "r\u{0}1" })
        ),
        dr
    );
    assert_eq!(
        incoming(
            TYPE_DRAFT,
            json!({ "v": 1, "account": ACCOUNT, "draftId": "r-1", "thread": "" })
        ),
        dr
    );
    // Not JSON, or not an object.
    assert_eq!(parse_incoming(TYPE_CONVERSATION, b"not json"), conv);
    assert_eq!(parse_incoming(TYPE_DRAFT, b"[1]"), dr);
    assert_eq!(parse_incoming(TYPE_DRAFT, b""), dr);
}

#[test]
fn the_open_reaches_the_ui_in_its_shape() {
    let v = serde_json::to_value(HandoffOpen::Draft {
        account_id: "a@b.example".into(),
        draft_id: "r-1".into(),
        thread_id: None,
    })
    .unwrap();
    assert_eq!(
        v,
        json!({ "kind": "draft", "accountId": "a@b.example", "draftId": "r-1", "threadId": null })
    );
    let u = serde_json::to_value(HandoffOpen::Unreadable {
        activity: ActivityKind::Conversation,
    })
    .unwrap();
    assert_eq!(
        u,
        json!({ "kind": "unreadable", "activity": "conversation" })
    );
}

#[test]
fn the_pending_open_is_taken_once() {
    let h = Handoff::default();
    assert_eq!(h.take_pending(), None);
    h.set_pending(HandoffOpen::Conversation {
        account_id: "a@b.example".into(),
        thread_id: "t".into(),
    });
    assert!(h.take_pending().is_some());
    assert_eq!(h.take_pending(), None);
}

#[test]
fn info_plist_declares_both_types() {
    let plist = include_str!("../Info.plist");
    for t in [TYPE_CONVERSATION, TYPE_DRAFT] {
        assert!(plist.contains(&format!("<string>{t}</string>")), "{t}");
    }
    assert!(plist.contains("<key>NSUserActivityTypes</key>"));
}
