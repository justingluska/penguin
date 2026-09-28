//! Sanitizer profiles for the rich-text composer. OWNER: richtext agent.
//!
//! Much stricter than `render_html`: the composer only knows paragraphs,
//! basic marks, links, lists, quotes, code and a few headings, so anything
//! else is dropped here before it can reach the editor or leave in a mail.
//!
//! - [`sanitize_compose_html`]: HTML entering the editor (pasted web/Google
//!   Docs content, a reopened draft, a signature). Keeps only the inline
//!   styles the editor reads as formatting (bold/italic/underline/strike).
//! - [`sanitize_outgoing_html`]: the composer's own HTML part on its way into
//!   a MIME message. Same tags, plus the minimal inline CSS mail clients need
//!   (font stack, margins, quote border), with values restricted to a
//!   character set that can't express `url()`, `expression()` or escapes.
//! - [`sanitize_quoted_html`]: a received message's HTML body, quoted in a
//!   reply or forward. The outgoing profile, after table markup is turned
//!   into blocks (rows and cells become `<div>`s, so cells don't run
//!   together). Its output is a fixpoint of [`sanitize_outgoing_html`], so the
//!   quote survives the send-time pass unchanged.
//!
//! No ids, no event handlers, no `style`/`script` content, and links are
//! http(s) (with a host) or mailto only. Images only in the `_with_images`
//! variants, and only as `cid:` references to parts the message itself
//! carries ([`CidMap`]): never a remote, `data:` or relative URL, so a quoted
//! original can't make the recipient (or the composer) load anything.
//!
//! Output is parser-stable, so one call is a fixpoint. ammonia unwraps
//! disallowed elements, and unwrapping a table, button or similar can leave
//! allowed elements nested where the HTML parser would restructure them on
//! the next parse (`<a>` in `<a>`, a block inside `<p>`, `<li>` in `<li>`,
//! `<h2>` directly in `<h1>`, a `<pre>` whose text starts with a newline).
//! [`stabilize`] rebuilds ammonia's tree from its output, unwraps those
//! (keeping their content), and serializes it once with html5ever, so the
//! DOM a browser builds from the output is exactly the tree we checked and a
//! reopened draft never drifts.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;

use ammonia::{Builder, UrlRelative};
use html5ever::serialize::{serialize, Serialize, SerializeOpts, Serializer, TraversalScope};
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use html5ever::{ns, Attribute, LocalName, QualName};

const TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "del",
    "div",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "li",
    "ol",
    "p",
    "pre",
    "s",
    "span",
    "strike",
    "strong",
    "u",
    "ul",
];

/// Removed with their content (everything else unknown is unwrapped).
const CLEAN_CONTENT_TAGS: &[&str] = &[
    "script",
    "style",
    "title",
    "noscript",
    "template",
    "iframe",
    "frame",
    "frameset",
    "object",
    "embed",
    "applet",
    "noembed",
    "noframes",
    "xmp",
    "plaintext",
    "textarea",
    "select",
    "option",
    "svg",
    "math",
    "head",
    "meta",
    "link",
    "base",
    "audio",
    "video",
    "canvas",
    "map",
    "button",
];

/// Classes that carry meaning for the composer (on `div`): our signature
/// block and the quoted original with its attribution line (Gmail's class
/// names, so Gmail recipients get it folded).
const CLASSES: &[&str] = &[
    "penguin-signature",
    "gmail_signature",
    "gmail_quote",
    "gmail_attr",
];

/// Classes allowed on `blockquote`: Gmail marks a reply's quoted original
/// `<blockquote class="gmail_quote">`.
const BLOCKQUOTE_CLASSES: &[&str] = &["gmail_quote"];

#[derive(Clone, Copy, PartialEq)]
enum Profile {
    Editor,
    Outgoing,
}

/// Inline images a profile keeps: a `cid:` image whose Content-ID,
/// normalized ([`crate::normalize_cid`]: no brackets, percent-decoded,
/// lowercase), is a key here is kept and written as `cid:<value>`; every
/// other image goes. Build it with [`cid_map`] (rewrite to fresh ids) or
/// [`keep_cids`] (keep these ids). Values that aren't [`is_safe_cid`] are
/// never written.
pub type CidMap = HashMap<String, String>;

/// A [`CidMap`] from (original Content-ID, id to write) pairs.
pub fn cid_map<I, A, B>(pairs: I) -> CidMap
where
    I: IntoIterator<Item = (A, B)>,
    A: AsRef<str>,
    B: Into<String>,
{
    pairs
        .into_iter()
        .map(|(k, v)| (crate::normalize_cid(k.as_ref()), v.into()))
        .filter(|(_, v)| is_safe_cid(v))
        .collect()
}

/// A [`CidMap`] that keeps references to these Content-IDs as they are.
pub fn keep_cids<I, A>(cids: I) -> CidMap
where
    I: IntoIterator<Item = A>,
    A: AsRef<str>,
{
    cid_map(cids.into_iter().map(|c| {
        let c = bare_cid(c.as_ref()).to_string();
        (c.clone(), c)
    }))
}

fn bare_cid(cid: &str) -> &str {
    cid.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
}

