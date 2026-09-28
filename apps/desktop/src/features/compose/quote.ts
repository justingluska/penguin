// The quoted original in replies and forwards, and which files a forward
// carries. Pure (no api, no DOM) so tests/quote.test.ts runs it directly.
//
// Outgoing markup is Gmail's own, so Gmail and clients that know its classes
// fold the quote the way they fold Gmail's (the backend's rule forwards
// build the same in penguin-provider/src/quote.rs):
//   forward: <div class="gmail_quote"><div dir="ltr" class="gmail_attr">
//            ---------- Forwarded message ----------<br>From: …<br></div>
//            <br><br>ORIGINAL</div>
//   reply:   <div class="gmail_quote"><div dir="ltr" class="gmail_attr">On …
//            wrote:<br></div><blockquote class="gmail_quote" style="…">
//            ORIGINAL</blockquote></div>
// ORIGINAL is the original's HTML body as the backend sanitized it
// (quote_sources → penguin-render sanitize_quoted_html: formatting, lists and
// links kept; nothing active, remote or tracking), or its text as escaped
// paragraphs when it has no HTML part or it hasn't loaded.
import type { AttachmentMeta, MessageView, OutgoingAttachment } from "../../lib/types";

export interface Quote {
  /** "On Apr 18, 2026, Mike Delgado <mike@…> wrote:" or the forward header. */
  header: string;
  text: string;
  forward: boolean;
  /** The original's body as sanitized HTML (quote_sources); null/absent = quote the text. */
  html?: string | null;
}

/** Gmail's quote bar, in values the outgoing sanitizer keeps (no rgb()). */
export const QUOTE_STYLE = "margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex";

/** Escape for text content (what the HTML serializer escapes, so the send-time sanitizer leaves it alone). */
export function escapeText(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/ /g, "&nbsp;");
}

/** Plain text as paragraphs (blank-line separated; single newlines become <br>). */
export function textParagraphs(text: string): string {
  return text
    .split(/\n{2,}/)
    .map((p) => `<p>${escapeText(p).replace(/\n/g, "<br>")}</p>`)
    .join("");
}

/** The quoted original as HTML, in Gmail's markup (appended after what the user wrote). */
export function quoteHtml(q: Quote): string {
  const body = q.html?.trim() ? q.html : textParagraphs(q.text);
  const attr = q.header
    .split("\n")
    .map(escapeText)
    .join("<br>");
  const inner = q.forward
    ? `<div dir="ltr" class="gmail_attr">${attr}<br></div><br><br>${body}`
    : `<div dir="ltr" class="gmail_attr">${attr}<br></div><blockquote class="gmail_quote" style="${QUOTE_STYLE}">${body}</blockquote>`;
  return `<br><div class="gmail_quote">${inner}</div>`;
}

/** The quoted original in the text/plain part. */
export function quotedText(q: Quote): string {
  if (q.forward) return `${q.header}\n\n${q.text}`;
  return `${q.header}\n${q.text
    .split("\n")
    .map((l) => (l.startsWith(">") ? `>${l}` : `> ${l}`))
    .join("\n")}`;
}

// ---------------------------------------------------------------------------
// Files

/** Size in bytes, whichever form the attachment is in. */
export function attachmentSize(a: OutgoingAttachment): number {
  return a.kind === "gmail" ? a.size : Math.floor((a.dataBase64.length * 3) / 4) - (a.dataBase64.endsWith("==") ? 2 : a.dataBase64.endsWith("=") ? 1 : 0);
}

/** Same file, whichever form it's in (picked bytes, or a ref after a save); an inline image is also told apart by its Content-ID. */
export function attachmentKey(a: OutgoingAttachment): string {
  return `${a.filename}\u0000${attachmentSize(a)}${a.contentId ? `\u0000${a.contentId}` : ""}`;
}

/**
 * A stored message's attachment as a ref the backend fetches on send.
 * `accountId` (the account the message is in) lets it fetch the file even
 * when the message goes out from another account.
 */
export function refOf(messageId: string, a: AttachmentMeta, accountId?: string): OutgoingAttachment {
  return { kind: "gmail", messageId, attachmentId: a.id, filename: a.filename, mimeType: a.mimeType, size: a.size, ...(accountId ? { accountId } : {}) };
}

