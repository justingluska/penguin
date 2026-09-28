// Write with AI inside the editor: what the on-device model is writing shows
// in place, as a suggestion, until it's accepted or discarded. The text it
// would replace is struck through; the new text follows it in the accent
// color. Nothing in the document changes until Accept (one undoable step).
//
// ProseMirror-only (decorations, no DOM beyond the widget), driven by
// transactions carrying `aiKey` meta from RichBody's handle.
import { Extension } from "@tiptap/core";
import { Plugin, PluginKey, type EditorState } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import type { Node as PMNode } from "@tiptap/pm/model";
import { findSignature } from "./signature.ts";

/** What the model is rewriting: a selection, or everything written above the signature. */
export interface AiTarget {
  from: number;
  to: number;
  /** The text in [from, to): one line per paragraph, "\n" for line breaks. */
  text: string;
  scope: "selection" | "writing";
}

export interface AiPreview {
  target: AiTarget;
  text: string;
  phase: "writing" | "done";
}

export const aiKey = new PluginKey<AiPreview | null>("penguinAiSuggest");

/** Everything above the signature (the whole document without one). */
export function writingRange(doc: PMNode): { from: number; to: number } {
  const sig = findSignature(doc);
  return { from: 0, to: sig ? sig.pos : doc.content.size };
}

/** Plain text of a range: one line per block, line breaks as "\n", no runs of blank lines. */
export function rangeText(doc: PMNode, from: number, to: number): string {
  return doc
    .textBetween(from, to, "\n", (leaf) => (leaf.type.name === "hardBreak" ? "\n" : ""))
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

/** What Write with AI works on: the selection (clipped to the writing), else the whole writing. */
export function aiTargetOf(state: EditorState): AiTarget {
  const { doc, selection } = state;
  const w = writingRange(doc);
  if (!selection.empty) {
    const from = Math.max(w.from, selection.from);
    const to = Math.min(w.to, selection.to);
    const text = to > from ? rangeText(doc, from, to) : "";
    if (text) return { from, to, text, scope: "selection" };
  }
  return { ...w, text: rangeText(doc, w.from, w.to), scope: "writing" };
}

/**
 * Editor content for plain text: one paragraph per line (the composer's
 * convention, see textToDoc), or inline text when it's one line going into
 * the middle of a paragraph.
 */
export function textContent(text: string, inline: boolean): Array<Record<string, unknown>> {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  if (inline && lines.length === 1) return lines[0] ? [{ type: "text", text: lines[0] }] : [];
  return lines.map((l) => (l ? { type: "paragraph", content: [{ type: "text", text: l }] } : { type: "paragraph" }));
}

export const AiSuggest = Extension.create({
  name: "aiSuggest",
  addProseMirrorPlugins() {
    return [
      new Plugin<AiPreview | null>({
        key: aiKey,
        state: {
          init: () => null,
          apply(tr, prev) {
            const meta = tr.getMeta(aiKey) as AiPreview | null | undefined;
            if (meta !== undefined) return meta;
            if (!prev || !tr.docChanged) return prev;
            // The document changed under the suggestion (a signature swap):
            // keep pointing at the same text.
            const from = tr.mapping.map(prev.target.from, 1);
            const to = Math.max(from, tr.mapping.map(prev.target.to, -1));
            return { ...prev, target: { ...prev.target, from, to } };
          },
        },
        props: {
          decorations(state) {
            const p = aiKey.getState(state);
            if (!p) return null;
            const { from, to } = p.target;
            const decos: Decoration[] = [];
            if (to > from) decos.push(Decoration.inline(from, to, { class: "cmp-ai-old" }));
            // Between blocks (the whole writing) the suggestion is a block of its own.
            const $to = state.doc.resolve(to);
            const block = !$to.parent.inlineContent;
            decos.push(
              Decoration.widget(
                to,
                () => {
                  const el = document.createElement(block ? "div" : "span");
                  el.className = `cmp-ai-new${block ? " is-block" : ""}${p.phase === "writing" ? " is-writing" : ""}`;
                  el.textContent = p.text;
                  el.setAttribute("aria-live", "polite");
                  el.setAttribute("aria-label", "Suggested text");
                  return el;
                },
                { side: 1, key: `ai:${p.phase}:${p.text.length}:${p.text.slice(-24)}`, ignoreSelection: true },
              ),
            );
            return DecorationSet.create(state.doc, decos);
          },
        },
      }),
    ];
  },
});
