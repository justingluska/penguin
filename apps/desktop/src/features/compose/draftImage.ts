// "Copy as image": the draft as a picture on the clipboard, to paste into a
// chat and show someone before sending. Built from what the composer already
// shows: From, To, Cc and the subject as text, the body as a clone of the
// editor's own DOM (already through the composer allowlist, so no new HTML
// enters the page), and the attached files by name. The quoted original is
// left out: it's someone else's mail.
import { domToBlob } from "modern-screenshot";
import type { Address } from "../../lib/types";
import { displayName } from "../../lib/format";

export interface DraftShot {
  from: string;
  to: Address[];
  cc: Address[];
  subject: string;
  /** Attached files' names (inline pictures are in the body already). */
  files: string[];
  /** The body editor's content element (`.ProseMirror`). */
  body: HTMLElement;
}

/** Names only: the picture is for showing the words, not handing out addresses. */
const people = (list: Address[]) => list.map((a) => displayName(a)).join(", ");

function row(label: string, value: string): HTMLElement {
  const el = document.createElement("div");
  el.className = "ds-row";
  const k = document.createElement("span");
  k.className = "ds-key";
  k.textContent = label;
  const v = document.createElement("span");
  v.className = "ds-val";
  v.textContent = value;
  el.append(k, v);
  return el;
}

/** The card the picture is taken of, laid out off screen (caller appends and removes it). */
export function draftCard(d: DraftShot): HTMLElement {
  const card = document.createElement("div");
  card.className = "draft-shot";
  card.setAttribute("aria-hidden", "true");
  const head = document.createElement("div");
  head.className = "ds-head";
  head.append(row("From", d.from));
  if (d.to.length) head.append(row("To", people(d.to)));
  if (d.cc.length) head.append(row("Cc", people(d.cc)));
  const subject = document.createElement("div");
  subject.className = "ds-subject";
  subject.textContent = d.subject.trim() || "(no subject)";
  const body = d.body.cloneNode(true) as HTMLElement;
  body.removeAttribute("contenteditable");
  body.removeAttribute("role");
  body.removeAttribute("tabindex");
  body.classList.remove("ProseMirror-focused");
  body.classList.add("ds-body");
  card.append(head, subject, body);
  if (d.files.length) {
    const files = document.createElement("div");
    files.className = "ds-files";
    for (const name of d.files) {
      const f = document.createElement("span");
      f.className = "ds-file";
      f.textContent = name;
      files.append(f);
    }
    card.append(files);
  }
  return card;
}

/** Put the draft on the clipboard as a PNG. Call from the click (WebKit wants a gesture). */
export async function copyDraftImage(d: DraftShot): Promise<void> {
  if (typeof ClipboardItem === "undefined" || !navigator.clipboard?.write) throw new Error("This system can't copy pictures");
  const card = draftCard(d);
  document.body.append(card);
  // The write starts inside the click; the picture follows as a promise.
  const png = domToBlob(card, { scale: Math.max(2, window.devicePixelRatio || 1), type: "image/png" }).finally(() => card.remove());
  await navigator.clipboard.write([new ClipboardItem({ "image/png": png })]);
}
