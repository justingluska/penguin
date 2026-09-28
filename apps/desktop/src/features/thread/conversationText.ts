// A conversation as clean plain text, for pasting into an AI chat, a note or
// a ticket (the thread toolbar's Copy, ⇧C, the ⌘K palette, the message menu).
// Pure (no api, no DOM, no store) so tests/conversationText.test.ts runs it
// directly; features/thread/copy.ts does the clipboard part.
//
//   Subject: Q4 brand refresh
//
//   === Message 1 of 2 ===
//   From: Priya Raman <priya@linden.example>
//   To: Sam Okafor <sam@northwind.example>
//   Date: Tue, Sep 22, 2026, 9:41 AM EDT
//
//   body…
//
//   Attachments: Deck_v7.pdf, Lockups.png
//
// Each body is the text/plain part when it's usable, else the sanitized HTML
// read as text (paragraphs, lists, "link text (url)"). Reply history that
// repeats an earlier message of the same thread is cut, so every message is
// in the copy once; history the thread doesn't have (a forward, a quote of
// mail from before this thread) stays.
import type { Address, AttachmentMeta, MessageView } from "../../lib/types";

/** The parts of a message the copy reads (a MessageView is one). */
export type CopyMessage = Pick<MessageView, "id" | "date" | "from" | "to" | "cc" | "subject" | "snippet" | "bodyText" | "html" | "labelIds"> & {
  attachments: Pick<AttachmentMeta, "filename" | "inline">[];
  bodyPending?: boolean;
};

export interface CopyThread {
  subject: string;
  /** Oldest first, as get_thread returns them. */
  messages: CopyMessage[];
}

export interface CopyOptions {
  /** How the Date: line reads; defaults to the local date and time with the zone. */
  formatDate?: (ms: number) => string;
}

// ---------------------------------------------------------------------------
// The whole conversation, or one message of it
// ---------------------------------------------------------------------------

/** Every message, oldest first, each once. */
export function conversationText(thread: CopyThread, opts: CopyOptions = {}): string {
  const msgs = [...thread.messages].sort((a, b) => a.date - b.date);
  const bodies = messageBodies(msgs);
  const parts = [`Subject: ${subjectOf(thread, msgs)}`];
  msgs.forEach((m, i) => {
    const head = msgs.length > 1 ? `=== Message ${i + 1} of ${msgs.length} ===\n` : "";
    parts.push(head + messageBlock(m, bodies[i], opts));
  });
  return parts.join("\n\n") + "\n";
}

/**
 * One message with its headers. Reply history that repeats earlier messages
 * of the thread is cut here too: the message's own words are what's wanted.
 */
export function messageText(thread: CopyThread, messageId: string, opts: CopyOptions = {}): string {
  const msgs = [...thread.messages].sort((a, b) => a.date - b.date);
  const i = msgs.findIndex((m) => m.id === messageId);
  if (i < 0) return "";
  const bodies = messageBodies(msgs.slice(0, i + 1));
  const subject = msgs[i].subject.trim() || subjectOf(thread, msgs);
  return `Subject: ${subject}\n\n${messageBlock(msgs[i], bodies[i], opts)}\n`;
}

function subjectOf(thread: CopyThread, msgs: CopyMessage[]): string {
  return thread.subject.trim() || msgs.find((m) => m.subject.trim())?.subject.trim() || "(no subject)";
}

function messageBlock(m: CopyMessage, body: string, opts: CopyOptions): string {
  const date = (opts.formatDate ?? readableDate)(m.date);
  const lines = [`From: ${addressText(m.from)}`];
  if (m.to.length) lines.push(`To: ${m.to.map(addressText).join(", ")}`);
  if (m.cc.length) lines.push(`Cc: ${m.cc.map(addressText).join(", ")}`);
  lines.push(`Date: ${date}`);
  if (m.labelIds.includes("DRAFT")) lines.push("Draft: not sent yet");
  const out = [lines.join("\n")];
  out.push(body || "(no text)");
  const files = m.attachments.filter((a) => !a.inline).map((a) => a.filename.trim() || "(unnamed file)");
  if (files.length) out.push(`Attachments: ${files.join(", ")}`);
  return out.join("\n\n");
}

