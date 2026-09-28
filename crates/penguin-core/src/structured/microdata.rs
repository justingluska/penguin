//! schema.org microdata → the JSON-LD shape, so `schema_org::map_items`
//! reads both. A streaming tag scanner, not a DOM: email HTML is often
//! malformed, and only elements with `itemscope`/`itemprop` matter.
//!
//! Property values follow the WHATWG HTML microdata rules: `meta` →
//! `content`; `a`, `area`, `link` → `href`; `img`, `audio`, `video`,
//! `source`, `track`, `iframe`, `embed` → `src`; `object` → `data`;
//! `data`, `meter` → `value`; `time` → `datetime` (else its text);
//! otherwise the element's text. An element with `itemscope` and
//! `itemprop` is a nested item. `itemtype` becomes `@type`.

use serde_json::{Map, Value};

use super::Extracted;

/// Elements that never have content (no end tag to wait for).
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

struct Item {
    types: Vec<String>,
    props: Vec<(String, Value)>,
    /// (parent item, property names) when nested.
    parent: Option<(usize, Vec<String>)>,
}

struct Open {
    name: String,
    /// The item this element opened.
    item: Option<usize>,
    /// A text-valued property waiting for the end tag: (item, names, text start).
    pending: Option<(usize, Vec<String>, usize)>,
}

pub(crate) fn from_microdata(html: &str) -> Vec<Extracted> {
    if memchr::memmem::find(html.as_bytes(), b"itemscope").is_none() {
        return Vec::new();
    }
    super::schema_org::map_items(&items(html))
}

/// Parse `html` into top-level items as JSON values.
pub(crate) fn items(html: &str) -> Vec<Value> {
    let b = html.as_bytes();
    let mut arena: Vec<Item> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut i = 0;
    while let Some(off) = memchr::memchr(b'<', &b[i..]) {
        let lt = i + off;
        if b[lt..].starts_with(b"<!--") {
            i = memchr::memmem::find(&b[lt + 4..], b"-->").map_or(b.len(), |p| lt + 4 + p + 3);
            continue;
        }
        let Some(gt_off) = tag_end(&b[lt..]) else {
            break;
        };
        let gt = lt + gt_off;
        let inner = &html[lt + 1..gt];
        i = gt + 1;
        if let Some(close) = inner.strip_prefix('/') {
            let name = close.trim().to_ascii_lowercase();
            if let Some(pos) = stack.iter().rposition(|o| o.name == name) {
                while stack.len() > pos {
                    let o = stack.pop().expect("non-empty");
                    if let Some((item, names, start)) = o.pending {
                        // Elements closed implicitly end here too.
                        let text = crate::text::html_to_text(&html[start..lt]);
                        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                        for n in names {
                            arena[item].props.push((n, Value::String(text.clone())));
                        }
                    }
                }
            }
            continue;
        }
        if inner.starts_with('!') || inner.starts_with('?') {
            continue;
        }
        let (name, attrs) = parse_tag(inner);
        if name.is_empty() {
            continue;
        }
        // Skip script/style content entirely.
        if name == "script" || name == "style" {
            let close = format!("</{name}");
            let lower_rest = html[i..].to_ascii_lowercase();
            i = lower_rest.find(&close).map_or(b.len(), |p| i + p);
            continue;
        }
        let attr = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let scope = attr("itemscope").is_some();
        let props: Vec<String> = attr("itemprop")
            .map(|p| p.split_whitespace().map(String::from).collect())
            .unwrap_or_default();
        let parent = stack.iter().rev().find_map(|o| o.item);
        let mut open = Open {
            name: name.clone(),
            item: None,
            pending: None,
        };
        if scope {
            let types = attr("itemtype")
                .map(|t| t.split_whitespace().map(String::from).collect())
                .unwrap_or_default();
            arena.push(Item {
                types,
                props: Vec::new(),
                parent: match (parent, props.is_empty()) {
                    (Some(p), false) => Some((p, props.clone())),
                    _ => None,
                },
            });
            open.item = Some(arena.len() - 1);
        } else if let (Some(p), false) = (parent, props.is_empty()) {
            let value = match name.as_str() {
                "meta" => attr("content").map(String::from),
                "a" | "area" | "link" => attr("href").map(String::from),
                "img" | "audio" | "video" | "source" | "track" | "iframe" | "embed" => {
                    attr("src").map(String::from)
                }
                "object" => attr("data").map(String::from),
                "data" | "meter" => attr("value").map(String::from),
                "time" => attr("datetime").map(String::from),
                _ => None,
            }
            // Email senders often put `content` on any element.
            .or_else(|| attr("content").map(String::from));
            match value {
                Some(v) => {
                    for n in &props {
                        arena[p].props.push((n.clone(), Value::String(decode(&v))));
                    }
                }
                None if !VOID.contains(&name.as_str()) => {
                    open.pending = Some((p, props.clone(), i));
                }
                None => {}
            }
        }
        let self_closing = inner.trim_end().ends_with('/');
        // A void element can still be an (empty) item; it just never opens.
        if !VOID.contains(&name.as_str()) && !self_closing {
            stack.push(open);
        }
    }
    // Build the trees bottom-up: children first.
    let mut built: Vec<Option<Value>> = vec![None; arena.len()];
    for idx in (0..arena.len()).rev() {
        let it = &arena[idx];
        let mut m = Map::new();
        if let Some(t) = it.types.first() {
            m.insert("@type".into(), Value::String(t.clone()));
        }
        for (k, v) in &it.props {
            add(&mut m, k, v.clone());
        }
        // Children were built already (they come later in document order).
        for (cidx, child) in arena.iter().enumerate().skip(idx + 1) {
            if let Some((p, names)) = &child.parent {
                if *p == idx {
                    if let Some(v) = built[cidx].clone() {
                        for n in names {
                            add(&mut m, n, v.clone());
                        }
                    }
                }
            }
        }
        built[idx] = Some(Value::Object(m));
    }
    arena
        .iter()
        .enumerate()
        .filter(|(_, it)| it.parent.is_none())
        .filter_map(|(i, _)| built[i].take())
        .collect()
}

