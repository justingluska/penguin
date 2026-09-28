// OWNER: richtext agent. The composer's document (ProseMirror JSON, see
// schema.ts) → what leaves the app: an email-ready HTML part with minimal
// inline CSS, and a faithful text/plain part. Pure functions, no DOM, so
// node --test covers them.
//
// Paragraphs are lines (margin 0), the way the old plain-text box read: Enter
// starts a new line, an empty paragraph is a blank line.
import type { JSONContent } from "@tiptap/core";
import { isSafeCid } from "../inline.ts";

export type Doc = JSONContent;

/** The conventional signature separator line (RFC 3676 §4.3): dash, dash, space. */
export const SIG_DASH = "-- ";

/** Only these link targets survive (the editor, the sanitizer and here agree). */
export function linkAllowed(href: string | null | undefined): href is string {
  if (!href) return false;
  const h = href.trim();
  if (/^mailto:[^\s]/i.test(h)) return true;
  if (!/^https?:\/\/[^\s/?#]/i.test(h)) return false;
  try {
    return !!new URL(h).host;
  } catch {
    return false;
  }
}

/** Turn what someone typed into a link target: bare domains get https, addresses mailto. */
export function normalizeHref(raw: string): string | null {
  const t = raw.trim();
  if (!t) return null;
  if (linkAllowed(t)) return t;
  if (/^[^\s@<>()]+@[^\s@<>()]+\.[^\s@<>()]+$/.test(t)) return `mailto:${t}`;
  if (/^[a-z0-9][a-z0-9-]*(\.[a-z0-9-]+)+([/?#].*)?$/i.test(t)) return linkAllowed(`https://${t}`) ? `https://${t}` : null;
  return null;
}

export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

const MONO = "ui-monospace, 'SF Mono', Menlo, Consolas, monospace";
export const STYLE = {
  p: "margin:0",
  h1: "margin:0.6em 0 0.3em;font-size:1.5em;line-height:1.25",
  h2: "margin:0.6em 0 0.3em;font-size:1.25em;line-height:1.3",
  h3: "margin:0.6em 0 0.3em;font-size:1.1em;line-height:1.35",
  list: "margin:0.25em 0;padding-left:1.6em",
  quote: "margin:0.25em 0 0.25em 0.8ex;border-left:1px solid #ccc;padding-left:1ex;color:#555",
  code: `font-family:${MONO};font-size:0.9em;background-color:#f2f2f2;padding:0 0.25em;border-radius:3px`,
  pre: `font-family:${MONO};font-size:0.9em;background-color:#f6f6f6;padding:0.6em 0.8em;border-radius:4px;white-space:pre-wrap;margin:0.4em 0`,
  hr: "margin:0.8em 0",
  img: "max-width:100%;height:auto",
} as const;

/** A paragraph that is just a line break (how some HTML parses an empty line) counts as empty. */
function inlineOf(n: Doc): Doc[] {
  const c = n.content ?? [];
  return c.length === 1 && c[0].type === "hardBreak" ? [] : c;
}

function isEmptyParagraph(n: Doc): boolean {
  return n.type === "paragraph" && inlineOf(n).every((c) => c.type === "text" && !(c.text ?? "").trim());
}

/** Top-level blocks without leading or trailing blank lines. */
function trimmedBlocks(doc: Doc): Doc[] {
  const blocks = [...(doc.content ?? [])];
  while (blocks.length && isEmptyParagraph(blocks[blocks.length - 1])) blocks.pop();
  while (blocks.length && isEmptyParagraph(blocks[0])) blocks.shift();
  return blocks;
}

export function isDocEmpty(doc: Doc | null | undefined): boolean {
  return !doc || trimmedBlocks(doc).length === 0;
}

// ---------------------------------------------------------------------------
// HTML

const MARK_ORDER = ["link", "bold", "italic", "underline", "strike", "code"];

function openMark(m: { type: string; attrs?: Record<string, unknown> }): [string, string] | null {
  switch (m.type) {
    case "bold":
      return ["<strong>", "</strong>"];
    case "italic":
      return ["<em>", "</em>"];
    case "underline":
      return ["<u>", "</u>"];
    case "strike":
      return ["<s>", "</s>"];
    case "code":
      return [`<code style="${STYLE.code}">`, "</code>"];
    case "link": {
      const href = m.attrs?.href as string | undefined;
      return linkAllowed(href) ? [`<a href="${escapeHtml(href.trim())}">`, "</a>"] : null;
    }
    default:
      return null;
  }
}

/** An inline image: `cid:` only, capped width, scaled down to fit narrow windows. */
function imageHtml(n: Doc): string {
  const cid = n.attrs?.cid as string | undefined;
  if (!isSafeCid(cid)) return "";
  const width = Number(n.attrs?.width);
  const w = width > 0 ? ` width="${Math.round(width)}"` : "";
  return `<img src="cid:${cid}" alt="${escapeHtml(String(n.attrs?.alt ?? ""))}"${w} style="${STYLE.img}">`;
}

function inlineHtml(nodes: Doc[]): string {
  let out = "";
  for (const n of nodes) {
    if (n.type === "hardBreak") {
      out += "<br>";
      continue;
    }
    if (n.type === "inlineImage") {
      out += imageHtml(n);
      continue;
    }
    if (n.type !== "text" || !n.text) continue;
    const marks = [...(n.marks ?? [])].sort((a, b) => MARK_ORDER.indexOf(a.type) - MARK_ORDER.indexOf(b.type));
    const tags = marks.map(openMark).filter((t): t is [string, string] => !!t);
    out += tags.map((t) => t[0]).join("") + escapeHtml(n.text) + tags.map((t) => t[1]).reverse().join("");
  }
  return out;
}

function listItemHtml(li: Doc): string {
  const kids = li.content ?? [];
  // A one-paragraph item stays inline: <li>text</li> renders the same everywhere.
  if (kids.length === 1 && kids[0].type === "paragraph") return `<li>${inlineHtml(inlineOf(kids[0]))}</li>`;
  return `<li>${blocksHtml(kids)}</li>`;
}

function blockHtml(n: Doc): string {
  switch (n.type) {
    case "paragraph": {
      const inner = inlineHtml(inlineOf(n));
      return `<p style="${STYLE.p}">${inner || "<br>"}</p>`;
    }
    case "heading": {
      const level = Math.min(3, Math.max(1, Number(n.attrs?.level) || 2)) as 1 | 2 | 3;
      return `<h${level} style="${STYLE[`h${level}`]}">${inlineHtml(n.content ?? [])}</h${level}>`;
    }
    case "bulletList":
      return `<ul style="${STYLE.list}">${(n.content ?? []).map(listItemHtml).join("")}</ul>`;
    case "orderedList": {
      const start = Number(n.attrs?.start) || 1;
      return `<ol${start !== 1 ? ` start="${start}"` : ""} style="${STYLE.list}">${(n.content ?? []).map(listItemHtml).join("")}</ol>`;
    }
    case "blockquote":
      return `<blockquote style="${STYLE.quote}">${blocksHtml(n.content ?? [])}</blockquote>`;
    case "codeBlock":
      return `<pre style="${STYLE.pre}"><code>${escapeHtml((n.content ?? []).map((t) => t.text ?? "").join(""))}</code></pre>`;
    case "horizontalRule":
      return `<hr style="${STYLE.hr}">`;
    case "signature":
      return `<div class="penguin-signature">${blocksHtml(n.content ?? [])}</div>`;
    default:
      // Unknown blocks keep their text.
      return n.content ? blocksHtml(n.content) : "";
  }
}

function blocksHtml(nodes: Doc[]): string {
  return nodes.map(blockHtml).join("");
}

/** The body's HTML (no wrapper, no quote): what toDraft wraps with the font stack. */
export function docToEmailHtml(doc: Doc): string {
  return blocksHtml(trimmedBlocks(doc));
}

// ---------------------------------------------------------------------------
// Plain text

function sameText(text: string, href: string): boolean {
  const norm = (s: string) =>
    s
      .trim()
      .toLowerCase()
      .replace(/^mailto:/, "")
      .replace(/^https?:\/\//, "")
      .replace(/\/$/, "");
  return norm(text) === norm(href);
}

function inlineText(nodes: Doc[]): string {
  let out = "";
  let i = 0;
  while (i < nodes.length) {
    const n = nodes[i];
    if (n.type === "hardBreak") {
      out += "\n";
      i++;
      continue;
    }
    if (n.type === "inlineImage") {
      // Gmail's text part says the same.
      out += `[image: ${String(n.attrs?.alt || "image")}]`;
      i++;
      continue;
    }
    const href = n.marks?.find((m) => m.type === "link")?.attrs?.href as string | undefined;
    if (!linkAllowed(href)) {
      out += n.text ?? "";
      i++;
      continue;
    }
    // One link can span several text nodes (bold inside a link): join the run.
    let text = "";
    while (i < nodes.length && nodes[i].marks?.find((m) => m.type === "link")?.attrs?.href === href) {
      text += nodes[i].text ?? "";
      i++;
    }
    out += sameText(text, href) ? text : `${text} (${href.trim().replace(/^mailto:/i, "")})`;
  }
  return out;
}

function prefixLines(lines: string[], first: string, rest: string): string[] {
  return lines.map((l, i) => (i === 0 ? first : rest) + l);
}

function listText(n: Doc): string[] {
  const ordered = n.type === "orderedList";
  const start = Number(n.attrs?.start) || 1;
  const items = n.content ?? [];
  const width = ordered ? `${start + items.length - 1}. `.length : 2;
  return items.flatMap((li, i) => {
    const lines = blocksText(li.content ?? []);
    const bullet = ordered ? `${start + i}. `.padEnd(width) : "- ";
    return prefixLines(lines.length ? lines : [""], bullet, " ".repeat(width));
  });
}

function blockText(n: Doc): string[] {
  switch (n.type) {
    case "paragraph":
      return inlineText(inlineOf(n)).split("\n");
    case "heading":
      return inlineText(n.content ?? []).split("\n");
    case "bulletList":
    case "orderedList":
      return listText(n);
    case "blockquote":
      return blocksText(n.content ?? []).map((l) => (l ? `> ${l}` : ">"));
    case "codeBlock":
      return (n.content ?? []).map((t) => t.text ?? "").join("").split("\n");
    case "horizontalRule":
      return ["----------"];
    case "signature": {
      // HTML parsing drops the separator's trailing space; the text part keeps "-- ".
      const lines = blocksText(n.content ?? []);
      if (lines[0] === "--") lines[0] = SIG_DASH;
      return lines;
    }
    default:
      return n.content ? blocksText(n.content) : [];
  }
}

function blocksText(nodes: Doc[]): string[] {
  return nodes.flatMap(blockText);
}

/** The text/plain part: lists as "- " / "1. ", quotes as "> ", links as "text (url)". */
export function docToText(doc: Doc): string {
  return (
    blocksText(trimmedBlocks(doc))
      // Trailing spaces go, except the "-- " signature separator's.
      .map((l) => (l === SIG_DASH ? l : l.replace(/[ \t]+$/, "")))
      .join("\n")
      .replace(/\s+$/, "")
  );
}

// ---------------------------------------------------------------------------
// Text → doc (older drafts, plain snippets)

/** Plain text as one paragraph per line. */
export function textToDoc(text: string): Doc {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  return {
    type: "doc",
    content: lines.map((l) => (l ? { type: "paragraph", content: [{ type: "text", text: l }] } : { type: "paragraph" })),
  };
}

/** The Content-IDs of the inline images in a document, in order. */
export function docCids(doc: Doc | null | undefined): string[] {
  const out: string[] = [];
  const walk = (n: Doc) => {
    const cid = n.attrs?.cid as string | undefined;
    if (n.type === "inlineImage" && isSafeCid(cid) && !out.includes(cid)) out.push(cid);
    n.content?.forEach(walk);
  };
  if (doc) walk(doc);
  return out;
}