/** "Name <email>", or the bare address when there's no name worth showing. */
export function addressText(a: Address): string {
  const name = (a.name ?? "").trim().replace(/^"(.*)"$/, "$1");
  if (!name || name.toLowerCase() === a.email.toLowerCase()) return a.email;
  const safe = /[,;<>"]/.test(name) ? `"${name.replace(/"/g, "'")}"` : name;
  return `${safe} <${a.email}>`;
}

const DATE_FORMAT: Intl.DateTimeFormatOptions = {
  weekday: "short",
  year: "numeric",
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
  timeZoneName: "short",
};

/** "Tue, Sep 22, 2026, 9:41 AM EDT" in the reader's locale and zone. */
export function readableDate(ms: number): string {
  return new Date(ms).toLocaleString(undefined, DATE_FORMAT);
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/**
 * Each message's text with the reply history cut where it repeats an
 * earlier message of the thread (msgs oldest first).
 */
export function messageBodies(msgs: CopyMessage[]): string[] {
  const probes: string[] = [];
  return msgs.map((m) => {
    const b = bodyParts(m);
    const quoted = squash(b.quoted);
    const repeats = quoted.length > 0 && probes.some((p) => quoted.includes(p));
    const text = repeats ? b.authored : b.full;
    const probe = squash(b.authored || b.full).slice(0, PROBE_LEN);
    if (probe.length >= MIN_PROBE) probes.push(probe);
    return text;
  });
}

/** How much of an earlier message's start must show up in a quote to count as a repeat. */
const PROBE_LEN = 60;
const MIN_PROBE = 8;

/** Letters and digits only, lowercased: quotes re-wrap, re-indent and drop formatting. */
function squash(s: string): string {
  return s.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, "");
}

interface BodyParts {
  /** Everything, in order. */
  full: string;
  /** Without the reply history. */
  authored: string;
  /** The reply history alone (for the repeat check). */
  quoted: string;
}

export function bodyParts(m: CopyMessage): BodyParts {
  if (m.bodyPending) {
    const s = m.snippet.trim();
    const text = s ? `${s} …\n\n(Only the start of this message has downloaded so far.)` : "(This message hasn't downloaded yet.)";
    return { full: text, authored: text, quoted: "" };
  }
  const plain = tidy(m.bodyText);
  const rich = m.html && !isTextDocument(m.html) ? htmlToText(m.html) : null;
  if (rich && !plainUsable(plain, rich)) {
    const cut = splitQuoted(rich.authored);
    return { full: rich.full, authored: cut.authored, quoted: [rich.quoted, cut.quoted].filter(Boolean).join("\n") };
  }
  const cut = splitQuoted(plain);
  return { full: plain, authored: cut.authored, quoted: cut.quoted };
}

/** penguin-render's document for a plain-text message (the HTML adds nothing). */
function isTextDocument(html: string): boolean {
  return /^\s*<!doctype html>\s*<html class="pg-text"/i.test(html);
}

const HTML_ONLY_NOTE =
  /(view|read|open|see) (this|the|our) (e-?mail|message|newsletter|issue)[^\n]{0,40}(browser|online|web)|(does ?n[o']t|cannot|can't) (support|display|show) html|html (e-?mail|message|version)|enable html/i;

/**
 * Whether the text/plain part is the message, or a stand-in for the HTML:
 * empty, a "view this email in your browser" note, a sliver of the HTML's
 * text, or the backend's own reading of the HTML (a message without a plain
 * part stores its HTML as text, links dropped; the HTML reading keeps them).
 */
export function plainUsable(plain: string, rich: Pick<HtmlText, "plain" | "webLinks">): boolean {
  const p = squash(plain);
  if (!p) return false;
  const h = squash(rich.plain);
  if (!h) return true;
  if (p === h) return false;
  // A real text/plain part spells its links out ("report <https://…>"); the
  // backend's reading of an HTML-only message has none.
  if (rich.webLinks > 0 && !/https?:\/\//i.test(plain)) return false;
  if (plain.length < 600 && HTML_ONLY_NOTE.test(plain) && h.length > p.length) return false;
  if (h.length > 200 && p.length < h.length * 0.4) return false;
  return true;
}

// ---------------------------------------------------------------------------
// Reply history in plain text (mirrors penguin-core text::split_quoted)
// ---------------------------------------------------------------------------

const WROTE_SUFFIXES = [
  "wrote:",
  "a écrit :",
  "a écrit:",
  "schrieb:",
  "escribió:",
  "ha scritto:",
  "schreef:",
  "napisał:",
  "skrev:",
  "kirjoitti:",
  "escreveu:",
];

/**
 * Split a plain body into (authored, quoted): quoted is ">" lines plus
 * everything from the first reply attribution ("On … wrote:", "-----Original
 * Message-----", an Outlook header block). A forward's payload is authored.
 */
export function splitQuoted(text: string): { authored: string; quoted: string } {
  const lines = text.split("\n");
  const cut = quoteCut(lines);
  const authored: string[] = [];
  const quoted: string[] = [];
  lines.forEach((line, i) => {
    if (i >= cut) quoted.push(line.replace(/^[>\s]+/, ""));
    else if (/^\s*>/.test(line)) quoted.push(line.replace(/^[>\s]+/, ""));
    else authored.push(line);
  });
  return { authored: tidy(authored.join("\n")), quoted: tidy(quoted.join("\n")) };
}

function quoteCut(lines: string[]): number {
  for (let i = 0; i < lines.length; i++) {
    const t = lines[i].trim();
    if (!t) continue;
    const lower = t.toLowerCase();
    // "On Tue, Mar 3, 2026 at 10:00 AM Mike <mike@x> wrote:", possibly wrapped over three lines.
    if (/^(on|le|am|el|il|op) /.test(lower)) {
      for (const line of lines.slice(i, i + 3)) {
        const l = line.trim().toLowerCase();
        if (WROTE_SUFFIXES.some((s) => l.endsWith(s)) || (l.includes(" schrieb ") && l.endsWith(":"))) return i;
      }
    }
    if (WROTE_SUFFIXES.some((s) => lower.endsWith(s)) && lower.includes("@")) return i;
    if (/^-{2,}\s*original message\s*-{2,}$/.test(lower)) return i;
    // Outlook: a rule of underscores followed by a From: header block.
    if (t.length >= 10 && /^_+$/.test(t)) {
      if (lines.slice(i + 1, i + 4).some((l) => /^from:/i.test(l.trim()))) return i;
      continue;
    }
    // Outlook/Apple header block without a rule: From: + (Sent:|Date:) + (To:|Subject:).
    if (/^from:/i.test(t) && !forwardContext(lines, i)) {
      const next = lines.slice(i + 1, i + 6).map((l) => l.trim());
      if (next.some((l) => /^(sent|date):/i.test(l)) && next.some((l) => /^(to|subject):/i.test(l))) return i;
    }
  }
  return lines.length;
}

function forwardContext(lines: string[], i: number): boolean {
  for (let j = i - 1; j >= 0; j--) {
    const l = lines[j].trim().toLowerCase();
    if (!l) continue;
    return l.includes("forwarded message") || l.startsWith("begin forwarded");
  }
  return false;
}

// ---------------------------------------------------------------------------
// Sanitized HTML → text
// ---------------------------------------------------------------------------

export interface HtmlText {
  /** Everything, with links as "text (url)". */
  full: string;
  /** Without the marked reply history (gmail_quote, Yahoo, Thunderbird, Outlook). */
  authored: string;
  /** The marked reply history alone. */
  quoted: string;
  /** Everything without link targets: compared with text/plain in plainUsable. */
  plain: string;
  /** Links to web pages that got a "(url)" after their text. */
  webLinks: number;
}

type Chunk =
  | { k: "text"; s: string; q: boolean; link?: boolean; keep?: boolean; lead?: boolean }
  | { k: "break"; n: number; q: boolean }
  | { k: "br"; q: boolean };

const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"]);
const SKIP = new Set(["head", "style", "script", "title", "textarea", "template", "noscript", "svg", "xml", "select", "button", "object", "iframe"]);
const RAW = new Set(["style", "script", "title", "textarea", "xml"]);
const PARAGRAPH = new Set(["p", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "pre", "table", "ul", "ol", "dl", "figure", "hr"]);
const BLOCK = new Set([
  "div", "tr", "li", "section", "article", "header", "footer", "main", "nav", "aside", "center", "dt", "dd",
  "tbody", "thead", "tfoot", "form", "address", "details", "summary", "caption", "body", "fieldset", "legend",
]);
/** Marked reply history: Gmail (ours included), Yahoo, Thunderbird, Apple's attribution, penguin-render's folded quote. */
const QUOTE_CLASSES = ["gmail_quote", "gmail_attr", "gmail_extra_quote", "yahoo_quoted", "moz-cite-prefix", "pg-quote"];
/** Outlook: everything after these is the history. */
const QUOTE_REST_IDS = ["appendonsend", "divrplyfwdmsg"];

interface Frame {
  name: string;
  skip: boolean;
  quote: boolean;
  pre: boolean;
  list?: { ordered: boolean; n: number };
  link?: { href: string; from: number };
}

export function htmlToText(html: string): HtmlText {
  const chunks: Chunk[] = [];
  const stack: Frame[] = [];
  let restQuoted = false;
  let webLinks = 0;
  const top = () => stack[stack.length - 1];
  const skipping = () => stack.some((f) => f.skip);
  const quoted = () => restQuoted || stack.some((f) => f.quote);
  const pre = () => stack.some((f) => f.pre);
  const push = (c: Chunk) => {
    if (!skipping()) chunks.push(c);
  };
  const listDepth = () => stack.filter((f) => f.list).length;

  const close = (name: string) => {
    const at = stack.map((f) => f.name).lastIndexOf(name);
    if (at < 0) return;
    while (stack.length > at) {
      const f = stack.pop()!;
      endElement(f);
    }
  };
  const endElement = (f: Frame) => {
    if (f.link) linkSuffix(f.link);
    if (PARAGRAPH.has(f.name)) push({ k: "break", n: 2, q: quoted() });
    else if (BLOCK.has(f.name)) push({ k: "break", n: 1, q: quoted() });
  };
  const linkSuffix = (link: { href: string; from: number }) => {
    const label = chunks
      .slice(link.from)
      .filter((c): c is Extract<Chunk, { k: "text" }> => c.k === "text")
      .map((c) => c.s)
      .join("")
      .replace(/\s+/g, " ")
      .trim();
    const target = linkTarget(link.href, label);
    if (!target) return;
    if (/^https?:/i.test(target)) webLinks++;
    push({ k: "text", s: ` (${target})`, q: quoted(), link: true });
  };

  let i = 0;
  const len = html.length;
  while (i < len) {
    const lt = html.indexOf("<", i);
    const end = lt < 0 ? len : lt;
    if (end > i) push({ k: "text", s: decodeEntities(html.slice(i, end)), q: quoted(), keep: pre() });
    if (lt < 0) break;
    i = lt;
    if (html.startsWith("<!--", i)) {
      const e = html.indexOf("-->", i + 4);
      i = e < 0 ? len : e + 3;
      continue;
    }
    const m = /^<(\/?)([a-zA-Z][a-zA-Z0-9:-]*)/.exec(html.slice(i, i + 64));
    if (!m) {
      if (html[i + 1] === "!" || html[i + 1] === "?") {
        i = tagEnd(html, i + 2);
        continue;
      }
      push({ k: "text", s: "<", q: quoted(), keep: pre() });
      i += 1;
      continue;
    }
    const closing = m[1] === "/";
    const name = m[2].toLowerCase();
    const after = tagEnd(html, i + m[0].length);
    const attrText = html.slice(i + m[0].length, Math.max(i + m[0].length, after - 1));
    i = after;
    if (closing) {
      close(name);
      continue;
    }
    const attrs = parseAttrs(attrText);
    // Raw text elements: jump to their close tag (a "<" inside is not a tag).
    if (RAW.has(name)) {
      const e = html.toLowerCase().indexOf(`</${name}`, i);
      i = e < 0 ? len : tagEnd(html, e + 2);
      if (!SKIP.has(name)) push({ k: "break", n: 1, q: quoted() });
      continue;
    }
    const cls = ` ${(attrs.class ?? "").toLowerCase()} `;
    const id = (attrs.id ?? "").toLowerCase();
    if (QUOTE_REST_IDS.includes(id)) restQuoted = true;
    if (name === "br") {
      push({ k: "br", q: quoted() });
      continue;
    }
    if (name === "img") continue;
    if (name === "hr") {
      push({ k: "break", n: 2, q: quoted() });
      continue;
    }
    if (VOID.has(name)) continue;

    const parent = top();
    const frame: Frame = {
      name,
      skip: SKIP.has(name) || hidden(attrs.style) || cls.includes(" pg-simplified ") || (name === "summary" && !!parent?.quote),
      quote: QUOTE_CLASSES.some((c) => cls.includes(` ${c} `)),
      pre: name === "pre",
    };
    // An unclosed <p> or <li> ends at the next one.
    if ((name === "p" || name === "li") && parent?.name === name) close(name);
    if (PARAGRAPH.has(name)) push({ k: "break", n: 2, q: quoted() || frame.quote });
    else if (BLOCK.has(name)) push({ k: "break", n: 1, q: quoted() || frame.quote });
    else if ((name === "td" || name === "th") && !frame.skip) push({ k: "text", s: " ", q: quoted() });
    if (name === "ul" || name === "ol") frame.list = { ordered: name === "ol", n: Number(attrs.start) || 1 };
    stack.push(frame);
    if (name === "li") {
      const list = [...stack].reverse().find((f) => f.list)?.list;
      const bullet = list?.ordered ? `${list.n++}. ` : "- ";
      push({ k: "text", s: "  ".repeat(Math.max(0, listDepth() - 1)) + bullet, q: quoted(), keep: true, lead: true });
    }
    if (name === "a" && attrs.href) frame.link = { href: attrs.href, from: chunks.length };
  }
  while (stack.length) endElement(stack.pop()!);

  const render = (keep: (c: Chunk) => boolean) => tidy(renderChunks(chunks.filter(keep)));
  return {
    full: render(() => true),
    authored: render((c) => !c.q),
    quoted: render((c) => c.q),
    plain: render((c) => !(c.k === "text" && c.link)),
    webLinks,
  };
}

/** Index just past the ">" that ends the tag starting before `from`, honoring quoted values. */
function tagEnd(html: string, from: number): number {
  let q = "";
  for (let i = from; i < html.length; i++) {
    const c = html[i];
    if (q) {
      if (c === q) q = "";
    } else if (c === '"' || c === "'") q = c;
    else if (c === ">") return i + 1;
  }
  return html.length;
}

function parseAttrs(s: string): Record<string, string> {
  const out: Record<string, string> = {};
  const re = /([^\s=/>"']+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>"']+)))?/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s))) {
    const k = m[1].toLowerCase();
    if (!(k in out)) out[k] = decodeEntities(m[2] ?? m[3] ?? m[4] ?? "");
  }
  return out;
}

/** Hidden by its style: preheaders, mobile-only copies, tracking filler. */
function hidden(style: string | undefined): boolean {
  if (!style) return false;
  const s = style.toLowerCase().replace(/\s+/g, "");
  if (/(^|;)display:none/.test(s) || /(^|;)visibility:hidden/.test(s) || /mso-hide:all/.test(s) || /(^|;)opacity:0(;|$|!)/.test(s)) return true;
  return /(^|;)(max-)?height:0(px)?(;|$|!)/.test(s) && /overflow:hidden/.test(s);
}

/** What a link adds after its text: its web address or email address, unless the text already says it. */
function linkTarget(href: string, label: string): string | null {
  const h = href.trim();
  if (/^mailto:/i.test(h)) {
    const addr = safeDecode(h.slice(7).split("?")[0]);
    return addr && !label.toLowerCase().includes(addr.toLowerCase()) ? addr : null;
  }
  if (!/^https?:\/\//i.test(h) || !label) return null;
  const url = cleanUrl(h);
  const bare = (u: string) => u.toLowerCase().replace(/^https?:\/\//, "").replace(/^www\./, "").replace(/\/$/, "");
  if (bare(label) === bare(url) || bare(label) === bare(h)) return null;
  return url;
}

/** Drop campaign tags (utm_*, Mailchimp and ad click ids): noise to a reader. */
function cleanUrl(href: string): string {
  let u: URL;
  try {
    u = new URL(href);
  } catch {
    // Not a URL the parser accepts (a template placeholder, bad escaping): show it as written.
    return href;
  }
  const drop = [...u.searchParams.keys()].filter((k) => /^(utm_|mc_(cid|eid)$|fbclid$|gclid$|_hsenc$|_hsmi$|mkt_tok$)/i.test(k));
  if (drop.length === 0) return href;
  drop.forEach((k) => u.searchParams.delete(k));
  return u.toString().replace(/\?$/, "");
}

function safeDecode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    // A stray "%" that isn't an escape: the raw text is the address.
    return s;
  }
}

function renderChunks(chunks: Chunk[]): string {
  let out = "";
  let breaks = 0; // newlines owed before the next text
  let space = false;
  let lead = false; // just wrote a list bullet: the item's first block stays on its line
  for (const c of chunks) {
    if (lead && c.k !== "text") continue;
    if (c.k === "break") {
      breaks = Math.max(breaks, c.n);
      space = false;
      continue;
    }
    if (c.k === "br") {
      breaks = Math.min(breaks + 1, 2);
      space = false;
      continue;
    }
    let s = c.s;
    if (!c.keep) {
      s = s.replace(/[\s ]+/g, " ");
      if (s.startsWith(" ")) space = true;
      s = s.trim();
      if (!s) continue;
    }
    if (out && breaks) out += "\n".repeat(breaks);
    else if (out && space && !out.endsWith("\n") && !out.endsWith(" ")) out += " ";
    breaks = 0;
    out += s;
    lead = !!c.lead;
    space = !c.keep && /[\s ]$/.test(c.s);
  }
  return out;
}

/** Trim line ends, drop invisible filler, and keep at most one blank line in a row. */
export function tidy(s: string): string {
  return s
    .replace(/\r\n?/g, "\n")
    .replace(/[​-‍⁠﻿­͏᠎]/g, "")
    .replace(/[   ]/g, " ")
    .split("\n")
    .map((l) => l.replace(/\s+$/, ""))
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .replace(/^\n+|\n+$/g, "");
}

const ENTITIES: Record<string, string> = {
  amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ", ensp: " ", emsp: " ", thinsp: " ",
  zwnj: "", zwj: "", shy: "", lrm: "", rlm: "",
  rsquo: "’", lsquo: "‘", rdquo: "”", ldquo: "“", sbquo: "‚", bdquo: "„",
  mdash: "—", ndash: "–", hellip: "…", bull: "•", middot: "·", copy: "©", reg: "®", trade: "™",
  laquo: "«", raquo: "»", euro: "€", pound: "£", yen: "¥", cent: "¢", deg: "°", times: "×", divide: "÷",
  larr: "←", rarr: "→", uarr: "↑", darr: "↓", para: "¶", sect: "§", dagger: "†",
};

function decodeEntities(s: string): string {
  if (!s.includes("&")) return s;
  return s.replace(/&(#x[0-9a-f]+|#[0-9]+|[a-z][a-z0-9]*);/gi, (all, e: string) => {
    if (e[0] === "#") {
      const n = e[1] === "x" || e[1] === "X" ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
      return n > 0 && n <= 0x10ffff ? String.fromCodePoint(n) : all;
    }
    return ENTITIES[e.toLowerCase()] ?? all;
  });
}
