// Reading a reopened draft's quoted original back out of its HTML part
// (browser DOM only; kept out of the editor chunk so the composer shell can
// use it). The HTML has been through penguin-render's composer allowlist
// (get_draft), and DOMParser builds an inert document.

function body(html: string): HTMLElement {
  return new DOMParser().parseFromString(html, "text/html").body;
}

/**
 * The quoted original's own HTML in a reopened reply or forward (what
 * quote.ts wrapped in Gmail's markup), so it goes out formatted again:
 * a reply's `<blockquote>` content, or a forward's content after its
 * `gmail_attr` header block. Null when the draft has no such quote (then
 * the quote comes back as text).
 */
export function quoteFromHtml(html: string): string | null {
  const quote = body(html).querySelector(".gmail_quote");
  if (!quote) return null;
  const bq = quote.querySelector(":scope > blockquote");
  if (bq) return bq.innerHTML.trim() || null;
  const attr = quote.querySelector(":scope > .gmail_attr");
  if (!attr) return null;
  const rest = quote.cloneNode(true) as Element;
  // Drop the header block and the <br>s Gmail puts after it.
  let n: ChildNode | null = rest.firstChild;
  while (n) {
    const next: ChildNode | null = n.nextSibling;
    const el = n.nodeType === 1 ? (n as Element) : null;
    const blank = n.nodeType === 3 && !(n.textContent ?? "").trim();
    if (el?.classList.contains("gmail_attr") || el?.tagName === "BR" || blank) n.remove();
    else break;
    n = next;
  }
  return rest.innerHTML.trim() || null;
}
