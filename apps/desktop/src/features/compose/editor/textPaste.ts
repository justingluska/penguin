// Plain text pasted into the composer (the conversation Copy, ⌘⇧V, text from
// a terminal): one paragraph per line and an empty paragraph per blank line,
// the composer's own shape for text (textToDoc). ProseMirror's default
// parser merges runs of newlines, which lost every blank line. No DOM, so
// node --test loads it.
import { Slice, type Schema } from "@tiptap/pm/model";
import { textToDoc } from "./serialize.ts";

export function plainTextSlice(schema: Schema, text: string): Slice {
  // A copied block usually ends with a newline; it isn't a blank line to keep.
  const trimmed = text.replace(/\r\n?/g, "\n").replace(/\n+$/, "");
  const doc = schema.nodeFromJSON(textToDoc(trimmed));
  // Open at both ends: the first and last lines join the paragraph at the caret.
  return new Slice(doc.content, 1, 1);
}
