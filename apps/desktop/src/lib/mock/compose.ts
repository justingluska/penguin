// Mock of the rich-text composer's backend helper (sanitize_compose_html).
// The real one is penguin-render's strict ammonia allowlist; this mirrors
// its rules closely enough to review paste behavior in the browser.
import type { MockHandler } from "./index";

const TAGS = new Set(["a", "b", "blockquote", "br", "code", "del", "div", "em", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "i", "li", "ol", "p", "pre", "s", "span", "strike", "strong", "u", "ul"]);
const DROP = new Set(["script", "style", "title", "noscript", "template", "iframe", "object", "embed", "svg", "math", "head", "meta", "link", "base", "audio", "video", "canvas", "textarea", "select", "button"]);
const CLASSES = new Set(["penguin-signature", "gmail_signature", "gmail_quote", "gmail_attr"]);
const STYLE_OK: Record<string, RegExp> = {
  "font-weight": /^(normal|bold|bolder|lighter|[1-9]00)$/,
  "font-style": /^(normal|italic|oblique)$/,
  "text-decoration": /^((none|underline|line-through|overline)\s*)+$/,
  "text-decoration-line": /^((none|underline|line-through|overline)\s*)+$/,
};

function linkOk(href: string): boolean {
  try {
    const u = new URL(href.trim());
    return ((u.protocol === "http:" || u.protocol === "https:") && !!u.host) || u.protocol === "mailto:";
  } catch {
    return false;
  }
}

/** `cids`: inline images to keep (`<img src="cid:…">` of the draft's own parts), like the backend's `_with_images` profiles. */
function clean(node: Node, out: Document, cids: Set<string>): Node[] {
  if (node.nodeType === Node.TEXT_NODE) return [out.createTextNode(node.textContent ?? "")];
  if (node.nodeType !== Node.ELEMENT_NODE) return [];
  const el = node as Element;
  const tag = el.tagName.toLowerCase();
  if (DROP.has(tag)) return [];
  if (tag === "img") {
    const cid = /^cid:(.+)$/i.exec(el.getAttribute("src")?.trim() ?? "")?.[1];
    if (!cid || !cids.has(cid)) return [];
    const img = out.createElement("img");
    img.setAttribute("src", `cid:${cid}`);
    const alt = el.getAttribute("alt");
    if (alt) img.setAttribute("alt", alt.slice(0, 200));
    if (/^\d{1,4}$/.test(el.getAttribute("width") ?? "")) img.setAttribute("width", el.getAttribute("width")!);
    return [img];
  }
  const kids = [...el.childNodes].flatMap((c) => clean(c, out, cids));
  if (!TAGS.has(tag)) return kids;
  const next = out.createElement(tag);
  const cls = (el.getAttribute("class") ?? "").split(/\s+/).filter((c) => CLASSES.has(c));
  if (cls.length) next.setAttribute("class", cls.join(" "));
  const dir = el.getAttribute("dir")?.toLowerCase();
  if (dir === "ltr" || dir === "rtl" || dir === "auto") next.setAttribute("dir", dir);
  const style = (el.getAttribute("style") ?? "")
    .split(";")
    .map((d) => d.split(":").map((x) => x.trim().toLowerCase()))
    .filter(([p, v]) => p && v && STYLE_OK[p]?.test(v))
    .map(([p, v]) => `${p}:${v}`)
    .join(";");
  if (style) next.setAttribute("style", style);
  if (tag === "a") {
    const href = el.getAttribute("href");
    if (href && linkOk(href)) next.setAttribute("href", href.trim());
  }
  if (tag === "ol" && /^\d{1,6}$/.test(el.getAttribute("start") ?? "")) next.setAttribute("start", el.getAttribute("start")!);
  kids.forEach((k) => next.appendChild(k));
  return [next];
}

export const composeHandlers: Record<string, MockHandler> = {
  // (`cids` is mock-only: get_draft's image-keeping variant.)
  sanitize_compose_html: ({ html, cids }) => {
    const src = new DOMParser().parseFromString(String(html), "text/html");
    const out = document.implementation.createHTMLDocument("");
    const box = out.createElement("div");
    const keep = new Set((cids as string[] | undefined) ?? []);
    [...src.body.childNodes].flatMap((n) => clean(n, out, keep)).forEach((n) => box.appendChild(n));
    return box.innerHTML;
  },
};
