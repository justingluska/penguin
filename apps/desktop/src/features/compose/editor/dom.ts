// OWNER: richtext agent. Sanitized HTML → composer nodes (browser only).
//
// Every string handed to these functions has already been through
// penguin-render's composer allowlist (get_draft, sanitize_compose_html,
// settings signatures). DOMParser builds an inert document (no scripts run,
// nothing loads), and the ProseMirror schema parser then keeps only the
// composer's own nodes and marks.
import { DOMParser as PMDOMParser, type Node as PMNode, type Schema } from "@tiptap/pm/model";

function body(html: string): HTMLElement {
  return new DOMParser().parseFromString(html, "text/html").body;
}

/** Parse sanitized HTML into a composer document. */
export function htmlToDoc(schema: Schema, html: string): PMNode {
  return PMDOMParser.fromSchema(schema).parse(body(html));
}

/** Parse sanitized HTML into top-level blocks (a signature's content). */
export function htmlToBlocks(schema: Schema, html: string): PMNode[] {
  const doc = htmlToDoc(schema, html);
  const out: PMNode[] = [];
  // A signature inside a signature would nest; take its content instead.
  doc.forEach((n) => (n.type.name === "signature" ? n.forEach((c) => out.push(c)) : out.push(n)));
  return out;
}

/**
 * A reopened reply's HTML without the quoted original (it comes back from
 * the text part and stays collapsed under the editor).
 */
export function withoutQuote(html: string): string {
  const b = body(html);
  const quote = b.querySelector(".gmail_quote");
  if (!quote) return html;
  // Everything from the quote on (Gmail puts a <br> or attribution line before it).
  let n: ChildNode | null = quote;
  while (n) {
    const next: ChildNode | null = n.nextSibling;
    n.remove();
    n = next;
  }
  return b.innerHTML;
}
