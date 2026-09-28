// OWNER: richtext agent. What the Composer can ask of the body editor.
//
// The editor is lazy-loaded, so the Composer holds a stable proxy: calls made
// before the editor mounts (initial focus, ⌘J right after opening) are
// replayed once it attaches.
import { useRef } from "react";
import type { AiTarget } from "./aiSuggest";

export type { AiTarget } from "./aiSuggest";

export interface BodyEditor {
  focus(where?: "start" | "end" | "keep"): void;
  /** Insert text at the caret (with a space before it when glued to a word). */
  insertText(text: string): void;
  /** Swap the signature block: the account's default, or null for none. */
  setSignature(signatureId: string | null): void;
  /**
   * The From account changed: its default signature replaces the current
   * one (none removes it); with no signature yet, it goes in when `insert`.
   */
  accountChanged(signatureId: string | null, insert: boolean): void;
  /** Id of the signature last put in by the editor (null: none or unknown). */
  signatureId(): string | null;
  /** Put inline images (already registered in inlineSrc) at document position `at`, or at the caret. */
  insertImages(images: InlineImageAttrs[], at: number | null): void;
  /** Insert an expanded snippet at the caret (replacing a selection) and select [selStart, selEnd) of it. */
  insertExpanded(text: string, selStart: number, selEnd: number): void;
  /** Replace everything written above the signature with `text` (an instant reply); the caret goes after it. */
  setWriting(text: string): void;
  /** What's written above the signature, as plain text ("" when nothing). */
  writingText(): string;
  /** What Write with AI would work on now: the selection, else the whole writing. */
  aiTarget(): AiTarget | null;
  /** Show `text` as a suggestion replacing `target` (the editor is read-only meanwhile). */
  aiPreview(target: AiTarget, text: string, phase: "writing" | "done"): void;
  /** Drop the suggestion; the text is back as it was, editable. */
  aiClear(): void;
  /** Put `text` in place of `target` as one undoable edit. */
  aiApply(target: AiTarget, text: string): void;
}

/** An inline image node's attributes (editor/schema.ts InlineImage). */
export interface InlineImageAttrs {
  cid: string;
  alt: string;
  width: number | null;
}

export class BodyHandle implements BodyEditor {
  private editor: BodyEditor | null = null;
  private queued: Array<(e: BodyEditor) => void> = [];

  attach(e: BodyEditor | null) {
    this.editor = e;
    if (!e) return;
    const q = this.queued;
    this.queued = [];
    q.forEach((f) => f(e));
  }

  private run(f: (e: BodyEditor) => void) {
    if (this.editor) f(this.editor);
    else this.queued.push(f);
  }

  focus(where: "start" | "end" | "keep" = "keep") {
    // Only the latest focus request matters.
    this.queued = this.queued.filter((f) => !(f as { focus?: true }).focus);
    const f = Object.assign((e: BodyEditor) => e.focus(where), { focus: true as const });
    this.run(f);
  }

  insertText(text: string) {
    this.run((e) => e.insertText(text));
  }

  setSignature(signatureId: string | null) {
    this.run((e) => e.setSignature(signatureId));
  }

  accountChanged(signatureId: string | null, insert: boolean) {
    this.run((e) => e.accountChanged(signatureId, insert));
  }

  signatureId(): string | null {
    return this.editor?.signatureId() ?? null;
  }

  insertImages(images: InlineImageAttrs[], at: number | null) {
    this.run((e) => e.insertImages(images, at));
  }

  insertExpanded(text: string, selStart: number, selEnd: number) {
    this.run((e) => e.insertExpanded(text, selStart, selEnd));
  }

  setWriting(text: string) {
    this.run((e) => e.setWriting(text));
  }

  writingText(): string {
    return this.editor?.writingText() ?? "";
  }

  aiTarget(): AiTarget | null {
    return this.editor?.aiTarget() ?? null;
  }

  aiPreview(target: AiTarget, text: string, phase: "writing" | "done") {
    this.editor?.aiPreview(target, text, phase);
  }

  aiClear() {
    this.editor?.aiClear();
  }

  aiApply(target: AiTarget, text: string) {
    this.editor?.aiApply(target, text);
  }
}

export function useBodyHandle(): BodyHandle {
  const ref = useRef<BodyHandle | null>(null);
  ref.current ??= new BodyHandle();
  return ref.current;
}
