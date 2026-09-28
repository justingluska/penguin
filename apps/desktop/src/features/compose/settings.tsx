// Compose settings mounted in Settings → Compose (features/settings/Compose.tsx):
// undo send, the send-later morning hour, instant replies and the snippet
// list; and the composer's rows in Settings → AI (Write with AI, suggested
// replies). docs/COMPOSE-SPEED.md.
import { useEffect, useRef, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import { updateSettings, useSetting } from "../../lib/settings";
import type { Snippet, SnippetFile, UndoSendSeconds } from "../../lib/types";
import { bytes as fmtBytes } from "../../lib/format";
import { toast } from "../../components/Toast";
import { Icon } from "../../components/Icon";
import { Choice, Switch } from "../settings/parts";
import { SNIPPET_VARIABLES, validTrigger } from "./snippets";
import { saveSnippets, useSnippets } from "./snippetStore";
import { parseAddress } from "./draft";
import { refreshAvailability, useAvailability } from "../summary/state";
import { writerUnavailableText } from "./writePrompt";
import "./settings.css";

function fail(e: unknown) {
  toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` });
}

const UNDO_CHOICES: UndoSendSeconds[] = [0, 5, 10, 20, 30];

export function UndoSendSetting() {
  const value = useSetting("undoSendSeconds");
  return (
    <div className="setting-row setting-tall">
      <div>
        <span className="setting-label">Undo send</span>
        <p className="st-muted">How long a sent message waits, so Z can take it back.</p>
      </div>
      <Choice<string>
        label="Undo send window"
        value={String(value)}
        options={UNDO_CHOICES.map((n) => ({ value: String(n), label: n === 0 ? "Off" : `${n} s` }))}
        onChange={(v) => updateSettings({ undoSendSeconds: Number(v) as UndoSendSeconds }).catch(fail)}
      />
    </div>
  );
}

const HOURS = [6, 7, 8, 9, 10];

export function SendLaterHourSetting() {
  const value = useSetting("sendLaterHour");
  const hours = HOURS.includes(value) ? HOURS : [...HOURS, value].sort((a, b) => a - b);
  return (
    <div className="setting-row setting-tall">
      <div>
        <span className="setting-label">Morning send time</span>
        <p className="st-muted">When “Tomorrow morning” and “Monday morning” in Send later (⌘⇧↵) go out.</p>
      </div>
      <Choice<string>
        label="Morning send time"
        value={String(value)}
        options={hours.map((h) => ({ value: String(h), label: `${h} AM` }))}
        onChange={(v) => updateSettings({ sendLaterHour: Number(v) }).catch(fail)}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Instant replies
// ---------------------------------------------------------------------------

const MAX_INSTANT = 9;

export function InstantRepliesSettings() {
  const s = useSetting("instantReplies");
  const [adding, setAdding] = useState("");
  const [editing, setEditing] = useState<{ i: number; text: string } | null>(null);

  const save = (replies: string[]) => updateSettings({ instantReplies: { ...s, replies } }).catch(fail);

  function add() {
    const t = adding.trim();
    if (!t) return;
    if (s.replies.some((r) => r.toLowerCase() === t.toLowerCase())) {
      toast({ message: "That one is already in the list" });
      return;
    }
    void save([...s.replies, t]).then(() => setAdding(""));
  }

  function move(i: number, d: -1 | 1) {
    const j = i + d;
    if (j < 0 || j >= s.replies.length) return;
    const next = [...s.replies];
    [next[i], next[j]] = [next[j], next[i]];
    void save(next);
  }

  return (
    <div className="setting-row setting-tall sn-settings" data-setting="instant-replies">
      <div className="sn-settings-inner">
        <div className="sn-settings-head">
          <div>
            <span className="setting-label">Instant replies</span>
            <p className="st-muted">
              One-liners offered when you reply, before you've written anything: <span className="kbd">⌃1</span>–<span className="kbd">⌃9</span> or a
              click puts one in, <span className="kbd">⌘↵</span> sends it. Also in the reply box under a conversation, and in ⌘K.
            </p>
          </div>
          <Switch label="Instant replies" on={s.enabled} onChange={(enabled) => updateSettings({ instantReplies: { ...s, enabled } }).catch(fail)} />
        </div>
        {s.enabled && (
          <>
            <ol className="ir-rows">
              {s.replies.map((r, i) => (
                <li key={`${i}:${r}`} className="ir-row">
                  <span className="kbd ir-key">⌃{i + 1}</span>
                  {editing?.i === i ? (
                    <input
                      className="input ir-input"
                      value={editing.text}
                      autoFocus
                      maxLength={200}
                      aria-label={`Instant reply ${i + 1}`}
                      onChange={(e) => setEditing({ i, text: e.target.value })}
                      onBlur={() => {
                        const t = editing.text.trim();
                        setEditing(null);
                        if (t && t !== r) void save(s.replies.map((x, j) => (j === i ? t : x)));
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                        else if (e.key === "Escape") {
                          e.preventDefault();
                          e.stopPropagation();
                          setEditing(null);
                        }
                      }}
                    />
                  ) : (
                    <button className="ir-text truncate" onClick={() => setEditing({ i, text: r })} title="Edit">
                      {r}
                    </button>
                  )}
                  <button className="btn btn-ghost btn-sm btn-icon" onClick={() => move(i, -1)} disabled={i === 0} aria-label={`Move “${r}” up`} title="Move up">
                    <Icon name="up" size="xs" />
                  </button>
                  <button
                    className="btn btn-ghost btn-sm btn-icon"
                    onClick={() => move(i, 1)}
                    disabled={i === s.replies.length - 1}
                    aria-label={`Move “${r}” down`}
                    title="Move down"
                  >
                    <Icon name="down" size="xs" />
                  </button>
                  <button className="btn btn-ghost btn-sm btn-icon" onClick={() => void save(s.replies.filter((_, j) => j !== i))} aria-label={`Delete “${r}”`} title="Delete">
                    <Icon name="trash" size="xs" />
                  </button>
                </li>
              ))}
            </ol>
            {s.replies.length < MAX_INSTANT && (
              <div className="ir-add">
                <input
                  className="input ir-input"
                  value={adding}
                  maxLength={200}
                  placeholder="Add a one-liner, e.g. “Works for me, thanks!”"
                  aria-label="New instant reply"
                  onChange={(e) => setAdding(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      add();
                    }
                  }}
                />
                <button className="btn btn-secondary btn-sm" onClick={add} disabled={!adding.trim()}>
                  <Icon name="plus" size="xs" />
                  Add
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Settings → AI: Write with AI and suggested replies (Apple's on-device model)
// ---------------------------------------------------------------------------

export function ComposeAiSettings() {
  const write = useSetting("writeWithAi");
  const instant = useSetting("instantReplies");
  const availability = useAvailability();
  // Apple Intelligence may have been switched on since: ask again here.
  useEffect(() => void refreshAvailability(true), []);
  const available = availability?.available === true;
  const why = availability === null ? "Checking Apple Intelligence on this Mac…" : available ? null : writerUnavailableText(availability.reason);
  return (
    <>
      <div className="setting-row setting-tall" data-setting="write-with-ai">
        <div>
          <span className="setting-label row-flex">Write with AI</span>
          <p className={"st-muted" + (why && availability ? " st-warn" : "")}>
            {why ??
              "In the composer, ⌘⇧J: draft a message from a few words, or make the selection shorter, friendlier, more formal or correct. Apple Intelligence, on this Mac; you accept or discard what it writes."}
          </p>
        </div>
        <Switch
          label="Write with AI"
          on={write && available}
          disabled={!available}
          onChange={(writeWithAi) => updateSettings({ writeWithAi }).catch(fail)}
        />
      </div>
      <div className="setting-row setting-tall" data-setting="suggested-replies">
        <div>
          <span className="setting-label row-flex">Suggested replies</span>
          <p className={"st-muted" + (why && availability ? " st-warn" : "")}>
            {why ??
              (instant.enabled
                ? "When you reply, Apple Intelligence suggests three short replies next to your instant replies. It reads the conversation on this Mac when the reply opens."
                : "Turn on Instant replies in Settings → Compose to use this.")}
          </p>
        </div>
        <Switch
          label="Suggested replies"
          on={instant.aiSuggestions && instant.enabled && available}
          disabled={!available || !instant.enabled}
          onChange={(aiSuggestions) => updateSettings({ instantReplies: { ...instant, aiSuggestions } }).catch(fail)}
        />
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// Snippets
// ---------------------------------------------------------------------------

type Editing = { original: Snippet | null; draft: Snippet };

function blankSnippet(): Snippet {
  return { id: `s-${Date.now().toString(36)}`, trigger: "", title: "", body: "", uses: 0, subject: "", cc: [], bcc: [], attachments: [] };
}

export function SnippetsSettings() {
  const snippets = useSnippets();
  const [editing, setEditing] = useState<Editing | null>(null);

  function startNew() {
    setEditing({ original: null, draft: blankSnippet() });
  }

  function remove(s: Snippet) {
    saveSnippets(snippets.filter((x) => x.id !== s.id)).catch(fail);
  }

  function commit(next: Snippet) {
    const list = editing?.original ? snippets.map((x) => (x.id === next.id ? next : x)) : [...snippets, next];
    saveSnippets(list)
      .then(() => setEditing(null))
      .catch(fail);
  }

  return (
    <div className="setting-row setting-tall sn-settings">
      <div className="sn-settings-inner">
        <div className="sn-settings-head">
          <div>
            <span className="setting-label">Snippets</span>
            <p className="st-muted">
              Type <span className="mono">;</span> and a trigger while writing, or press <span className="kbd">⌘;</span> to pick one. A snippet can
              also add a subject, Cc, Bcc and files.
            </p>
          </div>
          <button className="btn btn-secondary btn-sm" onClick={startNew} disabled={!!editing}>
            <Icon name="plus" size="xs" />
            New snippet
          </button>
        </div>

        {editing && !editing.original && (
          <SnippetEditor editing={editing} taken={snippets} onCancel={() => setEditing(null)} onSave={commit} />
        )}

        {snippets.length === 0 && !editing ? (
          <p className="st-muted sn-empty">No snippets yet.</p>
        ) : (
          <ul className="sn-rows">
            {snippets.map((s) =>
              editing?.original?.id === s.id ? (
                <li key={s.id}>
                  <SnippetEditor editing={editing} taken={snippets} onCancel={() => setEditing(null)} onSave={commit} />
                </li>
              ) : (
                <li key={s.id} className="sn-row">
                  <span className="mono sn-row-trigger">;{s.trigger}</span>
                  <span className="sn-row-title truncate">{s.title || "Untitled"}</span>
                  <span className="st-muted sn-row-body truncate">
                    {(s.cc.length > 0 || s.bcc.length > 0 || s.attachments.length > 0) && (
                      <Icon name="clip" size="2xs" className="sn-row-extra" />
                    )}
                    {s.body.replace(/\s+/g, " ")}
                  </span>
                  <span className="st-muted tnum sn-row-uses">{s.uses ? `${s.uses}×` : ""}</span>
                  <button className="btn btn-ghost btn-sm" onClick={() => setEditing({ original: s, draft: { ...s } })} disabled={!!editing}>
                    Edit
                  </button>
                  <button className="btn btn-ghost btn-sm btn-icon" onClick={() => remove(s)} title={`Delete ;${s.trigger}`} aria-label={`Delete ;${s.trigger}`}>
                    <Icon name="trash" size="xs" />
                  </button>
                </li>
              ),
            )}
          </ul>
        )}
      </div>
    </div>
  );
}

function readAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => {
      const url = String(r.result ?? "");
      resolve(url.slice(url.indexOf(",") + 1));
    };
    r.onerror = () => reject(r.error ?? new Error(`Couldn't read ${file.name}`));
    r.readAsDataURL(file);
  });
}

