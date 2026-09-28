// OWNER: richtext agent. The rich-text box for one signature in Settings →
// Compose (lazy chunk, shares the composer's schema). Outputs the editor's
// own HTML; the backend runs it through the composer allowlist on save.
import { useState } from "react";
import { EditorContent, useEditor, useEditorState } from "@tiptap/react";
import { Placeholder } from "@tiptap/extensions";
import { Icon } from "../../../components/Icon";
import { ComposerKeys, baseExtensions } from "./schema";
import { normalizeHref } from "./serialize";
import "./editor.css";

export default function SignatureEditor({ html, onChange }: { html: string; onChange: (html: string, empty: boolean) => void }) {
  const [link, setLink] = useState<{ href: string; error: boolean } | null>(null);
  const editor = useEditor({
    extensions: [
      ...baseExtensions(),
      Placeholder.configure({ placeholder: ({ pos }) => (pos === 0 ? "Your name, role, a link…" : "") }),
      ComposerKeys.configure({ onLink: () => openLink() }),
    ],
    // Stored signatures already passed the composer allowlist.
    content: html || "",
    editorProps: { attributes: { "aria-label": "Signature", role: "textbox", "aria-multiline": "true" } },
    onUpdate: ({ editor: ed }) => onChange(ed.getHTML(), ed.isEmpty),
  });
  const on = useEditorState({
    editor,
    selector: ({ editor: ed }) => ({
      bold: !!ed?.isActive("bold"),
      italic: !!ed?.isActive("italic"),
      underline: !!ed?.isActive("underline"),
      link: !!ed?.isActive("link"),
    }),
  });

  function openLink() {
    if (!editor) return;
    setLink({ href: editor.isActive("link") ? String(editor.getAttributes("link").href ?? "") : "", error: false });
  }

  function applyLink() {
    if (!editor || !link) return;
    if (!link.href.trim()) {
      editor.chain().focus().extendMarkRange("link").unsetLink().run();
      setLink(null);
      return;
    }
    const href = normalizeHref(link.href);
    if (!href) return setLink({ ...link, error: true });
    const chain = editor.chain().focus().extendMarkRange("link");
    if (editor.state.selection.empty && !editor.isActive("link")) {
      chain.insertContent({ type: "text", text: href.replace(/^(mailto:|https?:\/\/)/i, ""), marks: [{ type: "link", attrs: { href } }] });
    } else chain.setLink({ href });
    chain.run();
    setLink(null);
  }

  const btn = (label: string, active: boolean, run: () => void, icon: React.ReactNode) => (
    <button
      type="button"
      className={`btn btn-ghost btn-sm btn-icon cmp-fmt-btn${active ? " on" : ""}`}
      title={label}
      aria-label={label}
      aria-pressed={active}
      onMouseDown={(e) => e.preventDefault()}
      onClick={run}
    >
      {icon}
    </button>
  );

  return (
    <div className="sig-editor">
      <EditorContent editor={editor} />
      <div className="cmp-fmt" role="toolbar" aria-label="Signature formatting">
        {btn("Bold (⌘B)", on?.bold ?? false, () => editor?.chain().focus().toggleBold().run(), <Icon name="bold" size="xs" />)}
        {btn("Italic (⌘I)", on?.italic ?? false, () => editor?.chain().focus().toggleItalic().run(), <Icon name="italic" size="xs" />)}
        {btn("Underline (⌘U)", on?.underline ?? false, () => editor?.chain().focus().toggleUnderline().run(), <span className="sig-u">U</span>)}
        {btn("Link (⌘K)", on?.link ?? false, openLink, <Icon name="link" size="xs" />)}
        {link && (
          <form
            className="sig-link"
            onSubmit={(e) => {
              e.preventDefault();
              applyLink();
            }}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                setLink(null);
              }
            }}
          >
            <input
              className={`input cmp-link-input${link.error ? " sig-link-bad" : ""}`}
              value={link.href}
              onChange={(e) => setLink({ href: e.target.value, error: false })}
              placeholder="https://… or name@example.com (empty removes)"
              aria-label="Link address"
              autoFocus
              spellCheck={false}
            />
          </form>
        )}
      </div>
    </div>
  );
}
