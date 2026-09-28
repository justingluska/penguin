// OWNER: richtext agent. Settings → Signatures: the signature list (rich
// text), which accounts use each one, when it goes in by itself, and the
// "-- " separator. Its own Settings section (mounted by settings/index.tsx).
import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { asCommandError } from "../../lib/api";
import { updateSettings, useSettings } from "../../lib/settings";
import type { Account, Signature, SignatureInsert } from "../../lib/types";
import { assignDefaults } from "./editor/pick";
import { listOrderedAccounts } from "../../app/store";
import { toast } from "../../components/Toast";
import { Icon } from "../../components/Icon";
import { AccountBar, accountName } from "../../components/Identity";
import { ConfirmDialog, Section, Switch } from "../settings/parts";
import "./settings.css";
import "./signatures.css";
import { Select } from "../../components/Select";

const SignatureEditor = lazy(() => import("./editor/SignatureEditor"));

function fail(e: unknown) {
  toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` });
}

/** Defaults are keyed by lowercased account id (settings.rs normalizes them; so does assignDefaults). */
const key = (accountId: string) => accountId.toLowerCase();

/** A one-line text preview of stored (already sanitized) signature HTML. */
function preview(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  doc.querySelectorAll("p, div, li, br, h1, h2, h3").forEach((el) => el.append(" · "));
  return (doc.body.textContent ?? "").replace(/(\s*·\s*)+/g, " · ").replace(/^ · | · $/g, "").trim();
}

type Editing = { original: Signature | null; draft: Signature; empty: boolean; uses: string[] };

export function SignaturesSection() {
  const s = useSettings();
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [deleting, setDeleting] = useState<Signature | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    listOrderedAccounts().then(setAccounts, () => setAccounts([]));
  }, []);

  const usersOf = (sigId: string) => accounts.filter((a) => s.signatureDefaults[key(a.id)] === sigId);

  function start(original: Signature | null, draft: Signature, uses: string[]) {
    setError(null);
    setEditing({ original, draft, empty: !draft.html, uses });
  }

  function startNew() {
    // Accounts with no signature yet start ticked (all of them, for the first one).
    const bare = accounts.filter((a) => !s.signatureDefaults[key(a.id)]).map((a) => a.id);
    start(null, { id: `sig-${Date.now().toString(36)}`, name: "", html: "" }, bare);
  }

  function duplicate(sig: Signature) {
    start(null, { id: `sig-${Date.now().toString(36)}`, name: `${sig.name} copy`.slice(0, 60), html: sig.html }, []);
  }

  function commit() {
    if (!editing) return;
    const name = editing.draft.name.trim();
    if (!name) return setError("Give the signature a name.");
    if (editing.empty) return setError("Write the signature first.");
    const next = { ...editing.draft, name };
    const list = editing.original ? s.signatures.map((x) => (x.id === next.id ? next : x)) : [...s.signatures, next];
    updateSettings({ signatures: list, signatureDefaults: assignDefaults(s.signatureDefaults, next.id, editing.uses) })
      .then(() => setEditing(null))
      .catch(fail);
  }

  function remove(sig: Signature) {
    setDeleting(null);
    if (editing?.original?.id === sig.id) setEditing(null);
    updateSettings({
      signatures: s.signatures.filter((x) => x.id !== sig.id),
      signatureDefaults: assignDefaults(s.signatureDefaults, sig.id, []),
    }).catch(fail);
  }

  function setDefault(accountId: string, id: string) {
    const next = { ...s.signatureDefaults };
    if (id) next[key(accountId)] = id;
    else delete next[key(accountId)];
    updateSettings({ signatureDefaults: next }).catch(fail);
  }

  function setInsert(patch: Partial<SignatureInsert>) {
    updateSettings({ signatureInsert: { ...s.signatureInsert, ...patch } }).catch(fail);
  }

  const previews = useMemo(() => new Map(s.signatures.map((x) => [x.id, preview(x.html)])), [s.signatures]);

  const editor = editing && (
    <div
      className="sn-editor sig-edit"
      // Esc cancels the edit, not all of Settings.
      data-esc-owner=""
      onKeyDown={(e) => {
        if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
          e.preventDefault();
          commit();
        } else if (e.key === "Escape" && !e.defaultPrevented) {
          e.preventDefault();
          e.stopPropagation();
          setEditing(null);
        }
      }}
    >
      <label className="sn-field">
        <span className="st-muted">Name</span>
        <input
          className="input"
          value={editing.draft.name}
          onChange={(e) => setEditing({ ...editing, draft: { ...editing.draft, name: e.target.value } })}
          placeholder="Work"
          maxLength={60}
          autoFocus
        />
      </label>
      <div className="sn-field">
        <span className="st-muted">Signature</span>
        <Suspense fallback={<div className="sig-editor sig-loading" />}>
          <SignatureEditor
            html={editing.draft.html}
            onChange={(html, empty) => setEditing((cur) => (cur ? { ...cur, draft: { ...cur.draft, html }, empty } : cur))}
          />
        </Suspense>
      </div>
      {accounts.length > 0 && (
        <div className="sn-field">
          <span className="st-muted">Default for</span>
          <div className="sig-uses" role="group" aria-label="Accounts that use this signature">
            {accounts.map((a) => {
              const on = editing.uses.includes(a.id);
              const other = !on && s.signatureDefaults[key(a.id)] && s.signatureDefaults[key(a.id)] !== editing.draft.id;
              const otherName = other ? s.signatures.find((x) => x.id === s.signatureDefaults[key(a.id)])?.name : null;
              return (
                <button
                  key={a.id}
                  type="button"
                  role="checkbox"
                  aria-checked={on}
                  className={"sig-use" + (on ? " is-on" : "")}
                  title={otherName ? `${a.email} · uses "${otherName}" now` : a.email}
                  onClick={() =>
                    setEditing((cur) =>
                      cur ? { ...cur, uses: on ? cur.uses.filter((id) => id !== a.id) : [...cur.uses, a.id] } : cur,
                    )
                  }
                >
                  <span className="sig-check" aria-hidden="true">
                    {on && <Icon name="check" size="xs" />}
                  </span>
                  <AccountBar color={a.color} />
                  <span className="truncate">{accountName(a, accounts)}</span>
                  {otherName && <span className="faint truncate">· {otherName}</span>}
                </button>
              );
            })}
          </div>
        </div>
      )}
      {error && <p className="sn-error">{error}</p>}
      <div className="sn-editor-actions">
        <button className="btn btn-ghost btn-sm" onClick={() => setEditing(null)}>
          Cancel <span className="kbd">Esc</span>
        </button>
        <button className="btn btn-primary btn-sm" onClick={commit}>
          Save <span className="kbd">⌘↵</span>
        </button>
      </div>
    </div>
  );

  return (
    <Section id="signatures" icon="pencil" title="Signatures">
      <div className="setting-row setting-tall sn-settings">
        <div className="sn-settings-inner">
          <div className="sn-settings-head">
            <p className="st-muted">
              Rich text. Each account has a default, and switching From in the composer swaps it. Pick another one from
              the composer's toolbar any time.
            </p>
            <button className="btn btn-secondary btn-sm" onClick={startNew} disabled={!!editing}>
              <Icon name="plus" size="xs" />
              New signature
            </button>
          </div>
          {editing && !editing.original && editor}
          {s.signatures.length === 0 && !editing ? (
            <p className="st-muted sn-empty">No signatures yet.</p>
          ) : (
            s.signatures.length > 0 && (
              <ul className="sn-rows">
                {s.signatures.map((sig) => {
                  if (editing?.original?.id === sig.id) return <li key={sig.id}>{editor}</li>;
                  const users = usersOf(sig.id);
                  return (
                    <li key={sig.id} className="sig-row">
                      <div className="sig-row-main">
                        <span className="sn-row-title truncate">{sig.name}</span>
                        <span className="st-muted sn-row-body truncate">{previews.get(sig.id)}</span>
                      </div>
                      <span className="sig-row-users" title={users.map((a) => a.email).join("\n")}>
                        {users.length === 0 ? (
                          <span className="faint">Not a default</span>
                        ) : (
                          users.slice(0, 3).map((a) => (
                            <span key={a.id} className="sig-user">
                              <AccountBar color={a.color} />
                              <span className="truncate">{accountName(a, accounts)}</span>
                            </span>
                          ))
                        )}
                        {users.length > 3 && <span className="faint">+{users.length - 3}</span>}
                      </span>
                      <button
                        className="btn btn-ghost btn-sm"
                        onClick={() => start(sig, { ...sig }, usersOf(sig.id).map((a) => a.id))}
                        disabled={!!editing}
                      >
                        Edit
                      </button>
                      <button
                        className="btn btn-ghost btn-sm btn-icon"
                        onClick={() => duplicate(sig)}
                        disabled={!!editing || s.signatures.length >= 50}
                        title={`Duplicate ${sig.name}`}
                        aria-label={`Duplicate ${sig.name}`}
                      >
                        <Icon name="copy" size="xs" />
                      </button>
                      <button
                        className="btn btn-ghost btn-sm btn-icon"
                        onClick={() => setDeleting(sig)}
                        title={`Delete ${sig.name}`}
                        aria-label={`Delete ${sig.name}`}
                      >
                        <Icon name="trash" size="xs" />
                      </button>
                    </li>
                  );
                })}
              </ul>
            )
          )}
        </div>
      </div>

      {s.signatures.length > 0 && accounts.length > 0 && (
        <div className="setting-row setting-tall">
          <div className="sig-defaults">
            <span className="setting-label">Default signature per account</span>
            {accounts.map((a) => (
              <div key={a.id} className="sig-default-row">
                <span className="sig-default-acct truncate">
                  <AccountBar color={a.color} />
                  <span className="emph">{accountName(a, accounts)}</span> <span className="st-muted truncate">{a.email}</span>
                </span>
                <Select
                  className="sig-select"
                  label={`Default signature for ${a.email}`}
                  value={s.signatureDefaults[key(a.id)] ?? ""}
                  options={[{ value: "", label: "No signature" }, ...s.signatures.map((sig) => ({ value: sig.id, label: sig.name }))]}
                  onChange={(v) => setDefault(a.id, v)}
                />
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="setting-row">
        <span className="setting-label">Add to new messages</span>
        <Switch label="Add signature to new messages" on={s.signatureInsert.newMessages} onChange={(v) => setInsert({ newMessages: v })} />
      </div>
      <div className="setting-row">
        <div>
          <span className="setting-label">Add to replies</span>
          <p className="st-muted">Above the quoted message.</p>
        </div>
        <Switch label="Add signature to replies" on={s.signatureInsert.replies} onChange={(v) => setInsert({ replies: v })} />
      </div>
      <div className="setting-row">
        <span className="setting-label">Add to forwards</span>
        <Switch label="Add signature to forwards" on={s.signatureInsert.forwards} onChange={(v) => setInsert({ forwards: v })} />
      </div>
      <div className="setting-row">
        <div>
          <span className="setting-label">"-- " separator</span>
          <p className="st-muted">A line with two dashes above the signature, so mail apps can fold it.</p>
        </div>
        <Switch label="Signature separator" on={s.signatureSeparator} onChange={(v) => updateSettings({ signatureSeparator: v }).catch(fail)} />
      </div>
      {deleting && (
        <ConfirmDialog
          title={`Delete "${deleting.name}"?`}
          body={
            usersOf(deleting.id).length
              ? `${usersOf(deleting.id).map((a) => accountName(a, accounts)).join(", ")} will have no default signature.`
              : "It isn't anyone's default."
          }
          confirm="Delete"
          onConfirm={() => remove(deleting)}
          onCancel={() => setDeleting(null)}
        />
      )}
    </Section>
  );
}
