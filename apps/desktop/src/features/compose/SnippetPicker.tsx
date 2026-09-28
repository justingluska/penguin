// ⌘; in the composer: every snippet, filtered as you type, with a preview of
// the text as it will go in (variables filled from this message) and what
// else it brings (subject, Cc, Bcc, files). ↵ inserts at the caret; the
// inline ";trigger" completion in the body (editor/RichBody.tsx) is the other
// way in. Esc closes it and goes back to the text.
import { useEffect, useMemo, useRef, useState } from "react";
import type { Snippet } from "../../lib/types";
import { bytes as fmtBytes } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { openSettings } from "../settings/state";
import { expandSnippet, matchSnippets, type SnippetContext } from "./snippets";

export function SnippetPicker({
  snippets,
  context,
  onPick,
  onClose,
}: {
  snippets: Snippet[];
  context: SnippetContext;
  onPick: (s: Snippet) => void;
  onClose: () => void;
}) {
  const [q, setQ] = useState("");
  const [idx, setIdx] = useState(0);
  const list = useMemo(() => matchSnippets(snippets, q.replace(/^;/, "")), [snippets, q]);
  const cur = list[Math.min(idx, list.length - 1)] ?? null;
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => setIdx(0), [q]);
  useEffect(() => {
    listRef.current?.querySelector(".menu-item.active")?.scrollIntoView({ block: "nearest" });
  }, [idx]);

  return (
    <div
      className="panel cmp-snippet-picker"
      role="dialog"
      aria-label="Insert snippet"
      onKeyDown={(e) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          if (list.length) setIdx((i) => (Math.min(i, list.length - 1) + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length);
        } else if (e.key === "Enter" && !e.metaKey && !e.ctrlKey) {
          e.preventDefault();
          if (cur) onPick(cur);
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="cmp-sp-search">
        <Icon name="zap" size="xs" className="faint" />
        <input
          className="cmp-input"
          autoFocus
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="Find a snippet by trigger, name or text"
          aria-label="Find a snippet"
          spellCheck={false}
        />
        <span className="kbd">Esc</span>
      </div>
      {snippets.length === 0 ? (
        <div className="cmp-sp-empty">
          <p>No snippets yet.</p>
          <button className="btn btn-secondary btn-sm" onClick={() => (onClose(), openSettings("compose"))}>
            Create one in Settings
          </button>
        </div>
      ) : (
        <div className="cmp-sp-grid">
          <div className="cmp-sp-list" ref={listRef} role="listbox" aria-label="Snippets">
            {list.map((s, i) => (
              <div
                key={s.id}
                role="option"
                aria-selected={s === cur}
                className={`menu-item${s === cur ? " active" : ""}`}
                onMouseMove={() => setIdx(i)}
                onMouseDown={(e) => {
                  e.preventDefault();
                  onPick(s);
                }}
              >
                <span className="sn-key">;{s.trigger}</span>
                <span className="truncate">{s.title || s.trigger}</span>
                {(s.attachments.length > 0 || s.cc.length > 0 || s.bcc.length > 0) && <Icon name="clip" size="2xs" className="faint cmp-sp-extra" />}
              </div>
            ))}
            {list.length === 0 && <div className="cmp-sp-none faint">No snippet matches “{q}”</div>}
          </div>
          <div className="sn-preview cmp-sp-preview">
            {cur ? (
              <>
                {cur.subject && (
                  <div className="cmp-sp-meta">
                    <span className="faint">Subject</span> {cur.subject}
                  </div>
                )}
                {cur.cc.length > 0 && (
                  <div className="cmp-sp-meta">
                    <span className="faint">Cc</span> {cur.cc.join(", ")}
                  </div>
                )}
                {cur.bcc.length > 0 && (
                  <div className="cmp-sp-meta">
                    <span className="faint">Bcc</span> {cur.bcc.join(", ")}
                  </div>
                )}
                <p className="sn-body">
                  {expandSnippet(cur.body, context, { keepCursor: true })
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
                {cur.attachments.length > 0 && (
                  <div className="cmp-sp-files">
                    {cur.attachments.map((f) => (
                      <span key={f.id} className="cmp-att">
                        <Icon name="clip" size="2xs" />
                        <span className="cmp-att-name truncate">{f.filename}</span>
                        <span className="cmp-att-size">{fmtBytes(f.size)}</span>
                      </span>
                    ))}
                  </div>
                )}
                <div className="sn-foot">
                  <span className="kbd">↵</span>Insert<span className="kbd">Tab</span>Next variable
                </div>
              </>
            ) : null}
          </div>
        </div>
      )}
    </div>
  );
}
