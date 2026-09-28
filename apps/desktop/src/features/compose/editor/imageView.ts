// OWNER: richtext agent. How an inline image looks in the editor: an <img>
// whose picture comes from the composer's registry (../inlineSrc.ts) by the
// node's cid, never from the document. Shown at the width it will be sent
// at, and never wider than the text column (editor.css).
import type { NodeViewRenderer } from "@tiptap/core";
import type { Node as PMNode } from "@tiptap/pm/model";
import { inlineSrc, subscribeInlineSrc } from "../inlineSrc";

export const inlineImageView: NodeViewRenderer = ({ node }) => {
  const img = document.createElement("img");
  img.className = "cmp-inline-img";
  let cur: PMNode = node;
  let unsubscribe = () => {};

  const paint = () => {
    const e = inlineSrc(cur.attrs.cid);
    const alt = String(cur.attrs.alt || "Image");
    img.classList.toggle("is-loading", !e?.url && !e?.error);
    img.classList.toggle("is-broken", !!e?.error);
    if (e?.url && img.getAttribute("src") !== e.url) img.src = e.url;
    img.alt = alt;
    img.title = e?.error ? `Couldn't show ${alt}: ${e.error}` : `${alt} · right-click for options`;
    if (cur.attrs.width) img.width = cur.attrs.width;
    else img.removeAttribute("width");
  };
  const watch = () => {
    unsubscribe();
    unsubscribe = subscribeInlineSrc(cur.attrs.cid, paint);
  };
  watch();
  paint();

  return {
    dom: img,
    update(n) {
      if (n.type !== cur.type) return false;
      const moved = n.attrs.cid !== cur.attrs.cid;
      cur = n;
      if (moved) watch();
      paint();
      return true;
    },
    destroy() {
      unsubscribe();
    },
  };
};
