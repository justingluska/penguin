// OWNER: richtext agent. The composer's rich-text body (lazy chunk).
//
// TipTap (ProseMirror) with the composer schema (schema.ts): marks, links,
// lists, quotes, code, three heading sizes, inline images, and a signature
// block. Keeps the plain-text box's behaviors: ";" snippets, ":" emoji, Tab
// to the next {variable}, the quoted original folds under "…". Pasted or
// dropped images go in the text (sent as inline parts; right-click → Send as
// attachment); other files attach.
//
// Untrusted HTML never reaches the editor raw: pastes and drops go through
// penguin-render's composer allowlist (api.sanitizeComposeHtml) first, and
// the schema parser drops whatever isn't a composer node or mark.
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { EditorContent, useEditor, useEditorState, type Editor } from "@tiptap/react";
import { Placeholder } from "@tiptap/extensions";
import { Fragment, type Node as PMNode } from "@tiptap/pm/model";
import { TextSelection } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import { api, asCommandError } from "../../../lib/api";
import { currentSettings, useSettings } from "../../../lib/settings";
import { Icon } from "../../../components/Icon";
import { toast } from "../../../components/Toast";
import { openSettings } from "../../settings/state";
import { expandSnippet, matchSnippets, type Snippet, type SnippetContext } from "../snippets";
import { matchEmoji } from "../emoji";
import type { Quote } from "../draft";
import { ComposerKeys, baseExtensions } from "./schema";
import { docToText, normalizeHref, textToDoc, type Doc } from "./serialize";
import { findSignature, setSignature, signatureNode } from "./signature";
import { htmlToBlocks, withoutQuote } from "./dom";
import type { BodyEditor, BodyHandle } from "./handle";
import { inlineImageView } from "./imageView";
import { plainTextSlice } from "./textPaste";
import { AiSuggest, aiKey, aiTargetOf, rangeText, textContent, writingRange, type AiTarget } from "./aiSuggest";
import "./editor.css";
import { useDismiss } from "../../../lib/dismiss";

/** Transactions the editor makes by itself (signature insert): not an edit. */
const AUTO = "penguin-auto";

export interface RichBodyProps {
  handle: BodyHandle;
  /** Where the content comes from, in order: an in-memory doc, sanitized HTML, plain text. */
  initial: { doc: Doc | null; html: string | null; text: string; hasQuote: boolean };
  /** Signature to put in when the editor is created (a fresh message). */
  signatureOnCreate: string | null;
  onChange: (doc: Doc, text: string, byUser: boolean) => void;
  quote: Quote | null;
  quoteOpen: boolean;
  setQuoteOpen: (v: boolean) => void;
  snippets: Snippet[];
  snippetContext: () => SnippetContext;
  onSnippetUsed: (s: Snippet) => void;
  /** Files pasted or dropped: images go in the text at `at` (null: the caret), others attach. */
  onPasteFiles: (files: File[], at: number | null) => void;
  /** "Send as attachment" on an inline image (already taken out of the text). */
  onImageAsFile: (cid: string) => void;
}

/** The right-click menu on an inline image. */
interface ImageMenu {
  cid: string;
  pos: number;
  top: number;
  left: number;
}

/** What the caret is completing: ";trigger" (snippets) or ":name" (emoji). */
interface Trigger {
  kind: "snippet" | "emoji";
  /** Document position of the ";" / ":". */
  start: number;
  typed: string;
}

type Pick = { kind: "snippet"; snippet: Snippet } | { kind: "emoji"; name: string; glyph: string };

interface LinkEdit {
  href: string;
  /** Asked only when nothing is selected and the caret isn't in a link. */
  text: string | null;
  existing: boolean;
  error: string | null;
}