/** A message's files (inline images belong to its body and aren't forwarded as files). */
export function filesOf(messageId: string, attachments: AttachmentMeta[], accountId?: string): OutgoingAttachment[] {
  return attachments.filter((a) => !a.inline).map((a) => refOf(messageId, a, accountId));
}

/** `list` plus the entries of `add` it doesn't already have (by file identity). */
export function withFiles(list: OutgoingAttachment[], add: OutgoingAttachment[]): OutgoingAttachment[] {
  const have = new Set(list.map(attachmentKey));
  const out = [...list];
  for (const a of add) {
    const k = attachmentKey(a);
    if (have.has(k)) continue;
    have.add(k);
    out.push(a);
  }
  return out;
}

/** `list` without the files in `remove` (by file identity). */
export function withoutFiles(list: OutgoingAttachment[], remove: OutgoingAttachment[]): OutgoingAttachment[] {
  const drop = new Set(remove.map(attachmentKey));
  return list.filter((a) => !drop.has(attachmentKey(a)));
}

/** Whether every file in `files` is in `list`. */
export function hasAllFiles(list: OutgoingAttachment[], files: OutgoingAttachment[]): boolean {
  const have = new Set(list.map(attachmentKey));
  return files.every((f) => have.has(attachmentKey(f)));
}

export interface ForwardFiles {
  /** The forwarded message's own files. */
  own: OutgoingAttachment[];
  /** Files on the conversation's other messages that aren't among `own` (newest copy of each, oldest first). */
  earlier: OutgoingAttachment[];
}

/**
 * Files for forwarding `forwarded`. `others` (the conversation's other
 * messages, any order) only count when the whole conversation is forwarded;
 * a file sent twice (same name and size) is offered once, from its newest
 * message.
 */
export function forwardFiles(
  forwarded: { id: string; attachments: AttachmentMeta[] },
  others: { id: string; date: number; attachments: AttachmentMeta[] }[],
  accountId?: string,
): ForwardFiles {
  const own = filesOf(forwarded.id, forwarded.attachments, accountId);
  const seen = new Set(own.map(attachmentKey));
  const picked: { date: number; file: OutgoingAttachment }[] = [];
  for (const m of [...others].sort((a, b) => b.date - a.date)) {
    for (const f of filesOf(m.id, m.attachments, accountId)) {
      const k = attachmentKey(f);
      if (seen.has(k)) continue;
      seen.add(k);
      picked.push({ date: m.date, file: f });
    }
  }
  picked.sort((a, b) => a.date - b.date);
  return { own, earlier: picked.map((p) => p.file) };
}

/**
 * A forward's attachment list once its original has loaded: `current` (what
 * the user already attached) plus the forwarded message's own files, and
 * the conversation's other files (`others` in conversation order) as
 * `earlier`. Those are attached too when the forwarded message has no files
 * of its own (typically a short "see below" reply whose files are further
 * up), so forwarding a conversation from the bottom brings its files;
 * otherwise they're offered with a switch.
 */
export function forwardAttachments(
  current: OutgoingAttachment[],
  forwarded: { id: string; attachments: AttachmentMeta[] },
  others: { id: string; attachments: AttachmentMeta[] }[],
  accountId?: string,
): { attachments: OutgoingAttachment[]; earlier: OutgoingAttachment[] } {
  const files = forwardFiles(forwarded, others.map((o, i) => ({ ...o, date: i })), accountId);
  const attachments = withFiles(current, files.own);
  return { attachments: files.own.length ? attachments : withFiles(attachments, files.earlier), earlier: files.earlier };
}

/**
 * The message a reply or forward is about: the one asked for, else the
 * conversation's latest that isn't one of my unsent drafts.
 */
export function pickMessage<M extends Pick<MessageView, "id" | "labelIds">>(messages: M[], messageId: string | undefined): M | undefined {
  return messages.find((m) => m.id === messageId) ?? [...messages].reverse().find((m) => !m.labelIds.includes("DRAFT")) ?? messages[messages.length - 1];
}
