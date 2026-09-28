// Defense in depth for SearchHit.snippetHtml. Rust already escapes the text and
// only adds <mark>, but the UI must not trust that contract blindly: this
// re-escapes everything and lets through exactly `<mark>` and `</mark>`.

const ENTITY = /&(amp|lt|gt|quot|#39|#x27|apos);/g;
const DECODE: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', "#39": "'", "#x27": "'", apos: "'" };

function escapeText(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** Returns HTML containing only text and balanced <mark> elements. */
export function sanitizeSnippet(html: string): string {
  let out = "";
  let open = 0;
  // Split on the only tags we allow; everything else is text.
  for (const part of html.split(/(<\/?mark>)/i)) {
    const tag = part.toLowerCase();
    if (tag === "<mark>") {
      if (open === 0) {
        out += "<mark>";
        open = 1;
      }
      continue;
    }
    if (tag === "</mark>") {
      if (open === 1) {
        out += "</mark>";
        open = 0;
      }
      continue;
    }
    // Decode the entities Rust produced, then escape everything again, so a
    // raw "<" (or any other tag) can never survive as markup.
    out += escapeText(part.replace(ENTITY, (_, e: string) => DECODE[e]));
  }
  if (open) out += "</mark>";
  return out;
}
