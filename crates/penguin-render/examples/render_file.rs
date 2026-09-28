//! Render an HTML (or, with --text, plain-text) email body from a file and
//! print the iframe document. Handy for eyeballing sanitizer output:
//!
//!   cargo run -p penguin-render --example render_file -- body.html [--images] [--text] > out.html

use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .expect("usage: render_file <file> [--images] [--text]");
    let mut input = String::new();
    std::fs::File::open(path)
        .and_then(|mut f| f.read_to_string(&mut input))
        .expect("read input");
    let out = if args.iter().any(|a| a == "--text") {
        penguin_render::render_text(&input)
    } else {
        let opts = penguin_render::RenderOptions {
            allow_remote_images: args.iter().any(|a| a == "--images"),
            ..Default::default()
        };
        penguin_render::render_html(&input, &opts)
    };
    eprintln!(
        "blocked_remote_images={} trackers_removed={}",
        out.blocked_remote_images, out.trackers_removed
    );
    print!("{}", out.html);
}
