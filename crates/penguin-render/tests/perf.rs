//! Speed budgets for huge messages (release build; debug builds only
//! report the times): `cargo test -p penguin-render --release --test perf`.
//!
//! The budgets are relative to a reference job run in the same test on the
//! same input, so they hold on any machine: a 2 MB newsletter took 32 ms on
//! an Apple M5 Pro and 130 ms on a shared Haswell VM, the same work at a
//! quarter of the speed, and a fixed "under 50 ms" failed on the slow
//! machine while catching nothing. What they catch is Penguin's own cost
//! growing: a render may take at most twice what ammonia's default
//! sanitizer takes on the same input (it parses and serializes the same
//! HTML with html5ever; our policy, image and tracker handling, CSS
//! sanitizing and document assembly come on top), and plain text at most
//! ten times a single escaping pass over it (ammonia's `clean_text`).

use std::time::{Duration, Instant};

use penguin_render::{render_html, render_text, RenderOptions};

/// A newsletter shaped like real ESP output: a head stylesheet with media
/// queries, nested layout tables, heavy inline styles, remote images,
/// tracked links, and a pixel at the end.
fn newsletter(target_bytes: usize) -> String {
    let mut s = String::from(
        r##"<!DOCTYPE html><html><head><meta charset="utf-8"><style type="text/css">
        body{margin:0;padding:0;-webkit-text-size-adjust:100%} table{border-collapse:collapse}
        .container{width:600px;background-color:#ffffff} .btn a{color:#fff!important;text-decoration:none}
        @media only screen and (max-width:600px){.container{width:100%!important}.col{display:block!important;width:100%!important}}
        </style></head><body bgcolor="#f4f4f4" style="margin:0;padding:0"><center>
        <table class="container" width="600" cellpadding="0" cellspacing="0" border="0" align="center">"##,
    );
    let mut i = 0;
    while s.len() < target_bytes {
        s.push_str(&format!(
            r##"<tr><td class="col" valign="top" style="padding:24px 32px;font-family:Helvetica,Arial,sans-serif;font-size:16px;line-height:24px;color:#333333;border-bottom:1px solid #eeeeee">
            <a href="https://click.news.acme.example/ls/click?upn=abc{i}&amp;u=1"><img src="https://img.acme.example/story/{i}.jpg" width="536" height="268" alt="Story {i}" style="display:block;width:100%;max-width:536px;height:auto;border:0;border-radius:8px"></a>
            <h2 style="margin:16px 0 8px;font-size:22px;line-height:28px;color:#111111;font-weight:bold">Headline number {i} &mdash; what changed this week</h2>
            <p style="margin:0 0 12px">Lorem ipsum dolor sit amet, <strong>consectetur</strong> adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud <a href="https://click.news.acme.example/ls/click?upn=def{i}" style="color:#0b63ce;text-decoration:underline">exercitation</a> ullamco laboris.</p>
            <table class="btn" cellpadding="0" cellspacing="0" border="0"><tr><td bgcolor="#0b63ce" style="border-radius:6px;padding:10px 18px;background:linear-gradient(#0b63ce,#0a58b8)"><a href="https://click.news.acme.example/ls/click?upn=ghi{i}" style="color:#ffffff;font-weight:bold;font-size:14px">Read more</a></td></tr></table>
            </td></tr>"##
        ));
        i += 1;
    }
    s.push_str(r##"</table></center><img src="https://news.acme.example/o/open.gif?u=1" width="1" height="1"></body></html>"##);
    s
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// Median times of `ours` and `reference`, run alternately (so a busy
/// machine slows both alike) after a warm-up of each.
fn race<A, B>(
    runs: usize,
    mut ours: impl FnMut() -> A,
    mut reference: impl FnMut() -> B,
) -> (Duration, Duration) {
    std::hint::black_box(ours());
    std::hint::black_box(reference());
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for _ in 0..runs {
        let t = Instant::now();
        std::hint::black_box(ours());
        a.push(t.elapsed());
        let t = Instant::now();
        std::hint::black_box(reference());
        b.push(t.elapsed());
    }
    (median(a), median(b))
}

/// Render time over reference time, at most `budget` (release builds).
fn check_ratio(what: &str, ours: Duration, reference: Duration, budget: f64) {
    let ratio = ours.as_secs_f64() / reference.as_secs_f64();
    eprintln!(
        "{what}: median {ours:?}, reference {reference:?}, ratio {ratio:.2} (budget {budget})"
    );
    if !cfg!(debug_assertions) {
        assert!(
            ratio <= budget,
            "{what} took {ours:?}, {ratio:.2}x the reference's {reference:?} (budget {budget}x)"
        );
    }
}

#[test]
fn two_megabyte_newsletter_renders_fast() {
    let html = newsletter(2 * 1024 * 1024);
    assert!(html.len() >= 2 * 1024 * 1024);
    for opts in [
        RenderOptions::default(),
        RenderOptions {
            allow_remote_images: true,
            ..Default::default()
        },
    ] {
        let r = render_html(&html, &opts);
        assert_eq!(r.trackers_removed, 1);
        let (ours, reference) = race(
            7,
            || render_html(std::hint::black_box(&html), &opts),
            || ammonia::clean(std::hint::black_box(&html)),
        );
        // ~1.3x on the Haswell VM.
        check_ratio(
            &format!(
                "render_html 2 MB (allow_remote_images={})",
                opts.allow_remote_images
            ),
            ours,
            reference,
            2.0,
        );
    }
}

#[test]
fn large_plain_text_renders_fast() {
    let line = "> quoted line with a link https://acme.example/x and mail ada@lovelace.example\n";
    let text = format!(
        "Hello\n\nOn Mon, Ada wrote:\n{}",
        line.repeat(2 * 1024 * 1024 / line.len())
    );
    assert!(render_text(&text).html.contains("pg-quote"));
    let (ours, reference) = race(
        5,
        || render_text(std::hint::black_box(&text)),
        || ammonia::clean_text(std::hint::black_box(&text)),
    );
    // ~6x on the Haswell VM: links and addresses on every line.
    check_ratio("render_text 2 MB", ours, reference, 10.0);
}