/// A Content-ID Penguin will write into HTML and into a `Content-ID`
/// header: 1–200 characters of `A-Z a-z 0-9 . _ @ + = -` (what Gmail,
/// Outlook, Apple Mail and Penguin itself mint), so it needs no escaping in
/// an attribute, a URL or a header.
pub fn is_safe_cid(cid: &str) -> bool {
    (1..=200).contains(&cid.len())
        && cid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._@+=-".contains(&b))
}

/// The Content-IDs a sanitized fragment's images reference, in order,
/// without duplicates. Only for output of this module, where every image
/// source is `cid:` plus an [`is_safe_cid`] id.
pub fn referenced_cids(sanitized: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in sanitized.split("src=\"cid:").skip(1) {
        if let Some(end) = part.find('"') {
            let cid = &part[..end];
            if is_safe_cid(cid) && !out.iter().any(|c| c == cid) {
                out.push(cid.to_string());
            }
        }
    }
    out
}

/// HTML about to be loaded into the composer (paste, signature): no images.
pub fn sanitize_compose_html(html: &str) -> String {
    sanitize_compose_html_with_images(html, &CidMap::new())
}

/// [`sanitize_compose_html`] keeping the inline images `cids` names (a
/// reopened draft's own inline parts).
pub fn sanitize_compose_html_with_images(html: &str, cids: &CidMap) -> String {
    stabilize(&builder(Profile::Editor, cids).clean(html).to_string())
}

/// The composer's HTML part, right before it goes into a MIME message: no
/// images.
pub fn sanitize_outgoing_html(html: &str) -> String {
    sanitize_outgoing_html_with_images(html, &CidMap::new())
}

/// [`sanitize_outgoing_html`] keeping the images `cids` names: the
/// message's own inline parts (a pasted image, a quoted original's images).
pub fn sanitize_outgoing_html_with_images(html: &str, cids: &CidMap) -> String {
    stabilize(&builder(Profile::Outgoing, cids).clean(html).to_string())
}

/// A received message's HTML body, about to be quoted in a reply or forward:
/// headings, emphasis, lists, links, quotes and the outgoing profile's
/// presentation styles stay; scripts, styles sheets, images (remote or not),
/// forms and everything else go. Table rows and cells become `<div>`s first.
pub fn sanitize_quoted_html(html: &str) -> String {
    sanitize_quoted_html_with_images(html, &CidMap::new())
}

/// [`sanitize_quoted_html`] keeping the original's inline (`cid:`) images
/// that `cids` maps, rewritten to the ids the new message gives their
/// copies. Remote images are still dropped: quoting must never make the
/// recipient's client fetch what the original's sender linked to.
pub fn sanitize_quoted_html_with_images(html: &str, cids: &CidMap) -> String {
    sanitize_outgoing_html_with_images(&tables_to_blocks(html), cids)
}

/// Table elements the quote keeps as blocks (renamed to `div`).
const TABLE_TAGS: &[&str] = &[
    "table", "caption", "thead", "tbody", "tfoot", "tr", "td", "th",
];

