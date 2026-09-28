// Composer state: building a draft from the compose context (new / reply /
// reply all / forward), keeping unsent drafts in memory, and turning the
// editor state into the backend's Draft.
import { api } from "../../lib/api";
import { composeFontCss } from "../../lib/composeFonts";
import type { Account, Address, Draft, OutgoingAttachment } from "../../lib/types";
import type { UiState } from "../../lib/ui";
import { docToEmailHtml, escapeHtml, isDocEmpty, type Doc } from "./editor/serialize";
import { attachmentKey, attachmentSize, forwardAttachments, pickMessage, quoteHtml, quotedText, withFiles, type Quote } from "./quote";
import { sendableAttachments } from "./inline";

export { attachmentKey, attachmentSize, type Quote };

export type ComposeContext = NonNullable<UiState["composeContext"]>;

/**
 * What a reply or forward still needs from the backend once the composer is
 * up: the original's sanitized HTML (so the quote keeps its formatting) and,
 * for a forward, the files. `pending`: a message involved is headers-only
 * (older than the sync window), so its body and attachment list have to be
 * downloaded first — the composer shows that, and won't send until it's done.
 */
export interface OriginalLoad {
  accountId: string;
  /** The message quoted (and forwarded). */
  messageId: string;
  forward: boolean;
  /** Other messages whose files a whole-conversation forward offers. */
  otherIds: string[];
  pending: boolean;
}

export interface EditorState {
  key: string;
  /** Gmail draft id once the draft exists on the server. */
  draftId: string | null;
  /** Account the saved draft lives in (a From switch moves it on the next save). */
  draftAccountId: string | null;
  ctx: ComposeContext;
  accountId: string;
  to: Address[];
  cc: Address[];
  bcc: Address[];
  showCc: boolean;
  showBcc: boolean;
  subject: string;
  /** Plain text of the body (the text/plain part without the quote). */
  body: string;
  /** The rich-text editor's document (editor/schema.ts); null until the editor has loaded. */
  bodyDoc: Doc | null;
  /** A reopened draft's HTML part, already through the composer allowlist; loaded into the editor once. */
  bodyHtml: string | null;
  quote: Quote | null;
  /**
   * Files to send: picked/dropped/pasted ones carry bytes; saved or forwarded
   * ones are Gmail refs. Inline images (a pasted screenshot, the quoted
   * original's pictures) are here too, with a contentId; toDraft sends only
   * those the HTML still shows.
   */
  attachments: OutgoingAttachment[];
  /**
   * Forwarding a conversation: files on its other messages (not the one
   * forwarded). "Also attach N files from earlier messages" adds or removes
   * them from `attachments`; on by default when the forwarded message has
   * none of its own.
   */
  earlier: OutgoingAttachment[];
  /** Still to load from the backend (see OriginalLoad); null once done. */
  original: OriginalLoad | null;
  /** "Remind me if no reply" window for this message (local; not saved to Gmail). */
  remindAfterMs: number | null;
  /** Author of the message being answered (for the {sender_name} snippet variable). */
  replyingTo: Address | null;
  replyToThreadId: string | null;
  replyToMessageId: string | null;
  /** True once the user typed anything (drives "Draft kept"). */
  touched: boolean;
}

/** Unsent drafts, kept in memory when the composer is closed with Esc. */
export const drafts = new Map<string, EditorState>();

export function contextKey(ctx: ComposeContext): string {
  if (ctx.draftId) return `draft:${ctx.draftId}`;
  if (ctx.mode === "new" && ctx.messageId) return `draftmsg:${ctx.messageId}`;
  return ctx.mode === "new" ? "new" : `${ctx.mode}:${ctx.thread?.accountId}/${ctx.thread?.threadId}`;
}

export function pickAccount(accounts: Account[], threadAccount: string | undefined, filter: string | null): string {
  if (threadAccount && accounts.some((a) => a.id === threadAccount)) return threadAccount;
  if (filter && accounts.some((a) => a.id === filter)) return filter;
  return accounts[0]?.id ?? threadAccount ?? "";
}

export function blankState(ctx: ComposeContext, accountId: string): EditorState {
  return {
    key: contextKey(ctx),
    draftId: ctx.draftId ?? null,
    draftAccountId: ctx.draftId ? (ctx.thread?.accountId ?? accountId) : null,
    ctx,
    accountId,
    to: [],
    cc: [],
    bcc: [],
    showCc: false,
    showBcc: false,
    subject: "",
    body: "",
    bodyDoc: null,
    bodyHtml: null,
    quote: null,
    attachments: [],
    earlier: [],
    original: null,
    remindAfterMs: null,
    replyingTo: null,
    replyToThreadId: null,
    replyToMessageId: null,
    touched: false,
  };
}