export default function RichBody(props: RichBodyProps) {
  const { handle, quote, quoteOpen, setQuoteOpen, snippets, snippetContext } = props;
  const wrapRef = useRef<HTMLDivElement>(null);
  const pickerRef = useRef<HTMLDivElement>(null);
  const [trigger, setTrigger] = useState<Trigger | null>(null);
  const [dismissed, setDismissed] = useState<number | null>(null);
  const [idx, setIdx] = useState(0);
  const [pos, setPos] = useState({ top: 0, left: 0 });
  const [link, setLink] = useState<LinkEdit | null>(null);
  const [linkPos, setLinkPos] = useState({ top: 0, left: 0 });
  const [sigId, setSigId] = useState<string | null>(null);
  const [sigMenu, setSigMenu] = useState(false);
  const [imgMenu, setImgMenu] = useState<ImageMenu | null>(null);
  const imgMenuRef = useRef<HTMLDivElement>(null);
  useDismiss(!!imgMenu, () => setImgMenu(null), [imgMenuRef]);
  const settings = useSettings();

  const picks: Pick[] = useMemo(() => {
    if (!trigger) return [];
    if (trigger.kind === "emoji") return matchEmoji(trigger.typed).map(([name, glyph]) => ({ kind: "emoji", name, glyph }));
    return matchSnippets(snippets, trigger.typed)
      .slice(0, 6)
      .map((snippet) => ({ kind: "snippet", snippet }));
  }, [trigger, snippets]);
  const open = !!trigger && picks.length > 0 && dismissed !== trigger.start;
  const cur = open ? picks[Math.min(idx, picks.length - 1)] : null;

  // Handlers run inside ProseMirror callbacks: read the latest render's values.
  const live = useRef({ open, cur, picks, trigger, props });
  live.current = { open, cur, picks, trigger, props };
  const plainPasteAt = useRef(0);
  const pastedAt = useRef(0);
  const inSanitizedPaste = useRef(false);

  const editor = useEditor({
    extensions: [
      ...baseExtensions({ imageView: inlineImageView }),
      Placeholder.configure({
        // Only the first line of the message gets the hint.
        placeholder: ({ pos: p }) => (p === 0 ? "Write your message…  ; snippets · :emoji · ⌘K link" : ""),
      }),
      ComposerKeys.configure({ onLink: () => openLinkEditor() }),
      AiSuggest,
    ],
    content: initialContent(props.initial),
    editorProps: {
      attributes: { class: "cmp-text cmp-rich-text", "aria-label": "Message", "aria-multiline": "true", role: "textbox", spellcheck: "true" },
      handleKeyDown: (view, e) => onKeyDown(view, e),
      handleDOMEvents: {
        // ProseMirror claims Esc (preventDefault) on every keydown; with no
        // picker open, skip its handling so Esc reaches the Composer and closes.
        keydown: (_view, e) => e.key === "Escape" && !(live.current.open && live.current.cur),
        contextmenu: (view, e) => onContextMenu(view, e),
      },
      // ⌘-click (Ctrl-click) a link: open it in the browser, as in Gmail; a plain
      // click keeps editing, and ⌘K edits the link.
      handleClick: (view, pos, e) => onLinkClick(view, pos, e),
      handlePaste: (view, e) => onPaste(view, e),
      clipboardTextParser: (text, _ctx, _plain, view) => plainTextSlice(view.state.schema, text),
      handleDrop: (view, e, _slice, moved) => onDrop(view, e as DragEvent, moved),
    },
    onCreate: ({ editor: ed }) => {
      // Email HTML writes a blank line as <p><br></p>; the parser reads that
      // as a line break inside the line, which would show two lines tall.
      const tr = ed.state.tr;
      ed.state.doc.descendants((n, p) => {
        if (n.type.name === "paragraph" && n.childCount === 1 && n.firstChild!.type.name === "hardBreak") {
          tr.delete(tr.mapping.map(p + 1), tr.mapping.map(p + 2));
        }
      });
      if (tr.docChanged) ed.view.dispatch(tr.setMeta(AUTO, true).setMeta("addToHistory", false));
      if (props.signatureOnCreate) applySignature(ed, props.signatureOnCreate, true);
      // Report the parsed document once, so the draft state holds it (not an edit).
      report(ed, false);
    },
    onUpdate: ({ editor: ed, transaction }) => {
      report(ed, !transaction.getMeta(AUTO));
      requestAnimationFrame(readTrigger);
    },
    onSelectionUpdate: () => readTrigger(),
    onBlur: () => setTrigger(null),
  });

  function report(ed: Editor, byUser: boolean) {
    const doc = ed.getJSON();
    live.current.props.onChange(doc, docToText(doc), byUser);
  }

  // ---- the handle the Composer drives --------------------------------------

  useEffect(() => {
    if (!editor) return;
    const body: BodyEditor = {
      focus: (where) => {
        if (where === "start") editor.commands.focus("start");
        else if (where === "end") {
          const { doc } = editor.state;
          const sel = TextSelection.near(doc.resolve(endOfWriting(doc)), -1);
          editor.view.dispatch(editor.state.tr.setSelection(sel).scrollIntoView());
          editor.commands.focus();
        }
        else editor.commands.focus();
      },
      insertText: (text) => {
        const { from } = editor.state.selection;
        const before = editor.state.doc.textBetween(Math.max(0, from - 1), from, "\n", "\n");
        const lead = before && !/\s/.test(before) ? " " : "";
        editor.chain().focus().insertContent(lead + text).run();
      },
      setSignature: (id) => applySignature(editor, id, false),
      accountChanged: (id, insertIfNone) => {
        if (findSignature(editor.state.doc) || (id && insertIfNone)) applySignature(editor, id, false);
      },
      signatureId: () => sigIdRef.current,
      insertImages: (images, at) => {
        if (!images.length) return;
        const nodes = images.map((attrs) => ({ type: "inlineImage", attrs }));
        const chain = editor.chain().focus();
        if (at === null) chain.insertContent(nodes);
        else chain.insertContentAt(Math.max(0, Math.min(at, editor.state.doc.content.size)), nodes);
        chain.run();
      },
      insertExpanded: (text, selStart, selEnd) => {
        const { from, to } = editor.state.selection;
        replaceRange(from, to, text, selStart, selEnd);
      },
      setWriting: (text) => {
        applyText({ ...writingRange(editor.state.doc), text: "", scope: "writing" }, text);
      },
      writingText: () => {
        const w = writingRange(editor.state.doc);
        return rangeText(editor.state.doc, w.from, w.to);
      },
      aiTarget: () => aiTargetOf(editor.state),
      aiPreview: (target, text, phase) => {
        editor.setEditable(false, false);
        editor.view.dispatch(editor.state.tr.setMeta(aiKey, { target, text, phase }).setMeta(AUTO, true).setMeta("addToHistory", false));
      },
      aiClear: () => {
        if (aiKey.getState(editor.state)) editor.view.dispatch(editor.state.tr.setMeta(aiKey, null).setMeta(AUTO, true).setMeta("addToHistory", false));
        editor.setEditable(true, false);
      },
      aiApply: (target, text) => {
        const live = aiKey.getState(editor.state)?.target ?? target;
        editor.view.dispatch(editor.state.tr.setMeta(aiKey, null).setMeta(AUTO, true).setMeta("addToHistory", false));
        editor.setEditable(true, false);
        applyText(live, text);
      },
    };
    handle.attach(body);
    return () => handle.attach(null);
  }, [editor, handle]);

  const sigIdRef = useRef<string | null>(null);
  sigIdRef.current = sigId;

  /** Put signature `id` (null: none) in place of the current one. */
  function applySignature(ed: Editor, id: string | null, auto: boolean) {
    const s = currentSettings();
    const sig = id ? s.signatures.find((x) => x.id === id) : null;
    const schema = ed.schema;
    const node = sig ? signatureNode(schema, htmlToBlocks(schema, sig.html), s.signatureSeparator) : null;
    const tr = setSignature(ed.state.tr, node);
    setSigId(sig ? sig.id : null);
    if (!tr.docChanged) return;
    if (auto) tr.setMeta(AUTO, true).setMeta("addToHistory", false);
    ed.view.dispatch(tr);
  }

  // ---- snippets + emoji ------------------------------------------------------

  function readTrigger() {
    if (!editor || editor.isDestroyed) return;
    const { selection } = editor.state;
    if (!selection.empty) return setTrigger(null);
    const $from = selection.$from;
    if (!$from.parent.isTextblock || $from.parent.type.spec.code) return setTrigger(null);
    // Hard breaks read as "\n": one character per document position.
    const before = $from.parent.textBetween(0, $from.parentOffset, "\n", "\n");
    // ";" at a word start opens snippets; ":" plus 2+ letters at a word start
    // opens emoji (so "10:30" or "Re:" never do).
    const sn = /(^|\s);([a-z0-9_-]*)$/i.exec(before);
    const em = sn ? null : /(^|[\s(])(:)([a-z0-9_+-]{2,})$/i.exec(before);
    const caret = $from.pos;
    const next: Trigger | null = sn
      ? { kind: "snippet", start: caret - sn[2].length - 1, typed: sn[2] }
      : em
        ? { kind: "emoji", start: caret - em[3].length - 1, typed: em[3] }
        : null;
    setTrigger((t) => (t && next && t.kind === next.kind && t.start === next.start && t.typed === next.typed ? t : next));
  }

  useLayoutEffect(() => setIdx(0), [trigger?.typed, trigger?.kind]);
  useLayoutEffect(() => {
    const wrap = wrapRef.current;
    if (!open || !trigger || !wrap || !editor) return;
    const c = editor.view.coordsAtPos(trigger.start);
    const box = wrap.getBoundingClientRect();
    const h = pickerRef.current?.offsetHeight ?? 220;
    const below = c.bottom - box.top + 6;
    const above = c.top - box.top - h - 6;
    const top = below + h > wrap.clientHeight && above >= 0 ? above : below;
    const width = pickerRef.current?.offsetWidth ?? 480;
    const left = Math.min(c.left - box.left - 12, wrap.clientWidth - width - 12);
    setPos({ top, left: Math.max(12, left) });
  }, [open, trigger, cur, editor]);

  /** Replace the trigger with `text` (newlines become line breaks) and select [selStart, selEnd) of it. */
  function replaceTrigger(text: string, selStart: number, selEnd: number) {
    const t = live.current.trigger;
    if (!editor || !t) return;
    replaceRange(t.start, editor.state.selection.from, text, selStart, selEnd);
  }

  /** Put `text` (newlines become line breaks) in [from, to) and select [selStart, selEnd) of it. */
  function replaceRange(from: number, to: number, text: string, selStart: number, selEnd: number) {
    if (!editor) return;
    const { state } = editor;
    const schema = state.schema;
    const nodes: PMNode[] = [];
    text.split("\n").forEach((line, i) => {
      if (i > 0) nodes.push(schema.nodes.hardBreak.create());
      if (line) nodes.push(schema.text(line));
    });
    const tr = state.tr.replaceWith(from, to, Fragment.fromArray(nodes));
    const max = tr.doc.content.size;
    tr.setSelection(TextSelection.create(tr.doc, Math.min(max, from + selStart), Math.min(max, from + selEnd)));
    editor.view.dispatch(tr.scrollIntoView());
    setTrigger(null);
    editor.view.focus();
  }

  /**
   * Replace a target with plain text (Write with AI's accepted suggestion, an
   * instant reply): inline inside one paragraph, else one paragraph per line,
   * keeping the blank line above a signature. One undoable edit.
   */
  function applyText(target: AiTarget, text: string) {
    if (!editor) return;
    const { doc } = editor.state;
    const $from = doc.resolve(target.from);
    const $to = doc.resolve(target.to);
    const inline = target.scope === "selection" && $from.sameParent($to) && $from.parent.inlineContent;
    let content = textContent(text.replace(/^\n+|\s+$/g, ""), inline);
    if (!inline) {
      if (content.length === 0) content = [{ type: "paragraph" }];
      if (target.scope === "writing" && findSignature(doc)) content = [...content, { type: "paragraph" }];
    }
    editor.chain().focus().insertContentAt({ from: target.from, to: target.to }, content, { updateSelection: true }).run();
    if (target.scope === "writing") {
      const after = editor.state.doc;
      editor.view.dispatch(editor.state.tr.setSelection(TextSelection.near(after.resolve(endOfWriting(after)), -1)).scrollIntoView());
    }
  }

  function insert(p: Pick) {
    if (p.kind === "emoji") {
      replaceTrigger(p.glyph, p.glyph.length, p.glyph.length);
      return;
    }
    const { text, selStart, selEnd } = expandSnippet(p.snippet.body, live.current.props.snippetContext());
    replaceTrigger(text, selStart, selEnd);
    live.current.props.onSnippetUsed(p.snippet);
  }

  /** Tab: select the next {variable} a snippet left (wrapping around). */
  function nextVariable(view: EditorView): boolean {
    const { doc, selection } = view.state;
    const found: Array<[number, number]> = [];
    doc.descendants((n, p) => {
      if (!n.isText || !n.text) return;
      for (const m of n.text.matchAll(/\{[^}\n]+\}/g)) found.push([p + m.index!, p + m.index! + m[0].length]);
    });
    const hit = found.find(([a]) => a >= selection.to) ?? found[0];
    if (!hit) return false;
    view.dispatch(view.state.tr.setSelection(TextSelection.create(doc, hit[0], hit[1])).scrollIntoView());
    return true;
  }

  // ---- keys ------------------------------------------------------------------

  function onKeyDown(view: EditorView, e: KeyboardEvent): boolean {
    const { open: isOpen, cur: pick, picks: list, trigger: t } = live.current;
    const mod = e.metaKey || e.ctrlKey;
    if (isOpen && pick) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        setIdx((i) => (Math.min(i, list.length - 1) + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length);
        return true;
      }
      if ((e.key === "Enter" && !mod) || e.key === "Tab") {
        insert(pick);
        return true;
      }
      if (e.key === "Escape") {
        setDismissed(t!.start); // close the picker, not the composer
        return true;
      }
    }
    // The Composer's keys (⌘↵ send, ⌘⇧↵ later, ⌘⇧⌫ discard): keep ProseMirror
    // from also acting on them (⌘↵ would add a line break). Returning true
    // stops ProseMirror only; the key still reaches the Composer.
    if (mod && (e.key === "Enter" || (e.shiftKey && (e.key === "Backspace" || e.key === "Delete")))) return true;
    if (e.key === "Tab" && !e.shiftKey && !mod && !e.altKey && nextVariable(view)) return true;
    // ⌘⇧V: paste as plain text. WebKit fires no paste event for it, so read
    // the clipboard ourselves when none arrives.
    if (mod && e.shiftKey && e.code === "KeyV") {
      plainPasteAt.current = Date.now();
      const asked = plainPasteAt.current;
      setTimeout(() => {
        if (pastedAt.current >= asked || !navigator.clipboard?.readText) return;
        navigator.clipboard
          .readText()
          .then((text) => text && view.pasteText(text))
          .catch(() => toast({ tone: "error", message: "Couldn't read the clipboard. Use Edit → Paste instead." }));
      }, 60);
      return false;
    }
    return false;
  }

  // ---- paste + drop ----------------------------------------------------------

  function onPaste(view: EditorView, e: ClipboardEvent): boolean {
    pastedAt.current = Date.now();
    if (inSanitizedPaste.current) return false; // our own re-paste below
    const data = e.clipboardData;
    if (!data) return false;
    // Pasted files: screenshots go in the text, other files attach.
    const files = [...data.files];
    if (files.length) {
      live.current.props.onPasteFiles(files, null);
      return true;
    }
    const html = data.getData("text/html");
    const plain = Date.now() - plainPasteAt.current < 1000;
    if (plain || !html) return false; // ProseMirror's own plain-text paste
    const text = data.getData("text/plain");
    void api
      .sanitizeComposeHtml(html)
      .then((clean) => {
        if (view.isDestroyed) return;
        inSanitizedPaste.current = true;
        try {
          view.pasteHTML(clean);
        } finally {
          inSanitizedPaste.current = false;
        }
      })
      .catch((err) => {
        if (view.isDestroyed) return;
        toast({ tone: "error", message: `Pasted as plain text: ${asCommandError(err).message}` });
        if (text) view.pasteText(text);
      });
    return true;
  }

  function onDrop(view: EditorView, e: DragEvent, moved: boolean): boolean {
    if (moved || !e.dataTransfer) return false; // moving text (or an image) inside the editor
    if (e.dataTransfer.files.length) {
      // Images go in where they were dropped; the Composer attaches the rest.
      e.preventDefault();
      const files = [...e.dataTransfer.files];
      live.current.props.onPasteFiles(files, view.posAtCoords({ left: e.clientX, top: e.clientY })?.pos ?? null);
      return true;
    }
    const html = e.dataTransfer.getData("text/html");
    if (!html) return false;
    const at = view.posAtCoords({ left: e.clientX, top: e.clientY })?.pos;
    e.preventDefault();
    void api
      .sanitizeComposeHtml(html)
      .then((clean) => {
        if (view.isDestroyed || !editor) return;
        editor
          .chain()
          .focus(at ?? undefined)
          .insertContent(clean)
          .run();
      })
      .catch((err) => toast({ tone: "error", message: `Couldn't drop that: ${asCommandError(err).message}` }));
    return true;
  }

  // ---- inline images ---------------------------------------------------------

  function onContextMenu(view: EditorView, e: MouseEvent): boolean {
    const img = (e.target as Element | null)?.closest?.("img.cmp-inline-img");
    if (!img) return false;
    const pos = view.posAtDOM(img, 0);
    const node = view.state.doc.nodeAt(pos);
    if (node?.type.name !== "inlineImage") return false;
    e.preventDefault();
    const box = wrapRef.current?.getBoundingClientRect();
    if (!box) return true;
    setImgMenu({
      cid: String(node.attrs.cid),
      pos,
      top: e.clientY - box.top + 4,
      left: Math.max(8, Math.min(e.clientX - box.left, box.width - 232)),
    });
    return true;
  }

  /** Take the image out of the text; `asFile` keeps it as an attachment. */
  function dropImage(m: ImageMenu, asFile: boolean) {
    setImgMenu(null);
    if (!editor) return;
    const { state } = editor;
    const node = state.doc.nodeAt(m.pos);
    if (node?.type.name === "inlineImage" && node.attrs.cid === m.cid) {
      editor.view.dispatch(state.tr.delete(m.pos, m.pos + node.nodeSize));
    }
    if (asFile) live.current.props.onImageAsFile(m.cid);
    editor.commands.focus();
  }

  // ---- links -----------------------------------------------------------------

  function linkHrefAt(view: EditorView, pos: number): string | null {
    const isLink = (m: { type: { name: string } }) => m.type.name === "link";
    const mark = view.state.doc.nodeAt(pos)?.marks.find(isLink) ?? view.state.doc.resolve(pos).marks().find(isLink);
    return mark ? normalizeHref(String(mark.attrs.href ?? "")) : null;
  }

  function openHref(href: string) {
    api.openExternal(href).catch((err) => toast({ tone: "error", message: "Couldn't open the link", detail: asCommandError(err).message }));
  }

  function onLinkClick(view: EditorView, pos: number, e: MouseEvent): boolean {
    if (!(e.metaKey || e.ctrlKey)) return false;
    const href = linkHrefAt(view, pos);
    if (!href) return false;
    openHref(href);
    return true;
  }

  function openLinkEditor() {
    if (!editor) return;
    const { state } = editor;
    const existing = editor.isActive("link");
    const href = existing ? String(editor.getAttributes("link").href ?? "") : "";
    const selected = state.doc.textBetween(state.selection.from, state.selection.to, " ");
    const guess = !existing && normalizeHref(selected) ? selected.trim() : "";
    const c = editor.view.coordsAtPos(state.selection.from);
    const box = wrapRef.current?.getBoundingClientRect();
    if (box) setLinkPos({ top: c.bottom - box.top + 6, left: Math.max(12, Math.min(c.left - box.left - 12, box.width - 372)) });
    setLink({ href: href || guess, text: state.selection.empty && !existing ? "" : null, existing, error: null });
  }

  function applyLink() {
    if (!editor || !link) return;
    const href = normalizeHref(link.href);
    if (!href) {
      setLink({ ...link, error: "Use a web address (https://…) or an email address." });
      return;
    }
    const chain = editor.chain().focus();
    if (link.text !== null) {
      const text = link.text.trim() || href.replace(/^(mailto:|https?:\/\/)/i, "");
      const { from } = editor.state.selection;
      const before = editor.state.doc.textBetween(Math.max(0, from - 1), from, "\n", "\n");
      if (before && !/\s/.test(before)) chain.insertContent(" ");
      chain.insertContent({ type: "text", text, marks: [{ type: "link", attrs: { href } }] }).unsetMark("link").insertContent(" ");
    } else chain.extendMarkRange("link").setLink({ href });
    chain.run();
    setLink(null);
  }

  function removeLink() {
    editor?.chain().focus().extendMarkRange("link").unsetLink().run();
    setLink(null);
  }

  // ---- toolbar state ---------------------------------------------------------

  const active = useEditorState({
    editor,
    selector: ({ editor: ed }) =>
      ed
        ? {
            bold: ed.isActive("bold"),
            italic: ed.isActive("italic"),
            underline: ed.isActive("underline"),
            strike: ed.isActive("strike"),
            code: ed.isActive("code"),
            link: ed.isActive("link"),
            bullet: ed.isActive("bulletList"),
            ordered: ed.isActive("orderedList"),
            quote: ed.isActive("blockquote"),
            heading: ed.isActive("heading"),
            hasSig: !!findSignature(ed.state.doc),
          }
        : null,
  });

  const run = (f: (e: Editor) => void) => () => {
    if (editor) f(editor);
  };

  return (
    <div className="cmp-body cmp-body-live cmp-rich" ref={wrapRef}>
      <EditorContent editor={editor} className="cmp-rich-host" />
      {quote && (
        <div className="cmp-quote">
          <button className="cmp-quote-toggle" onClick={() => setQuoteOpen(!quoteOpen)} title={quoteOpen ? "Hide quoted text" : "Show quoted text"}>
            <Icon name="more" size="xs" />
          </button>
          {quoteOpen && (
            <pre className="cmp-quote-text">
              {quote.header}
              {"\n"}
              {quote.text}
            </pre>
          )}
        </div>
      )}

      <div className="cmp-fmt" role="toolbar" aria-label="Formatting" onMouseDown={(e) => e.preventDefault()}>
        <Fmt label="Bold" keys="⌘B" on={active?.bold} onClick={run((e) => e.chain().focus().toggleBold().run())}>
          <Icon name="bold" size="xs" />
        </Fmt>
        <Fmt label="Italic" keys="⌘I" on={active?.italic} onClick={run((e) => e.chain().focus().toggleItalic().run())}>
          <Icon name="italic" size="xs" />
        </Fmt>
        <Fmt label="Underline" keys="⌘U" on={active?.underline} onClick={run((e) => e.chain().focus().toggleUnderline().run())}>
          <Glyph d={GLYPHS.underline} />
        </Fmt>
        <Fmt label="Strikethrough" keys="⌘⇧X" on={active?.strike} onClick={run((e) => e.chain().focus().toggleStrike().run())}>
          <Glyph d={GLYPHS.strike} />
        </Fmt>
        <span className="cmp-fmt-sep" />
        <Fmt label="Link" keys="⌘K" on={active?.link} onClick={openLinkEditor}>
          <Icon name="link" size="xs" />
        </Fmt>
        <Fmt label="Bulleted list" keys="⌘⇧8" on={active?.bullet} onClick={run((e) => e.chain().focus().toggleBulletList().run())}>
          <Icon name="list" size="xs" />
        </Fmt>
        <Fmt label="Numbered list" keys="⌘⇧7" on={active?.ordered} onClick={run((e) => e.chain().focus().toggleOrderedList().run())}>
          <Glyph d={GLYPHS.ordered} />
        </Fmt>
        <Fmt label="Quote" keys="⌘⇧9" on={active?.quote} onClick={run((e) => e.chain().focus().toggleBlockquote().run())}>
          <Glyph d={GLYPHS.quote} />
        </Fmt>
        <Fmt label="Code" keys="⌘E" on={active?.code} onClick={run((e) => e.chain().focus().toggleCode().run())}>
          <Glyph d={GLYPHS.code} />
        </Fmt>
        <Fmt label="Heading" keys="⌘⌥2" on={active?.heading} onClick={run((e) => e.chain().focus().toggleHeading({ level: 2 }).run())}>
          <Glyph d={GLYPHS.heading} />
        </Fmt>
        <Fmt label="Clear formatting" keys="⌘\" onClick={run((e) => e.chain().focus().unsetAllMarks().clearNodes().run())}>
          <Glyph d={GLYPHS.clear} />
        </Fmt>
        <span className="grow" />
        <span className="cmp-menu-anchor">
          <button
            className={`btn btn-ghost btn-sm cmp-sig-btn${sigMenu ? " is-open" : ""}`}
            onClick={() => setSigMenu((o) => !o)}
            title="Signature"
            aria-haspopup="menu"
            aria-expanded={sigMenu}
          >
            <Glyph d={GLYPHS.signature} />
            {active?.hasSig ? (settings.signatures.find((s) => s.id === sigId)?.name ?? "Signature") : "No signature"}
            <Icon name="down" size="xs" className="faint" />
          </button>
          {sigMenu && (
            <SignatureMenu
              current={active?.hasSig ? sigId : null}
              signatures={settings.signatures}
              onPick={(id) => {
                setSigMenu(false);
                if (editor) applySignature(editor, id, false);
              }}
              onClose={() => setSigMenu(false)}
            />
          )}
        </span>
      </div>

      {imgMenu && (
        <div ref={imgMenuRef} className="panel menu cmp-img-menu" role="menu" style={{ top: imgMenu.top, left: imgMenu.left }}>
          <div role="menuitem" className="menu-item" onClick={() => dropImage(imgMenu, true)}>
            <Icon name="clip" size="xs" />
            Send as attachment
          </div>
          <div role="menuitem" className="menu-item" onClick={() => dropImage(imgMenu, false)}>
            <Icon name="trash" size="xs" />
            Remove image
          </div>
        </div>
      )}

      {link && (
        <div className="panel cmp-link" style={{ top: linkPos.top, left: linkPos.left }} onMouseDown={(e) => e.stopPropagation()}>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              applyLink();
            }}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                setLink(null);
                editor?.commands.focus();
              }
            }}
          >
            {link.text !== null && (
              <input
                className="input cmp-link-input"
                placeholder="Text to show"
                value={link.text}
                onChange={(e) => setLink({ ...link, text: e.target.value })}
                aria-label="Link text"
              />
            )}
            <input
              className="input cmp-link-input"
              placeholder="https://… or name@example.com"
              value={link.href}
              onChange={(e) => setLink({ ...link, href: e.target.value, error: null })}
              aria-label="Link address"
              autoFocus
              spellCheck={false}
            />
            {link.error && <p className="cmp-link-error">{link.error}</p>}
            <div className="cmp-link-actions">
              {link.existing && (
                <button type="button" className="btn btn-ghost btn-sm" onClick={removeLink}>
                  Remove link
                </button>
              )}
              {link.existing && normalizeHref(link.href) && (
                <button type="button" className="btn btn-ghost btn-sm" title="Open in your browser (or ⌘-click the link)" onClick={() => openHref(normalizeHref(link.href)!)}>
                  Open
                </button>
              )}
              <span className="grow" />
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => (setLink(null), editor?.commands.focus())}>
                Cancel <span className="kbd">Esc</span>
              </button>
              <button type="submit" className="btn btn-primary btn-sm">
                {link.existing ? "Update" : "Add link"} <span className="kbd">↵</span>
              </button>
            </div>
          </form>
        </div>
      )}

      {open && cur && trigger && trigger.kind === "emoji" && (
        <div ref={pickerRef} className="panel menu cmp-emoji" style={{ top: pos.top, left: pos.left }} role="listbox">
          {picks.map((p, i) =>
            p.kind === "emoji" ? (
              <div
                key={p.name}
                role="option"
                aria-selected={p === cur}
                className={`menu-item${p === cur ? " active" : ""}`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  insert(p);
                }}
                onMouseMove={() => setIdx(i)}
              >
                <span className="cmp-emoji-glyph">{p.glyph}</span>
                <span className="mono cmp-emoji-name">:{p.name}:</span>
                {p === cur && <span className="kbd">↵</span>}
              </div>
            ) : null,
          )}
        </div>
      )}
      {open && cur && cur.kind === "snippet" && trigger && (
        <div ref={pickerRef} className="snippets panel cmp-snippets" style={{ top: pos.top, left: pos.left }}>
          <div className="sn-list">
            <div className="sn-head">
              <span>Snippets</span>
              <span className="faint mono" style={{ fontSize: 11 }}>
                ;{trigger.typed}
              </span>
            </div>
            {picks.map((p, i) =>
              p.kind === "snippet" ? (
                <a
                  key={p.snippet.id}
                  className={`menu-item${p === cur ? " active" : ""}`}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    insert(p);
                  }}
                  onMouseMove={() => setIdx(i)}
                >
                  <span className="sn-key">;{p.snippet.trigger}</span>
                  <span className="truncate">{p.snippet.title}</span>
                </a>
              ) : null,
            )}
          </div>
          <div className="sn-preview">
            <div className="faint" style={{ fontSize: 11, marginBottom: 6 }}>
              Preview{cur.snippet.uses ? ` · used ${cur.snippet.uses} ${cur.snippet.uses === 1 ? "time" : "times"}` : ""}
            </div>
            <p className="sn-body">
              {expandSnippet(cur.snippet.body, snippetContext(), { keepCursor: true })
                .text.split(/(\{[^}\n]+\})/)
                .map((part, i) =>
                  i % 2 ? (
                    <span key={i} className="var">
                      {part}
                    </span>
                  ) : (
                    part
                  ),
                )}
            </p>
            <div className="sn-foot">
              <span className="kbd">↵</span>Insert<span className="kbd">Tab</span>Next variable
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/** ⌘J: the end of the last line I wrote, above the signature (else the first line). */
function endOfWriting(doc: PMNode): number {
  let end = 0;
  let done = false;
  doc.forEach((n, p) => {
    if (done || n.type.name === "signature") {
      done = true;
      return;
    }
    if (n.textContent.trim() || n.type.name !== "paragraph") end = p + n.nodeSize;
  });
  return end;
}

