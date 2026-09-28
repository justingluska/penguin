//! A newsletter shaped like real ESP output, grown to any size (the same
//! generator as penguin-render's tests/perf.rs speed-budget test).
#![allow(dead_code)]

/// A newsletter shaped like real ESP output: a head stylesheet with media
/// queries, nested layout tables, heavy inline styles, remote images,
/// tracked links, and a pixel at the end.
pub fn newsletter(target_bytes: usize) -> String {
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
