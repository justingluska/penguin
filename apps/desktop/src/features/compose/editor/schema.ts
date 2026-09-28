// OWNER: richtext agent. The composer's editor schema (TipTap/ProseMirror).
//
// The schema is the first allowlist: anything pasted or loaded that isn't a
// node or mark below is dropped by the parser, whatever the sanitizer let
// through. Keys are remapped where TipTap's defaults collide with the
// composer's field jumps (⌘⇧S subject, ⌘⇧B Bcc): strike is ⌘⇧X and quote
// ⌘⇧9, as in Gmail. No DOM or CSS imports: node --test loads this file.
import { Extension, Node, getSchema, type AnyExtension, type NodeViewRenderer } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";
import { Strike } from "@tiptap/extension-strike";
import { Blockquote } from "@tiptap/extension-blockquote";
import { Link } from "@tiptap/extension-link";
import { linkAllowed } from "./serialize.ts";
import { isSafeCid } from "../inline.ts";

/** The signature block: swapped when the From account changes. */
export const Signature = Node.create({
  name: "signature",
  group: "block",
  content: "block+",
  defining: true,
  parseHTML() {
    return [{ tag: "div.penguin-signature" }, { tag: "div.gmail_signature" }];
  },
  renderHTML() {
    return ["div", { class: "penguin-signature" }, 0];
  },
});

/**
 * A pasted or dropped image, shown in the text and sent as an inline part
 * (features/compose/inline.ts). Only `cid:` sources parse: a pasted web
 * image (never reaching here, the sanitizer drops it) or any other URL is
 * not an image of this message. The picture itself comes from the
 * composer's registry (editor/imageView.ts), never from the document.
 */
export const InlineImage = Node.create({
  name: "inlineImage",
  group: "inline",
  inline: true,
  atom: true,
  draggable: true,
  selectable: true,
  addAttributes() {
    return { cid: { default: null }, alt: { default: "" }, width: { default: null } };
  },
  parseHTML() {
    return [
      {
        tag: "img[src]",
        getAttrs: (el) => {
          const m = /^cid:(.+)$/i.exec((el as Element).getAttribute("src")?.trim() ?? "");
          if (!m || !isSafeCid(m[1])) return false;
          const w = Number((el as Element).getAttribute("width"));
          return { cid: m[1], alt: (el as Element).getAttribute("alt") ?? "", width: w > 0 ? Math.round(w) : null };
        },
      },
    ];
  },
  renderHTML({ node }) {
    return ["img", { src: `cid:${node.attrs.cid}`, alt: node.attrs.alt, ...(node.attrs.width ? { width: String(node.attrs.width) } : {}) }];
  },
});

const ComposerStrike = Strike.extend({
  addKeyboardShortcuts() {
    return { "Mod-Shift-x": () => this.editor.commands.toggleStrike() };
  },
});

const ComposerBlockquote = Blockquote.extend({
  addKeyboardShortcuts() {
    return { "Mod-Shift-9": () => this.editor.commands.toggleBlockquote() };
  },
});

export interface ComposerKeyOptions {
  /** ⌘K: open the link editor (the global palette stays closed in the composer). */
  onLink: () => void;
}

/** Composer-only keys: ⌘K link, ⌘\ clear formatting. */
export const ComposerKeys = Extension.create<ComposerKeyOptions>({
  name: "composerKeys",
  addOptions() {
    return { onLink: () => {} };
  },
  addKeyboardShortcuts() {
    return {
      "Mod-k": () => {
        this.options.onLink();
        return true;
      },
      "Mod-\\": () => this.editor.chain().focus().unsetAllMarks().clearNodes().run(),
    };
  },
});

/** Everything but the editor-instance bits (placeholder, key callbacks); `imageView` draws inline images. */
export function baseExtensions(opts: { imageView?: NodeViewRenderer } = {}): AnyExtension[] {
  return [
    StarterKit.configure({
      heading: { levels: [1, 2, 3] },
      strike: false,
      blockquote: false,
      link: false,
      trailingNode: false,
      dropcursor: { color: "var(--accent)", width: 2 },
    }),
    ComposerStrike,
    ComposerBlockquote,
    Link.configure({
      openOnClick: false,
      autolink: true,
      linkOnPaste: true,
      defaultProtocol: "https",
      // Only http(s) and mailto, here and everywhere downstream.
      isAllowedUri: (url) => linkAllowed(url),
      HTMLAttributes: { target: null, rel: null, class: null },
    }),
    Signature,
    opts.imageView ? InlineImage.extend({ addNodeView: () => opts.imageView! }) : InlineImage,
  ];
}

/** The schema alone (tests, off-editor transforms). */
export function composerSchema() {
  return getSchema(baseExtensions());
}