function withPrefix(prefix: "Re:" | "Fwd:", subject: string): string {
  const s = subject.trim();
  const re = prefix === "Re:" ? /^re:/i : /^(fwd?|fw):/i;
  return re.test(s) ? s : `${prefix} ${s}`.trim();
}

function fmtAddr(a: Address): string {
  return a.name ? `${a.name} <${a.email}>` : a.email;
}

function fmtDate(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { month: "short", day: "numeric", year: "numeric", hour: "numeric", minute: "2-digit" });
}

const same = (a: Address, b: string) => a.email.toLowerCase() === b.toLowerCase();

function dedupe(list: Address[], exclude: string[]): Address[] {
  const seen = new Set(exclude.map((e) => e.toLowerCase()));
  const out: Address[] = [];
  for (const a of list) {
    const k = a.email.toLowerCase();
    if (seen.has(k)) continue;
    seen.add(k);
    out.push(a);
  }
  return out;
}

/**
 * Fill recipients, subject and quote from the thread being answered (local
 * data only). The quote starts as the original's text; `original` says what
 * loadOriginal() must still fetch (its HTML, a forward's files).
 */
export async function prefill(state: EditorState, accounts: Account[]): Promise<EditorState> {
  const { ctx } = state;
  if (ctx.mode === "new" || !ctx.thread) return state;
  const thread = await api.getThread(ctx.thread.accountId, ctx.thread.threadId);
  if (!thread || thread.messages.length === 0) return state;
  const msg = pickMessage(thread.messages, ctx.messageId)!;
  const mine = accounts.find((a) => a.id === state.accountId)?.email ?? "";
  const fromMe = mine !== "" && same(msg.from, mine);
  const next: EditorState = { ...state };

  if (ctx.mode === "forward") {
    // The whole conversation (F, the menus, the reply dock) offers the other
    // messages' files too; a message's own Forward button just its own.
    const whole = ctx.whole || !ctx.messageId;
    const others = whole ? thread.messages.filter((m) => m.id !== msg.id && !m.labelIds.includes("DRAFT")) : [];
    next.subject = withPrefix("Fwd:", thread.subject || msg.subject);
    next.quote = {
      forward: true,
      header: [
        "---------- Forwarded message ----------",
        `From: ${fmtAddr(msg.from)}`,
        `Date: ${fmtDate(msg.date)}`,
        `Subject: ${msg.subject}`,
        `To: ${msg.to.map(fmtAddr).join(", ")}`,
        ...(msg.cc.length ? [`Cc: ${msg.cc.map(fmtAddr).join(", ")}`] : []),
      ].join("\n"),
      text: msg.bodyText,
    };
    next.original = {
      accountId: thread.accountId,
      messageId: msg.id,
      forward: true,
      // (quote_sources takes at most 500 at once; the newest are the likeliest.)
      otherIds: others.slice(-400).map((m) => m.id),
      pending: !!msg.bodyPending || others.some((m) => m.bodyPending),
    };
    return next;
  }

  next.subject = withPrefix("Re:", thread.subject || msg.subject);
  next.replyToThreadId = thread.threadId;
  next.replyToMessageId = msg.id;
  next.replyingTo = msg.from;
  // Replying to my own message goes back to its original recipients.
  const primary = fromMe ? msg.to : msg.replyTo.length ? msg.replyTo : [msg.from];
  next.to = dedupe(primary, [mine]);
  if (ctx.mode === "replyAll") {
    const rest = fromMe ? msg.cc : [...msg.to, ...msg.cc];
    next.cc = dedupe(rest, [mine, ...next.to.map((a) => a.email)]);
    next.showCc = next.cc.length > 0;
  }
  next.quote = { forward: false, header: `On ${fmtDate(msg.date)}, ${fmtAddr(msg.from)} wrote:`, text: msg.bodyText };
  next.original = { accountId: thread.accountId, messageId: msg.id, forward: false, otherIds: [], pending: !!msg.bodyPending };
  return next;
}

/**
 * Fetch what `load` names (quote_sources: local, unless a message is
 * headers-only and has to be downloaded) and return how to apply it to the
 * editor state as it is by then. Rejects with the backend's error; the
 * composer shows it with Retry and doesn't send meanwhile, so files are
 * never silently left out.
 */
