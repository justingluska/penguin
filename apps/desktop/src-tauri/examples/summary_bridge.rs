//! Smoke test for the Swift bridge to Apple's Foundation Models (thread
//! summaries, docs/SUMMARIES.md).
//!
//!   cargo run -p penguin-desktop --example summary_bridge
//!
//! Prints what the bridge was built with, what the framework says about
//! this Mac, and runs one small guided summary of a made-up message,
//! printing every event, then one set of suggested replies (the
//! `ReplyOptions` schema). On a Mac with Apple Intelligence on, that's the
//! real on-device model streaming; in a VM (CI) the generation ends at once
//! with `unavailable`, because Apple Intelligence doesn't run in VMs.
//!
//! `EXPECT_FEATURES=<n>` makes it fail unless the build features are `n`
//! (CI pins which SDK APIs each Xcode compiles in).

#[cfg(target_os = "macos")]
fn main() {
    use penguin_desktop_lib::summary::apple::{build_features, AppleEngine};
    use penguin_desktop_lib::summary::engine::{Engine, EngineEvent, GenRequest};

    let features = build_features();
    println!(
        "build features: {features} (1 = FoundationModels, 2 = macOS 26.4 token APIs, 4 = Xcode 27 error types)"
    );
    if let Ok(want) = std::env::var("EXPECT_FEATURES") {
        let want: i32 = want.parse().expect("EXPECT_FEATURES is a number");
        assert_eq!(
            features, want,
            "the bridge was compiled with different APIs than expected"
        );
    }
    let engine = AppleEngine;
    let availability = engine.availability();
    println!("availability: {availability:?}");

    let request = GenRequest {
        instructions: penguin_core::summary::instructions("Thu 2026-09-10"),
        prompt: "Summarize this email conversation.\nSubject: Offsite venue\n\n\
[#1 · Wed 2026-09-09 16:10 · Maya Lin <maya@northwind.example>]\n\
Hi Sam, could you confirm the venue for the offsite by Friday? The lake house has room for 14.\n\n\
[#2 · Thu 2026-09-10 08:45 · You]\nThe lake house works. I'll send the headcount tomorrow."
            .into(),
        guided: true,
        permissive: false,
        stream: true,
        max_response_tokens: 400,
        temperature: Some(0.3),
        schema: None,
    };
    // Then suggested replies (instant replies): guided into `ReplyOptions`.
    let replies = GenRequest {
        instructions: penguin_core::writing::suggestion_instructions("Thu 2026-09-10"),
        prompt: penguin_core::writing::suggestion_prompt(
            "Offsite venue",
            &[penguin_core::summary::SourceMessage {
                number: 1,
                message_id: "m1".into(),
                header: "[#1 · Wed 2026-09-09 16:10 · Maya Lin <maya@northwind.example>]".into(),
                text: "Could you confirm the lake house for the offsite by Friday?".into(),
                preview_only: false,
            }],
        ),
        guided: true,
        permissive: false,
        stream: false,
        max_response_tokens: penguin_core::writing::SUGGESTION_RESPONSE_TOKENS,
        temperature: Some(0.6),
        schema: Some(penguin_desktop_lib::summary::engine::SCHEMA_REPLIES.into()),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for request in [&request, &replies] {
        let terminal = rt.block_on(async {
            let mut generation = engine.start(request);
            while let Some(event) = generation.events.recv().await {
                println!("{event:?}");
                if !matches!(event, EngineEvent::Snapshot(_)) {
                    return Some(event);
                }
            }
            None
        });
        // Exactly one terminal event must arrive, whatever the machine.
        assert!(terminal.is_some(), "the bridge ended without done or error");
        if !availability.available {
            assert!(
                matches!(terminal, Some(EngineEvent::Error(_))),
                "an unavailable model must end in an error"
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("The Foundation Models bridge is macOS only.");
}
