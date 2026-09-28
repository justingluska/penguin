// OWNER: richtext agent. Swapping the signature block in the composer
// document in place (From switch, the signature menu). ProseMirror-only, no
// DOM: covered by node --test. Which signature an account uses: pick.ts.
import { Fragment, type Node as PMNode, type Schema } from "@tiptap/pm/model";
import type { Transaction } from "@tiptap/pm/state";
import { SIG_DASH } from "./serialize.ts";

/** Where the signature block sits, if the document has one (top level). */
export function findSignature(doc: PMNode): { pos: number; node: PMNode } | null {
  let hit: { pos: number; node: PMNode } | null = null;
  doc.forEach((node, pos) => {
    if (!hit && node.type.name === "signature") hit = { pos, node };
  });
  return hit;
}

function isSepLine(n: PMNode | null | undefined): boolean {
  return !!n && n.type.name === "paragraph" && /^--\s?$/.test(n.textContent);
}

/**
 * The signature block for `blocks` (already parsed with the composer
 * schema), with the "-- " line on top when asked. A signature whose own
 * first line is already a separator doesn't get a second one.
 */
export function signatureNode(schema: Schema, blocks: PMNode[], separator: boolean): PMNode | null {
  const content = blocks.filter((b) => b.isBlock);
  if (!content.length) return null;
  const sep = separator && !isSepLine(content[0]) ? [schema.nodes.paragraph.create(null, schema.text(SIG_DASH))] : [];
  return schema.nodes.signature.create(null, Fragment.fromArray([...sep, ...content]));
}

/**
 * Put `sig` in place of the current signature (or remove it with null).
 * With no signature yet it goes at the end, after one blank line.
 */
export function setSignature(tr: Transaction, sig: PMNode | null): Transaction {
  const found = findSignature(tr.doc);
  if (found) {
    const end = found.pos + found.node.nodeSize;
    if (sig) tr.replaceWith(found.pos, end, sig);
    else {
      tr.delete(found.pos, end);
      // Don't leave the blank line that stood above it (or an empty doc).
      const last = tr.doc.lastChild;
      if (tr.doc.childCount > 1 && last && last.type.name === "paragraph" && last.content.size === 0) {
        tr.delete(tr.doc.content.size - last.nodeSize, tr.doc.content.size);
      }
    }
    return tr;
  }
  if (!sig) return tr;
  const schema = tr.doc.type.schema;
  const last = tr.doc.lastChild;
  const lead =
    last && last.type.name === "paragraph" && last.content.size === 0 && tr.doc.childCount > 1
      ? []
      : [schema.nodes.paragraph.create()];
  tr.insert(tr.doc.content.size, Fragment.fromArray([...lead, sig]));
  return tr;
}