function initialContent(initial: RichBodyProps["initial"]): Doc | string {
  if (initial.doc) return initial.doc;
  // Already through the composer allowlist (get_draft); TipTap parses it
  // with its schema. The quoted original comes back from the text part.
  if (initial.html) return initial.hasQuote ? withoutQuote(initial.html) : initial.html;
  return textToDoc(initial.text);
}

function Fmt({ label, keys, on, onClick, children }: { label: string; keys: string; on?: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      className={`btn btn-ghost btn-sm btn-icon cmp-fmt-btn${on ? " on" : ""}`}
      onClick={onClick}
      title={`${label} (${keys})`}
      aria-label={label}
      aria-pressed={on ?? undefined}
    >
      {children}
    </button>
  );
}

/** Formatting glyphs the shared sprite doesn't have (Lucide shapes, same stroke). */
const GLYPHS = {
  underline: '<path d="M6 4v6a6 6 0 0 0 12 0V4"/><line x1="4" x2="20" y1="20" y2="20"/>',
  strike: '<path d="M16 4H9a3 3 0 0 0-2.83 4"/><path d="M14 12a4 4 0 0 1 0 8H6"/><line x1="4" x2="20" y1="12" y2="12"/>',
  ordered: '<path d="M10 12h11"/><path d="M10 18h11"/><path d="M10 6h11"/><path d="M4 10h2"/><path d="M4 6h1v4"/><path d="M6 18H4c0-1 2-2 2-3s-1-1.5-2-1"/>',
  quote: '<path d="M17 6H3"/><path d="M21 12H8"/><path d="M21 18H8"/><path d="M3 12v6"/>',
  code: '<polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>',
  heading: '<path d="M6 12h12"/><path d="M6 20V4"/><path d="M18 20V4"/>',
  clear: '<path d="M4 7V4h16v3"/><path d="M5 20h6"/><path d="M13 4 8 20"/><path d="m15 15 5 5"/><path d="m20 15-5 5"/>',
  signature: '<path d="m21 17-2.16-1.73a2.5 2.5 0 0 0-3.1-.04L14 16.5l-1.5-1.5-3 3"/><path d="M3 21h18"/><path d="M5 17 15.5 6.5a2.12 2.12 0 0 0-3-3L2 14v3z"/>',
} as const;