fn add(m: &mut Map<String, Value>, k: &str, v: Value) {
    match m.get_mut(k) {
        None => {
            m.insert(k.to_string(), v);
        }
        Some(Value::Array(a)) => a.push(v),
        Some(existing) => {
            let old = existing.take();
            *existing = Value::Array(vec![old, v]);
        }
    }
}

/// Offset of the `>` closing the tag at the start of `b`, honoring quotes.
fn tag_end(b: &[u8]) -> Option<usize> {
    let mut q: Option<u8> = None;
    for (i, &c) in b.iter().enumerate().skip(1) {
        match q {
            Some(x) if c == x => q = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => q = Some(c),
            None if c == b'>' => return Some(i),
            None => {}
        }
    }
    None
}

/// Tag name (lowercase) and attributes (lowercase names, raw values).
fn parse_tag(inner: &str) -> (String, Vec<(String, String)>) {
    let s = inner.trim_end_matches('/');
    let name_end = s
        .find(|c: char| c.is_whitespace() || c == '/')
        .unwrap_or(s.len());
    let name = s[..name_end].to_ascii_lowercase();
    let mut attrs = Vec::new();
    let b = s.as_bytes();
    let mut i = name_end;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let ns = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        if ns == i {
            break;
        }
        let an = s[ns..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                let vs = i + 1;
                let ve = s[vs..].find(q as char).map_or(s.len(), |p| vs + p);
                val = s[vs..ve].to_string();
                i = (ve + 1).min(b.len());
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                val = s[vs..i].to_string();
            }
        }
        attrs.push((an, val));
    }
    (name, attrs)
}

fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        match crate::text::decode_entity(&rest[p..]) {
            Some((txt, n)) => {
                out.push_str(&txt);
                rest = &rest[p + n..];
            }
            None => {
                out.push('&');
                rest = &rest[p + 1..];
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_items_and_value_rules() {
        let html = r#"<div itemscope itemtype="http://schema.org/ParcelDelivery">
          <div itemprop="carrier" itemscope itemtype="http://schema.org/Organization">
            <meta itemprop="name" content="UPS"/>
          </div>
          <span itemprop="trackingNumber">1Z5R89390357567127</span>
          <link itemprop="trackingUrl" href="https://www.ups.com/track?tracknum=1Z5R89390357567127"/>
          <time itemprop="expectedArrivalUntil" datetime="2026-10-02T20:00:00-07:00">Friday</time>
          <div itemprop="partOfOrder" itemscope itemtype="http://schema.org/Order">
            <b itemprop="orderNumber">A-1042</b>
            <div itemprop="merchant" itemscope itemtype="http://schema.org/Organization"><span itemprop="name">Acme &amp; Co</span></div>
          </div>
        </div>"#;
        let v = items(html);
        assert_eq!(v.len(), 1);
        let d = &v[0];
        assert_eq!(d["@type"], "http://schema.org/ParcelDelivery");
        assert_eq!(d["carrier"]["name"], "UPS");
        assert_eq!(d["trackingNumber"], "1Z5R89390357567127");
        assert_eq!(d["expectedArrivalUntil"], "2026-10-02T20:00:00-07:00");
        assert_eq!(d["partOfOrder"]["orderNumber"], "A-1042");
        assert_eq!(d["partOfOrder"]["merchant"]["name"], "Acme & Co");
    }
}