const MAX_SNIPPET_FILE = 25 * 1024 * 1024;

/** "Dana <dana@x.example>, sam@y.example" → the entries; an unreadable one is the error. */
export function parseAddressList(raw: string): { list: string[]; bad: string | null } {
  const parts = raw
    .split(/[,;\n]+/)
    .map((p) => p.trim())
    .filter(Boolean);
  for (const p of parts) if (!parseAddress(p)) return { list: [], bad: p };
  return { list: parts, bad: null };
}

function SnippetEditor({
  editing,
  taken,
  onCancel,
  onSave,
}: {
  editing: Editing;
  taken: Snippet[];
  onCancel: () => void;
  onSave: (s: Snippet) => void;
}) {
  const [d, setD] = useState<Snippet>(editing.draft);
  const [cc, setCc] = useState(editing.draft.cc.join(", "));
  const [bcc, setBcc] = useState(editing.draft.bcc.join(", "));
  const [more, setMore] = useState(!!(editing.draft.subject || editing.draft.cc.length || editing.draft.bcc.length || editing.draft.attachments.length));
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const bodyRef = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  function insertVar(v: string) {
    const el = bodyRef.current;
    const at = el?.selectionStart ?? d.body.length;
    const end = el?.selectionEnd ?? at;
    setD({ ...d, body: d.body.slice(0, at) + v + d.body.slice(end) });
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(at + v.length, at + v.length);
    });
  }

  async function addFiles(files: File[]) {
    if (!files.length) return;
    const tooBig = files.find((f) => f.size > MAX_SNIPPET_FILE);
    if (tooBig) return setError(`“${tooBig.name}” is over 25 MB, the most an email can carry.`);
    if (d.attachments.length + files.length > 10) return setError("A snippet can carry up to 10 files.");
    setUploading(true);
    setError(null);
    try {
      const saved: SnippetFile[] = [];
      for (const f of files) saved.push(await api.saveSnippetFile(f.name, f.type || "application/octet-stream", await readAsBase64(f)));
      setD((cur) => ({ ...cur, attachments: [...cur.attachments, ...saved.filter((s) => !cur.attachments.some((a) => a.id === s.id))] }));
    } catch (e) {
      setError(`Couldn't keep that file: ${asCommandError(e).message}`);
    } finally {
      setUploading(false);
    }
  }

  function save() {
    const trigger = d.trigger.trim().toLowerCase().replace(/^;/, "");
    if (!validTrigger(trigger)) return setError("Triggers use 1–32 lowercase letters, digits, - or _.");
    if (taken.some((s) => s.trigger === trigger && s.id !== d.id)) return setError(`;${trigger} is already used by another snippet.`);
    if (!d.body.trim()) return setError("Write the text the snippet inserts.");
    const c = parseAddressList(cc);
    if (c.bad) return setError(`“${c.bad}” in Cc isn't an email address.`);
    const b = parseAddressList(bcc);
    if (b.bad) return setError(`“${b.bad}” in Bcc isn't an email address.`);
    onSave({ ...d, trigger, title: d.title.trim() || trigger, subject: d.subject.trim(), cc: c.list, bcc: b.list });
  }

  return (
    <div
      className="sn-editor"
      onKeyDown={(e) => {
        if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
          e.preventDefault();
          save();
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onCancel();
        }
      }}
    >
      <div className="sn-editor-row">
        <label className="sn-field sn-field-trigger">
          <span className="st-muted">Trigger</span>
          <span className="sn-trigger-input">
            <span className="mono">;</span>
            <input
              className="input mono"
              value={d.trigger}
              onChange={(e) => setD({ ...d, trigger: e.target.value })}
              placeholder="thx"
              maxLength={33}
              autoFocus
              spellCheck={false}
            />
          </span>
        </label>
        <label className="sn-field sn-field-title">
          <span className="st-muted">Name</span>
          <input className="input" value={d.title} onChange={(e) => setD({ ...d, title: e.target.value })} placeholder="Thanks + next steps" maxLength={80} />
        </label>
      </div>
      <label className="sn-field">
        <span className="st-muted">Text</span>
        <textarea
          ref={bodyRef}
          className="input sn-body-input"
          value={d.body}
          onChange={(e) => setD({ ...d, body: e.target.value })}
          placeholder="Thanks, {first_name}. {cursor}"
          maxLength={10_000}
          rows={5}
        />
      </label>
      <div className="sn-vars">
        {SNIPPET_VARIABLES.map(([v, what]) => (
          <button key={v} className="sn-var" title={what} onClick={() => insertVar(v)} type="button">
            {v}
          </button>
        ))}
        <span className="st-muted">Other {"{placeholders}"} stay in the text; Tab jumps between them, and Send asks first while one is left.</span>
      </div>
      {more ? (
        <div className="sn-extras">
          <label className="sn-field">
            <span className="st-muted">Subject (used when the message has none)</span>
            <input className="input" value={d.subject} onChange={(e) => setD({ ...d, subject: e.target.value })} placeholder="Optional" maxLength={200} />
          </label>
          <div className="sn-editor-row">
            <label className="sn-field sn-field-title">
              <span className="st-muted">Cc</span>
              <input className="input" value={cc} onChange={(e) => setCc(e.target.value)} placeholder="Name <someone@company.example>, …" spellCheck={false} />
            </label>
            <label className="sn-field sn-field-title">
              <span className="st-muted">Bcc</span>
              <input className="input" value={bcc} onChange={(e) => setBcc(e.target.value)} placeholder="Optional" spellCheck={false} />
            </label>
          </div>
          <div className="sn-field">
            <span className="st-muted">Files (kept on this Mac, attached each time)</span>
            <div className="sn-files">
              {d.attachments.map((f) => (
                <span key={f.id} className="cmp-att">
                  <Icon name="clip" size="2xs" />
                  <span className="cmp-att-name truncate">{f.filename}</span>
                  <span className="cmp-att-size">{fmtBytes(f.size)}</span>
                  <button
                    type="button"
                    className="cmp-att-x"
                    aria-label={`Remove ${f.filename}`}
                    onClick={() => setD({ ...d, attachments: d.attachments.filter((a) => a.id !== f.id) })}
                  >
                    <Icon name="x" size="2xs" />
                  </button>
                </span>
              ))}
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => fileRef.current?.click()} disabled={uploading}>
                <Icon name="plus" size="xs" />
                {uploading ? "Adding…" : "Add file"}
              </button>
              <input
                ref={fileRef}
                type="file"
                multiple
                hidden
                onChange={(e) => {
                  const files = [...(e.target.files ?? [])];
                  e.target.value = "";
                  void addFiles(files);
                }}
              />
            </div>
          </div>
        </div>
      ) : (
        <button type="button" className="btn btn-ghost btn-sm sn-more" onClick={() => setMore(true)}>
          <Icon name="plus" size="xs" />
          Subject, Cc, Bcc, files
        </button>
      )}
      {error && <p className="sn-error">{error}</p>}
      <div className="sn-editor-actions">
        <button className="btn btn-ghost btn-sm" onClick={onCancel}>
          Cancel <span className="kbd">Esc</span>
        </button>
        <button className="btn btn-primary btn-sm" onClick={save} disabled={uploading}>
          Save <span className="kbd">⌘↵</span>
        </button>
      </div>
    </div>
  );
}