/// Rename table start and end tags to `div`, leaving their attributes for the
/// sanitizer to filter. A plain scan, not a parse: a match inside a comment,
/// a script or an attribute value is harmless, since the sanitizer runs on
/// the result anyway (this only decides how cell text is laid out).
fn tables_to_blocks(html: &str) -> Cow<'_, str> {
    let b = html.as_bytes();
    let mut out = String::new();
    let mut copied = 0;
    let mut from = 0;
    while let Some(off) = html[from..].find('<') {
        let at = from + off;
        from = at + 1;
        let start = if b.get(at + 1) == Some(&b'/') {
            at + 2
        } else {
            at + 1
        };
        let len = b[start.min(b.len())..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        let end = start + len;
        let ends_name = matches!(
            b.get(end),
            None | Some(b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
        );
        if len == 0 || !ends_name {
            continue;
        }
        let name = &html[start..end];
        if TABLE_TAGS.iter().any(|t| t.eq_ignore_ascii_case(name)) {
            out.push_str(&html[copied..start]);
            out.push_str("div");
            copied = end;
        }
    }
    if copied == 0 {
        return Cow::Borrowed(html);
    }
    out.push_str(&html[copied..]);
    Cow::Owned(out)
}

// ---------------------------------------------------------------------------
// Parser-stable output

/// Start tags that close an open `<p>` (the parser checks "p in button
/// scope", and none of the allowed tags is a scope boundary, so any of
/// these anywhere inside a `<p>` splits it).
const P_CLOSERS: &[&str] = &[
    "blockquote",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "li",
    "ol",
    "p",
    "pre",
    "ul",
];

/// Special elements that stop the parser's search for an open `<li>` to
/// close (all but address/div/p, restricted to the allowed tags).
const LI_BARRIERS: &[&str] = &[
    "blockquote",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ol",
    "pre",
    "ul",
];

/// Deeper nesting than this is flattened (the extra elements are unwrapped),
/// which keeps every walk below bounded. The editor makes nothing near it.
const MAX_DEPTH: usize = 256;

fn is_heading(tag: &str) -> bool {
    matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

/// ammonia's output as a tree: well-nested, only the allowed tags.
enum Tree {
    Element {
        name: QualName,
        attrs: Vec<Attribute>,
        kids: Vec<Tree>,
    },
    Text(String),
}

impl Tree {
    fn tag(&self) -> Option<&str> {
        match self {
            Tree::Element { name, .. } => Some(&name.local),
            Tree::Text(_) => None,
        }
    }

    fn kids(&self) -> &[Tree] {
        match self {
            Tree::Element { kids, .. } => kids,
            Tree::Text(_) => &[],
        }
    }

    fn write<S: Serializer>(&self, s: &mut S) -> io::Result<()> {
        match self {
            Tree::Element { name, attrs, kids } => {
                s.start_elem(name.clone(), attrs.iter().map(|a| (&a.name, &a.value[..])))?;
                for k in kids {
                    k.write(s)?;
                }
                s.end_elem(name.clone())
            }
            Tree::Text(t) => s.write_text(t),
        }
    }
}

struct Fragment(Vec<Tree>);

impl Serialize for Fragment {
    fn serialize<S: Serializer>(&self, s: &mut S, _scope: TraversalScope) -> io::Result<()> {
        self.0.iter().try_for_each(|t| t.write(s))
    }
}

/// Make ammonia's serialized tree parser-stable (see the module docs).
///
/// `html` is ammonia output: well-nested, attribute values quoted and
/// escaped, only the allowed tags. Tokenizing it (no tree builder, so no
/// restructuring) gives back ammonia's own tree exactly. html5ever's
/// serializer writes the result, so escaping is the same as ammonia's.
fn stabilize(html: &str) -> String {
    let mut kids = rebuild(html);
    while fix(&mut kids, None, &mut Vec::new()) {}
    pre_newlines(&mut kids);
    let mut out = Vec::with_capacity(html.len() + 16);
    let opts = SerializeOpts {
        traversal_scope: TraversalScope::ChildrenOnly(None),
        ..Default::default()
    };
    serialize(&mut out, &Fragment(kids), opts).expect("writing to a Vec");
    String::from_utf8(out).expect("html5ever writes UTF-8")
}

/// Unwrap (replace by their children) the allowed elements the parser would
/// restructure. Returns whether anything changed; the caller repeats until
/// nothing does, since an unwrap moves children into a new context.
fn fix(kids: &mut Vec<Tree>, parent: Option<&str>, ancestors: &mut Vec<String>) -> bool {
    let mut changed = false;
    let mut kept = Vec::with_capacity(kids.len());
    for child in std::mem::take(kids) {
        if must_unwrap(&child, parent, ancestors) {
            changed = true;
            if let Tree::Element { kids: inner, .. } = child {
                kept.extend(inner);
            }
        } else {
            kept.push(child);
        }
    }
    *kids = kept;
    for child in kids.iter_mut() {
        if let Tree::Element {
            name, kids: inner, ..
        } = child
        {
            ancestors.push(name.local.to_string());
            changed |= fix(inner, Some(&name.local), ancestors);
            ancestors.pop();
        }
    }
    changed
}

/// `ancestors` ends with `parent`.
fn must_unwrap(child: &Tree, parent: Option<&str>, ancestors: &[String]) -> bool {
    let Some(tag) = child.tag() else {
        return false;
    };
    match tag {
        // An <a> start tag closes any open <a> (adoption agency).
        "a" => ancestors.iter().any(|a| a == "a"),
        // A block start tag closes the <p> around it.
        "p" if has_descendant(child, P_CLOSERS) => true,
        // A heading start tag pops a heading that is the current node.
        t if is_heading(t) => parent.is_some_and(is_heading),
        // An image whose source the attribute filter refused shows nothing.
        "img" => !has_attr(child, "src"),
        // An <li> start tag closes an open <li> unless a list, quote,
        // heading or pre stands between them.
        "li" => {
            for a in ancestors.iter().rev() {
                if a == "li" {
                    return true;
                }
                if LI_BARRIERS.contains(&a.as_str()) {
                    return false;
                }
            }
            false
        }
        _ => false,
    }
}

fn has_attr(node: &Tree, attr: &str) -> bool {
    match node {
        Tree::Element { attrs, .. } => attrs.iter().any(|a| &*a.name.local == attr),
        Tree::Text(_) => false,
    }
}

fn has_descendant(node: &Tree, tags: &[&str]) -> bool {
    node.kids()
        .iter()
        .any(|c| c.tag().is_some_and(|t| tags.contains(&t)) || has_descendant(c, tags))
}

/// A `<pre>` whose text starts with a newline gets one more: the parser
/// drops the first newline after `<pre>`, and the serializer doesn't add it.
fn pre_newlines(kids: &mut [Tree]) {
    for k in kids {
        if let Tree::Element {
            name, kids: inner, ..
        } = k
        {
            if &*name.local == "pre" {
                if let Some(Tree::Text(t)) = inner.first_mut() {
                    if t.starts_with('\n') {
                        t.insert(0, '\n');
                    }
                }
            }
            pre_newlines(inner);
        }
    }
}

/// Open elements while tokenizing; the bottom frame is the fragment root.
struct Frame {
    name: Option<QualName>,
    attrs: Vec<Attribute>,
    kids: Vec<Tree>,
}

/// Tree builder over bare tokens: each start tag opens a child of the
/// current element (br/hr/img are void), each end tag closes back to its match,
/// text is appended.
struct TreeSink {
    stack: RefCell<Vec<Frame>>,
    /// Start tags past MAX_DEPTH that were dropped (their end tags too).
    skipped: RefCell<Vec<LocalName>>,
}

impl TreeSink {
    fn close_top(stack: &mut Vec<Frame>) {
        let f = stack.pop().expect("caller keeps the root");
        let el = Tree::Element {
            name: f.name.expect("only the root has no name"),
            attrs: f.attrs,
            kids: f.kids,
        };
        stack.last_mut().expect("root").kids.push(el);
    }
}

impl TokenSink for TreeSink {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut stack = self.stack.borrow_mut();
        match token {
            Token::TagToken(tag) => match tag.kind {
                TagKind::StartTag => {
                    let void = matches!(&*tag.name, "br" | "hr" | "img") || tag.self_closing;
                    if stack.len() > MAX_DEPTH && !void {
                        self.skipped.borrow_mut().push(tag.name);
                        return TokenSinkResult::Continue;
                    }
                    let frame = Frame {
                        name: Some(QualName::new(None, ns!(html), tag.name)),
                        attrs: tag.attrs,
                        kids: Vec::new(),
                    };
                    stack.push(frame);
                    if void {
                        Self::close_top(&mut stack);
                    }
                }
                TagKind::EndTag => {
                    let mut skipped = self.skipped.borrow_mut();
                    if skipped.last() == Some(&tag.name) {
                        skipped.pop();
                        return TokenSinkResult::Continue;
                    }
                    let open = stack
                        .iter()
                        .rposition(|f| f.name.as_ref().is_some_and(|n| n.local == tag.name));
                    if let Some(i) = open.filter(|&i| i > 0) {
                        while stack.len() > i {
                            Self::close_top(&mut stack);
                        }
                    }
                }
            },
            Token::CharacterTokens(t) => {
                let kids = &mut stack.last_mut().expect("root").kids;
                match kids.last_mut() {
                    Some(Tree::Text(prev)) => prev.push_str(&t),
                    _ => kids.push(Tree::Text(t.to_string())),
                }
            }
            // ammonia emits no comments, doctypes or NULs.
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

fn rebuild(html: &str) -> Vec<Tree> {
    let sink = TreeSink {
        stack: RefCell::new(vec![Frame {
            name: None,
            attrs: Vec::new(),
            kids: Vec::new(),
        }]),
        skipped: RefCell::new(Vec::new()),
    };
    let tok = Tokenizer::new(sink, TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    let _ = tok.feed(&input);
    tok.end();
    let mut stack = tok.sink.stack.into_inner();
    while stack.len() > 1 {
        TreeSink::close_top(&mut stack);
    }
    stack.pop().map(|f| f.kids).unwrap_or_default()
}

fn builder(profile: Profile, cids: &CidMap) -> Builder<'static> {
    let mut b = Builder::empty();
    let mut tags: HashSet<&str> = TAGS.iter().copied().collect();
    let mut schemes: HashSet<&str> = ["http", "https", "mailto"].into_iter().collect();
    let mut tag_attributes: HashMap<&str, HashSet<&str>> = [
        ("a", ["href", "title"].into_iter().collect()),
        ("ol", ["start"].into_iter().collect()),
    ]
    .into_iter()
    .collect();
    if !cids.is_empty() {
        tags.insert("img");
        schemes.insert("cid");
        tag_attributes.insert(
            "img",
            ["src", "alt", "width", "height"].into_iter().collect(),
        );
    }
    let cids = Arc::new(cids.clone());
    b.tags(tags)
        .clean_content_tags(CLEAN_CONTENT_TAGS.iter().copied().collect())
        .generic_attributes(["dir", "style"].into_iter().collect())
        .allowed_classes(
            [
                ("div", CLASSES.iter().copied().collect()),
                ("blockquote", BLOCKQUOTE_CLASSES.iter().copied().collect()),
            ]
            .into_iter()
            .collect(),
        )
        .tag_attributes(tag_attributes)
        .url_schemes(schemes)
        .url_relative(UrlRelative::Deny)
        .link_rel(None)
        .strip_comments(true)
        .attribute_filter(move |element, attribute, value| match attribute {
            "href" => super::link_href_ok(value).then(|| Cow::Owned(value.trim().to_string())),
            // Images: only `cid:` references to the message's own parts,
            // written with the id the map gives them.
            "src" if element == "img" => {
                let v = value.trim();
                let rest = v
                    .get(..4)
                    .filter(|p| p.eq_ignore_ascii_case("cid:"))
                    .map(|_| &v[4..])?;
                let out = cids.get(&crate::normalize_cid(rest))?;
                is_safe_cid(out).then(|| Cow::Owned(format!("cid:{out}")))
            }
            "alt" if element == "img" => Some(Cow::Owned(value.chars().take(200).collect())),
            "width" | "height" if element == "img" => value
                .trim()
                .trim_end_matches("px")
                .parse::<u16>()
                .ok()
                .filter(|n| (1..=4000).contains(n))
                .map(|n| Cow::Owned(n.to_string())),
            // allowed_classes has already filtered it.
            "class" => Some(Cow::Borrowed(value)),
            "dir" => matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "ltr" | "rtl" | "auto"
            )
            .then(|| Cow::Owned(value.trim().to_ascii_lowercase())),
            "start" => (value.trim().len() <= 6 && value.trim().parse::<u32>().is_ok())
                .then(|| Cow::Owned(value.trim().to_string())),
            "style" => {
                let css = filter_style(value, profile, element);
                (!css.is_empty()).then_some(Cow::Owned(css))
            }
            "title" => Some(Cow::Owned(value.chars().take(200).collect())),
            _ => None,
        });
    b
}

/// The crate's CSS sanitizer first (escapes, comments, `expression()`, and
/// every `url()` refused by the resolver), then only this profile's
/// properties, with plain values.
fn filter_style(style: &str, profile: Profile, element: &str) -> String {
    let sanitized = crate::css::sanitize_declarations(style, &|_| None);
    let mut out = Vec::new();
    for decl in sanitized.split(';') {
        let Some((prop, value)) = decl.split_once(':') else {
            continue;
        };
        let prop = prop.trim().to_ascii_lowercase();
        let value = value.trim().trim_end_matches("!important").trim();
        if value.is_empty() || value.len() > 200 || !plain_value(value) {
            continue;
        }
        let v = value.to_ascii_lowercase();
        let ok = match prop.as_str() {
            // What the editor turns into marks (Google Docs pastes use these).
            "font-weight" => {
                matches!(v.as_str(), "normal" | "bold" | "bolder" | "lighter")
                    || (v.len() == 3 && v.ends_with("00") && v.parse::<u16>().is_ok())
            }
            "font-style" => matches!(v.as_str(), "normal" | "italic" | "oblique"),
            "text-decoration" | "text-decoration-line" => v
                .split_ascii_whitespace()
                .all(|t| matches!(t, "none" | "underline" | "line-through" | "overline")),
            // Presentation the composer itself emits for mail clients.
            "font-family" | "font-size" | "line-height" | "margin" | "margin-top"
            | "margin-bottom" | "margin-left" | "margin-right" | "padding" | "padding-left"
            | "padding-right" | "padding-top" | "padding-bottom" | "border-left"
            | "border-radius" | "color" | "background-color" | "white-space" => {
                profile == Profile::Outgoing
            }
            // An inline image's size (the composer's "max-width:100%").
            "max-width" | "width" | "height" => profile == Profile::Outgoing && element == "img",
            _ => false,
        };
        if ok {
            out.push(format!("{prop}:{value}"));
        }
    }
    out.join(";")
}

/// Letters, digits, spaces and `#.,%'"-` only: no parentheses (so no
/// `url()`/`expression()`/`var()`), no backslash escapes, no comments, no
/// `<`, `&` or `;`.
fn plain_value(v: &str) -> bool {
    // Quotes must pair up: an unterminated CSS string would swallow the
    // declarations after it.
    v.matches('"').count().is_multiple_of(2)
        && v.matches('\'').count().is_multiple_of(2)
        && v.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, ' ' | '#' | '.' | ',' | '%' | '\'' | '"' | '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_handlers_and_bad_links_are_removed() {
        let dirty = r#"<p onclick="x()">Hi<script>alert(1)</script><style>p{color:red}</style>
            <a href="javascript:alert(1)">js</a> <a href="https://ok.example/a?b=1" target="_blank" onmouseover="x">ok</a>
            <a href="data:text/html,<script>alert(1)</script>">data</a> <a href="//evil.example">rel</a>
            <a href="mailto:dana@acme.example">mail</a><img src="https://t.example/p.gif" onerror="x">
            <iframe src="https://evil.example"></iframe><svg><script>alert(1)</script></svg></p>"#;
        let clean = sanitize_compose_html(dirty);
        for bad in [
            "script",
            "style",
            "onclick",
            "onerror",
            "onmouseover",
            "javascript",
            "data:",
            "iframe",
            "svg",
            "img",
            "target",
            "evil",
        ] {
            assert!(
                !clean.to_ascii_lowercase().contains(bad),
                "{bad} survived: {clean}"
            );
        }
        assert!(
            clean.contains(r#"<a href="https://ok.example/a?b=1">ok</a>"#),
            "{clean}"
        );
        assert!(
            clean.contains(r#"<a href="mailto:dana@acme.example">mail</a>"#),
            "{clean}"
        );
        assert!(
            clean.contains(">js</a>") || clean.contains("js"),
            "link text stays"
        );
    }

    #[test]
    fn google_docs_paste_keeps_formatting_styles_only() {
        // Trimmed from a real Google Docs clipboard payload (fictional text).
        let docs = r#"<meta charset="utf-8"><b style="font-weight:normal;" id="docs-internal-guid-1a2b"><p dir="ltr" style="line-height:1.38;margin-top:0pt;margin-bottom:0pt;"><span style="font-size:11pt;font-family:Arial,sans-serif;color:#000000;background-color:transparent;font-weight:700;font-style:normal;text-decoration:none;vertical-align:baseline;white-space:pre-wrap;">Agenda</span></p><ul style="margin-top:0;margin-bottom:0;"><li dir="ltr" style="list-style-type:disc;"><p dir="ltr"><span style="font-style:italic;text-decoration:underline;-webkit-text-decoration-skip:none;">Budget</span></p></li></ul></b>"#;
        let clean = sanitize_compose_html(docs);
        assert!(!clean.contains("id="), "{clean}");
        assert!(!clean.contains("meta"), "{clean}");
        assert!(
            !clean.contains("font-family")
                && !clean.contains("font-size")
                && !clean.contains("color"),
            "{clean}"
        );
        assert!(
            clean.contains(r#"<b style="font-weight:normal">"#),
            "the Docs wrapper keeps its not-bold hint: {clean}"
        );
        assert!(clean.contains("font-weight:700"), "{clean}");
        assert!(
            clean.contains("font-style:italic;text-decoration:underline"),
            "{clean}"
        );
        assert!(
            clean.contains("<ul>") && clean.contains("<li dir=\"ltr\">"),
            "{clean}"
        );
    }

    #[test]
    fn css_values_that_could_load_or_run_anything_are_dropped() {
        for style in [
            "font-family:x;background-color:url(https://t.example/a)",
            "color:expression(alert(1))",
            "font-family:\\75rl(x)",
            "font-family:a/**/b",
            "color:red;behavior:url(x.htc)",
            "font-family:x</style><script>",
        ] {
            let out = sanitize_outgoing_html(&format!(r#"<p style="{style}">x</p>"#));
            let l = out.to_ascii_lowercase();
            for bad in ["url", "expression", "\\", "/*", "behavior", "script"] {
                assert!(!l.contains(bad), "{style} → {out}");
            }
        }
    }

    #[test]
    fn outgoing_keeps_composer_presentation() {
        let html = r#"<div dir="auto" style="font-family:'Source Serif 4', Georgia, serif;font-size:15px"><p style="margin:0 0 1em">Hi <strong>Dana</strong></p><blockquote style="margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex"><p>q</p></blockquote><div class="penguin-signature bogus">-- <br>Ana</div><div class="gmail_quote">On …</div></div>"#;
        let out = sanitize_outgoing_html(html);
        assert!(
            out.contains("font-family:'Source Serif 4', Georgia, serif;font-size:15px"),
            "{out}"
        );
        assert!(out.contains("border-left:1px solid #ccc"), "{out}");
        assert!(out.contains(r#"class="penguin-signature""#), "{out}");
        assert!(out.contains(r#"class="gmail_quote""#), "{out}");
        // The editor profile drops presentation but keeps structure.
        let ed = sanitize_compose_html(html);
        assert!(
            !ed.contains("font-family") && !ed.contains("border-left"),
            "{ed}"
        );
        assert!(
            ed.contains("<strong>Dana</strong>") && ed.contains("<blockquote>"),
            "{ed}"
        );
    }

    #[test]
    fn every_style_the_composer_emits_survives() {
        // editor/serialize.ts STYLE, verbatim.
        for style in [
            "margin:0",
            "margin:0.6em 0 0.3em;font-size:1.25em;line-height:1.3",
            "margin:0.25em 0;padding-left:1.6em",
            "margin:0.25em 0 0.25em 0.8ex;border-left:1px solid #ccc;padding-left:1ex;color:#555",
            "font-family:ui-monospace, 'SF Mono', Menlo, Consolas, monospace;font-size:0.9em;background-color:#f2f2f2;padding:0 0.25em;border-radius:3px",
            "font-family:ui-monospace, 'SF Mono', Menlo, Consolas, monospace;font-size:0.9em;background-color:#f6f6f6;padding:0.6em 0.8em;border-radius:4px;white-space:pre-wrap;margin:0.4em 0",
            "margin:0.8em 0",
            "font-family:'Source Serif 4', Georgia, 'Times New Roman', Times, serif;font-size:15px",
        ] {
            let out = sanitize_outgoing_html(&format!(r#"<p style="{style}">x</p>"#));
            let kept = out.split('"').nth(1).unwrap_or_default().replace("&quot;", "\"");
            let props = |s: &str| -> Vec<String> {
                s.split(';').filter_map(|d| d.split_once(':')).map(|(p, _)| p.trim().to_string()).collect()
            };
            assert_eq!(props(&kept), props(style), "{style} → {out}");
        }
    }

    /// One call is a fixpoint: the output re-parses to exactly the tree it
    /// was serialized from, so sanitizing again changes nothing.
    #[test]
    fn one_pass_is_a_fixpoint_over_the_corpus() {
        use crate::xss_tests::{CLASSICS, CLIENT_CVES, MXSS};
        let extra = [
            // ammonia unwraps the table and leaves <a> inside <a>.
            r#"<a href="https://ok.example"><table><a href="javascript:x">x</a></table></a>"#,
            r#"<a href="https://a.example">a<table><tr><td><a href="https://b.example">b</a></td></tr></table>c</a>"#,
            // Blocks that end up inside <p>, via tables, buttons, marquee, object.
            "<p>a<table><tr><td><div>b</div></td></tr></table>c</p>",
            "<p>a<button><p>b</p></button>c</p>",
            "<p>a<marquee><ul><li>b</li></ul></marquee></p>",
            "<p><object><h2>t</h2></object></p>",
            // <li> in <li> and <h2> directly in <h1>.
            "<ul><li>a<table><tr><td><li>b</li></td></tr></table></li></ul>",
            "<h1>a<table><tr><td><h2>b<table><tr><td><h3>c</h3></td></tr></table></h2></td></tr></table></h1>",
            // <pre> with leading newlines (the parser drops the first).
            "<pre>\n\nx</pre>",
            "<pre>\nx</pre>",
            "<div><pre>\n</pre></div>",
            // Entities and non-breaking spaces survive the rebuild.
            "<p>a &amp; b &lt;c&gt; &nbsp;&quot;q&quot;</p>",
            r#"<p style="font-family:&quot;x">u</p>"#,
        ];
        let all = CLASSICS
            .iter()
            .chain(MXSS)
            .chain(CLIENT_CVES)
            .copied()
            .chain(extra);
        for p in all {
            for (name, f) in [
                ("editor", sanitize_compose_html as fn(&str) -> String),
                ("outgoing", sanitize_outgoing_html),
            ] {
                let once = f(p);
                assert_eq!(f(&once), once, "{name} {p:?}");
            }
        }
    }

    #[test]
    fn restructured_nestings_are_unwrapped_keeping_text() {
        let a = sanitize_compose_html(
            r#"<a href="https://ok.example">x<table><a href="https://b.example">y</a></table>z</a>"#,
        );
        // (ammonia's own parse already moved "z" after the outer link.)
        assert_eq!(a, r#"<a href="https://ok.example">xy</a>z"#);
        assert_eq!(
            // marquee doesn't close the <p>, so after unwrapping it a <div> sits
            // inside the <p>; the <p> goes instead (a table would already
            // have closed it during ammonia's own parse).
            sanitize_compose_html("<p>a<marquee><div>b</div></marquee>c</p>"),
            "a<div>b</div>c"
        );
        assert_eq!(
            sanitize_compose_html("<ul><li>a<table><tr><td><li>b</li></td></tr></table></li></ul>"),
            "<ul><li>ab</li></ul>"
        );
        assert_eq!(
            sanitize_compose_html("<h1>a<table><tr><td><h2>b</h2></td></tr></table></h1>"),
            "<h1>ab</h1>"
        );
        // Stable nestings stay: <li> under its own list, <p> in <li>, <h2> in <b> in <h1>.
        let ok = "<ul><li><p>a</p><ul><li>b</li></ul></li></ul><h1><b><h2>c</h2></b></h1><blockquote><p>q</p></blockquote>";
        assert_eq!(sanitize_compose_html(ok), ok);
        assert_eq!(
            sanitize_compose_html("<pre>\n\nx</pre>"),
            "<pre>\n\nx</pre>"
        );
    }

    #[test]
    fn deep_nesting_is_flattened_not_a_stack_overflow() {
        let deep = format!("{}x{}", "<b><i>".repeat(20_000), "</i></b>".repeat(20_000));
        for f in [sanitize_compose_html, sanitize_outgoing_html] {
            let once = f(&deep);
            assert!(once.contains('x'));
            assert!(once.matches("<b>").count() + once.matches("<i>").count() <= MAX_DEPTH);
            assert_eq!(f(&once), once);
        }
    }

    #[test]
    fn unbalanced_quotes_in_style_are_dropped() {
        let out = sanitize_outgoing_html(
            r#"<p style="font-family:'x;color:#555;margin:0">a</p><p style="font-family:&quot;y">b</p>"#,
        );
        // An open quote runs to the end of the attribute, so nothing after it is kept.
        assert_eq!(out, "<p>a</p><p>b</p>");
        assert!(
            !plain_value("'x")
                && !plain_value("\"a\" 'b")
                && plain_value("'Source Serif 4', serif")
        );
    }

    #[test]
    fn quoted_original_keeps_its_formatting_and_nothing_active() {
        let mail = r##"<html><head><style>h1{color:red}</style><title>t</title></head><body>
            <h1 style="color:#1a1a1a;font-size:22px">Scope of work v3</h1>
            <p>Hi team, <b>two</b> changes and a <a href="https://docs.acme.example/sow">link</a>:</p>
            <ul><li><i>re-timed</i> milestones</li><li><u>report</u> attached</li></ul>
            <table width="600" bgcolor="#fff"><tr><td style="padding:4px">Phase</td><td>Weeks</td></tr>
            <tr><td>Discovery</td><td>2</td></tr></table>
            <img src="https://pixel.tracker.example/o.gif" width="1" height="1"><img src="cid:logo@acme.example" alt="logo">
            <script>alert(1)</script><form action="https://evil.example"><input name="x"></form>
            <div class="gmail_quote"><div class="gmail_attr">On Mon, Dana wrote:</div>
            <blockquote class="gmail_quote" style="margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex"><p>earlier</p></blockquote></div>
            </body></html>"##;
        let q = sanitize_quoted_html(mail);
        for keep in [
            r##"<h1 style="color:#1a1a1a;font-size:22px">Scope of work v3</h1>"##,
            "<b>two</b>",
            r##"<a href="https://docs.acme.example/sow">link</a>"##,
            "<li><i>re-timed</i> milestones</li>",
            "<u>report</u>",
            r##"<div style="padding:4px">Phase</div><div>Weeks</div>"##,
            r##"<div class="gmail_attr">On Mon, Dana wrote:</div>"##,
            r##"<blockquote class="gmail_quote" style="margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex">"##,
        ] {
            assert!(q.contains(keep), "missing {keep}\n{q}");
        }
        for gone in [
            "<style",
            "color:red",
            "<title",
            "<img",
            "pixel.tracker",
            "cid:",
            "<script",
            "alert",
            "<form",
            "<input",
            "evil",
            "<table",
            "<td",
            "bgcolor",
            "width",
        ] {
            assert!(!q.contains(gone), "{gone} survived\n{q}");
        }
        assert_eq!(
            sanitize_outgoing_html(&q),
            q,
            "unchanged by the send-time pass"
        );
        assert_eq!(sanitize_quoted_html(&q), q, "a fixpoint");
    }

    #[test]
    fn tables_to_blocks_renames_only_table_tags() {
        assert_eq!(
            tables_to_blocks(
                "<TABLE><TBody><tr><td a=1>x</td><th/></tr></tbody></table><tdx>y</tdx><p>z"
            ),
            "<div><div><div><div a=1>x</div><div/></div></div></div><tdx>y</tdx><p>z"
        );
        assert!(matches!(
            tables_to_blocks("<p>no tables</p>"),
            Cow::Borrowed(_)
        ));
        assert_eq!(tables_to_blocks("a < b <"), "a < b <");
        assert_eq!(tables_to_blocks("<"), "<");
    }

    #[test]
    fn no_images_without_a_cid_map() {
        let html = r#"<p>a<img src="cid:logo@acme.example" alt="logo">b</p>"#;
        for f in [
            sanitize_compose_html as fn(&str) -> String,
            sanitize_outgoing_html,
            sanitize_quoted_html,
        ] {
            assert_eq!(f(html), "<p>ab</p>");
        }
    }

    #[test]
    fn quoted_inline_images_are_rewritten_and_remote_ones_dropped() {
        let mail = r#"<table><tr><td><img src="CID:Logo%40Acme.example" alt="Acme" width="120px" height="40" style="display:block;width:120px;border:0">
            <img src="cid:unknown@acme.example"><img src="https://t.example/p.gif"><img src="data:image/png;base64,AAAA">
            <img src="cid:logo@acme.example" onerror="x()" srcset="https://t.example/a.png 2x"></td></tr></table>
            <a href="cid:logo@acme.example">link</a><div style="width:600px;max-width:100%">w</div>"#;
        let map = cid_map([("<logo@acme.example>", "pg.1a2b@penguin.invalid")]);
        let q = sanitize_quoted_html_with_images(mail, &map);
        assert!(
            q.contains(r#"<img src="cid:pg.1a2b@penguin.invalid" alt="Acme" width="120" height="40" style="width:120px">"#),
            "{q}"
        );
        for gone in [
            "unknown",
            "https://t.example",
            "data:",
            "onerror",
            "srcset",
            "logo@acme",
            "href=\"cid",
            "600px",
            "max-width",
        ] {
            assert!(!q.contains(gone), "{gone} survived\n{q}");
        }
        assert_eq!(q.matches("<img").count(), 2, "{q}");
        assert_eq!(referenced_cids(&q), vec!["pg.1a2b@penguin.invalid"]);
        // The send-time pass keeps it when the part is on the message, and
        // drops it when it isn't.
        let keep = keep_cids(["pg.1a2b@penguin.invalid"]);
        assert_eq!(sanitize_outgoing_html_with_images(&q, &keep), q);
        assert_eq!(sanitize_quoted_html_with_images(&q, &keep), q);
        assert!(!sanitize_outgoing_html(&q).contains("<img"));
    }

    #[test]
    fn unsafe_ids_are_never_written() {
        assert!(is_safe_cid("image001.png@01D9A7B3.C5E8F3A0") && is_safe_cid("ii_m1x2y3"));
        for bad in [
            "",
            "a b",
            "a\"b",
            "a>b",
            "a\r\nX: y",
            "a%20b",
            &"x".repeat(201),
        ] {
            assert!(!is_safe_cid(bad), "{bad:?}");
        }
        let map = cid_map([("logo", "bad id\">")]);
        assert!(map.is_empty());
        let html = r#"<img src="cid:logo">"#;
        assert_eq!(
            sanitize_outgoing_html_with_images(html, &cid_map([("logo", "ok@x")])),
            r#"<img src="cid:ok@x">"#
        );
    }

    #[test]
    fn composer_images_survive_the_editor_and_the_send_pass() {
        // What editor/serialize.ts writes for a pasted image.
        let out = r#"<p style="margin:0">Look:</p><p style="margin:0"><img src="cid:img-00ff.11@penguin.invalid" alt="Pasted image.png" width="600" style="max-width:100%;height:auto"></p>"#;
        let keep = keep_cids(["img-00ff.11@penguin.invalid"]);
        assert_eq!(sanitize_outgoing_html_with_images(out, &keep), out);
        // Reopened into the editor: the image stays, presentation goes.
        assert_eq!(
            sanitize_compose_html_with_images(out, &keep),
            r#"<p>Look:</p><p><img src="cid:img-00ff.11@penguin.invalid" alt="Pasted image.png" width="600"></p>"#
        );
        // A fixpoint in every profile.
        for f in [
            sanitize_compose_html_with_images,
            sanitize_outgoing_html_with_images,
            sanitize_quoted_html_with_images,
        ] {
            let once = f(out, &keep);
            assert_eq!(f(&once, &keep), once);
        }
    }

    #[test]
    fn formatting_round_trips() {
        let html = r#"<p><strong>b</strong> <em>i</em> <u>u</u> <s>s</s> <code>c</code> <a href="https://acme.example/x">l</a></p><h2>H</h2><ul><li><p>one</p></li></ul><ol start="3"><li><p>three</p></li></ol><blockquote><p>q</p></blockquote><pre><code>x &lt; y</code></pre><hr>"#;
        assert_eq!(sanitize_compose_html(html), html);
        assert_eq!(sanitize_outgoing_html(html), html);
    }
}