export async function loadOriginal(load: OriginalLoad): Promise<(s: EditorState) => EditorState> {
  const [quoted, others] = await Promise.all([
    api.quoteSources(load.accountId, [load.messageId], true),
    load.otherIds.length ? api.quoteSources(load.accountId, load.otherIds, false) : Promise.resolve([]),
  ]);
  const src = quoted[0];
  return (s) => {
    const next: EditorState = { ...s, original: null };
    if (src && s.quote) {
      next.quote = { ...s.quote, html: src.html, text: src.text?.trim() ? src.text : s.quote.text };
    }
    if (load.forward && src) {
      const files = forwardAttachments(
        s.attachments,
        { id: src.messageId, attachments: src.attachments },
        others.map((o) => ({ id: o.messageId, attachments: o.attachments })),
        load.accountId,
      );
      next.attachments = files.attachments;
      next.earlier = files.earlier;
    }
    // The quote's inline images (under the fresh ids its HTML now uses).
    if (src && s.quote && src.html) next.attachments = withFiles(next.attachments, src.inlineImages ?? []);
    return next;
  };
}

function paragraphs(text: string): string {
  return text
    .split(/\n{2,}/)
    .map((p) => `<p>${escapeHtml(p).replace(/\n/g, "<br>")}</p>`)
    .join("");
}

export function toDraft(s: EditorState): Draft {
  const body = s.body.replace(/\s+$/, "");
  const bodyText = s.quote ? `${body}\n\n${quotedText(s.quote)}` : body;
  // The editor's document when it has loaded; before that (a draft closed
  // before the editor chunk arrived) the text, as paragraphs.
  let html = s.bodyDoc ? (isDocEmpty(s.bodyDoc) ? "" : docToEmailHtml(s.bodyDoc)) : paragraphs(body);
  if (s.quote) html += quoteHtml(s.quote);
  return {
    accountId: s.accountId,
    to: s.to,
    cc: s.showCc ? s.cc : [],
    bcc: s.showBcc ? s.bcc : [],
    subject: s.subject.trim(),
    bodyText,
    // The writing font as a fallback stack; recipients without it see their own.
    bodyHtml: `<div dir="auto" style="${composeFontCss()}">${html}</div>`,
    replyToThreadId: s.replyToThreadId,
    replyToMessageId: s.replyToMessageId,
    // Images deleted from the text (or a quote gone plain) stay behind.
    attachments: sendableAttachments(s.attachments, html),
  };
}

/** Gmail's limit on a message's attachments (decoded bytes). */
export const MAX_ATTACHMENT_BYTES = 25 * 1024 * 1024;

const EMAIL = /^[^\s@<>(),;:"]+@[^\s@<>(),;:"]+\.[^\s@<>(),;:"]+$/;

/** Parse typed text ("Dana <dana@x.example>", "dana@x.example") into an address. */
export function parseAddress(raw: string): Address | null {
  const t = raw.trim().replace(/[,;]+$/, "");
  const m = /^(.*)<([^>]+)>$/.exec(t);
  if (m) {
    const email = m[2].trim();
    const name = m[1].trim().replace(/^"|"$/g, "");
    return EMAIL.test(email) ? { name: name || null, email } : null;
  }
  return EMAIL.test(t) ? { name: null, email: t } : null;
}

/** "On <date>, <who> wrote:" starts the quoted original in a saved reply. */
const QUOTE_HEADER = /\n\nOn [^\n]+ wrote:\n/;
const FORWARD_HEADER = /\n\n---------- Forwarded message ----------\n/;

/** "On Sep 24, 2026, 9:41 AM, Priya Natarajan <priya@…> wrote:" → Priya's address. */
export function senderFromQuoteHeader(header: string): Address | null {
  const m = /, ([^,<]*?)\s*<([^>]+)> wrote:$/.exec(header.trim());
  if (m) return { name: m[1].trim() || null, email: m[2].trim() };
  const bare = /, ([^\s,]+@[^\s,]+) wrote:$/.exec(header.trim());
  return bare ? { name: null, email: bare[1] } : null;
}

/** Split a saved body back into what the user wrote and the quoted original. */
export function splitQuote(bodyText: string): { body: string; quote: Quote | null } {
  const fwd = FORWARD_HEADER.exec(bodyText);
  if (fwd) {
    const rest = bodyText.slice(fwd.index + 2);
    const blank = rest.indexOf("\n\n");
    return {
      body: bodyText.slice(0, fwd.index),
      quote: { forward: true, header: blank === -1 ? rest : rest.slice(0, blank), text: blank === -1 ? "" : rest.slice(blank + 2) },
    };
  }
  const m = QUOTE_HEADER.exec(bodyText);
  if (!m) return { body: bodyText, quote: null };
  const text = bodyText
    .slice(m.index + m[0].length)
    .split("\n")
    .map((l) => l.replace(/^> ?/, ""))
    .join("\n");
  return { body: bodyText.slice(0, m.index), quote: { forward: false, header: m[0].trim(), text } };
}