function Glyph({ d }: { d: string }) {
  // Static, trusted markup from GLYPHS above (no user content).
  return <svg className="i i-xs" viewBox="0 0 24 24" aria-hidden="true" dangerouslySetInnerHTML={{ __html: d }} />;
}

function SignatureMenu({
  current,
  signatures,
  onPick,
  onClose,
}: {
  current: string | null;
  signatures: { id: string; name: string }[];
  onPick: (id: string | null) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  // The anchor span holds the toggle button, whose own click closes it.
  useDismiss(true, onClose, [() => ref.current?.parentElement]);
  return (
    <div ref={ref} className="panel menu cmp-sig-menu" role="menu">
      {signatures.map((s) => (
        <div key={s.id} role="menuitemradio" aria-checked={s.id === current} className="menu-item" onClick={() => onPick(s.id)}>
          <span className="emph truncate">{s.name}</span>
          {s.id === current && <Icon name="check" size="xs" className="accent-ico" />}
        </div>
      ))}
      <div role="menuitemradio" aria-checked={current === null} className="menu-item" onClick={() => onPick(null)}>
        <span>No signature</span>
        {current === null && <Icon name="check" size="xs" className="accent-ico" />}
      </div>
      <div className="menu-sep" />
      <div role="menuitem" className="menu-item faint" onClick={() => openSettings("signatures")}>
        <Icon name="settings" size="xs" />
        Edit signatures…
      </div>
    </div>
  );
}
