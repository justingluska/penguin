// Settings → Share links (docs/SHARE-LINKS.md): the user's own S3-compatible
// storage for "Copy Share Link", a Test that tries it for real, how long
// links work, cleanup, and whether agents may share. The secret access key
// goes to the Keychain (share/config.rs) and never comes back here.
//
// Opened by "Copy Share Link…" before setup, the page shows why at the top
// and, once the storage is saved and its test passes, finishes that share.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { api, asCommandError } from "../../lib/api";
import type { ShareLinkConfig, ShareLinkConfigInput, ShareTestReport } from "../../lib/types";
import { Icon, type IconName } from "../../components/Icon";
import { toast } from "../../components/Toast";
import { Choice, ConfirmDialog, Section, Switch } from "../settings/parts";
import { openSettings } from "../settings/state";
import { LIFETIMES, SHARE_DOCS_URL, formMissing, tidyEndpoint } from "./model";
import { setPendingShare, setShareConfig, takePendingShare, useShareConfig, usePendingShare } from "./state";
import { shareNow } from "./actions";
import "./share.css";

const blankForm = (c: ShareLinkConfig | null): ShareLinkConfigInput => ({
  endpoint: c?.endpoint ?? "",
  bucket: c?.bucket ?? "",
  region: c?.region && c.region !== "auto" ? c.region : "",
  accessKeyId: c?.accessKeyId ?? "",
  secretAccessKey: "",
  lifetime: c?.lifetime ?? "24h",
  deleteOnExpiry: c?.deleteOnExpiry ?? true,
  allowAgents: c?.allowAgents ?? false,
});

/** The storage part of the form, to tell whether it differs from what's saved and which form a test passed for. */
const storageKey = (f: ShareLinkConfigInput) =>
  JSON.stringify([f.endpoint.trim(), f.bucket.trim(), f.region.trim() || "auto", f.accessKeyId.trim(), f.secretAccessKey?.trim() ?? ""]);

const STEP_LABEL: Record<ShareTestReport["steps"][number]["step"], string> = {
  upload: "Upload",
  link: "Share link",
  private: "Private bucket",
  delete: "Delete",
};
const STEP_ICON: Record<ShareTestReport["steps"][number]["status"], IconName> = { ok: "check", failed: "x", warning: "info", skipped: "minus" };

const openDocs = () =>
  void api.openExternal(SHARE_DOCS_URL).catch((e) => toast({ tone: "error", message: "Couldn't open the link", detail: asCommandError(e).message }));

export function ShareLinksSettings() {
  const config = useShareConfig();
  const pending = usePendingShare();
  const [form, setForm] = useState<ShareLinkConfigInput>(() => blankForm(config));
  const [loaded, setLoaded] = useState(config != null);
  const [busy, setBusy] = useState<"test" | "save" | "share" | null>(null);
  const [report, setReport] = useState<ShareTestReport | null>(null);
  const [passedFor, setPassedFor] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const firstField = useRef<HTMLInputElement>(null);

  // Fresh settings on open; the form starts from them.
  useEffect(() => {
    let live = true;
    api.shareLinkConfigGet().then(
      (c) => {
        if (!live) return;
        setShareConfig(c);
        setForm(blankForm(c));
        setLoaded(true);
      },
      (e) => live && setError(asCommandError(e).message),
    );
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    if (pending && loaded && !config?.configured) firstField.current?.focus();
  }, [pending, loaded, config?.configured]);

  const saved = blankForm(config);
  const dirty = storageKey(form) !== storageKey({ ...saved, secretAccessKey: "" }) || form.lifetime !== saved.lifetime || form.deleteOnExpiry !== saved.deleteOnExpiry || form.allowAgents !== saved.allowAgents;
  const missing = formMissing(form, !!config?.hasSecret);
  const edit = (patch: Partial<ShareLinkConfigInput>) => {
    setForm((f) => ({ ...f, ...patch }));
    setError(null);
  };

  const test = async (f: ShareLinkConfigInput): Promise<boolean> => {
    setBusy("test");
    setError(null);
    try {
      const r = await api.shareLinkConfigTest(f);
      setReport(r);
      setPassedFor(r.ok ? storageKey(f) : null);
      return r.ok;
    } catch (e) {
      setReport(null);
      setError(asCommandError(e).message);
      return false;
    } finally {
      setBusy(null);
    }
  };

  /** The waiting file, once storage is saved and tested. */
  const finishPending = async () => {
    const t = takePendingShare();
    if (!t) return;
    setBusy("share");
    try {
      await shareNow(t);
    } finally {
      setBusy(null);
    }
  };

  const save = async () => {
    const f = { ...form, ...tidyEndpoint(form.endpoint, form.bucket) };
    setForm(f);
    setBusy("save");
    setError(null);
    let c: ShareLinkConfig;
    try {
      c = await api.shareLinkConfigSet(f);
    } catch (e) {
      setError(asCommandError(e).message);
      setBusy(null);
      return;
    }
    setShareConfig(c);
    setBusy(null);
    // Saved: test it right away (unless this exact storage just passed).
    const ok = passedFor === storageKey(f) || (await test(f));
    setForm(blankForm(c));
    setPassedFor(ok ? storageKey({ ...blankForm(c), secretAccessKey: "" }) : null);
    if (ok && pending) await finishPending();
    else if (ok) toast({ kind: "success", message: "Share links are set up", detail: "Right-click a picture or attachment → Copy Share Link" });
  };

  /** The Test button: try the form as it is; with a file waiting and this storage saved, share it. */
  const runTest = async () => {
    const ok = await test({ ...form, ...tidyEndpoint(form.endpoint, form.bucket) });
    if (ok && pending && on && !dirty) await finishPending();
  };

  /** Lifetime, cleanup and the agent switch save at once when the storage is saved and unchanged. */
  const option = (patch: Partial<ShareLinkConfigInput>) => {
    const next = { ...form, ...patch };
    setForm(next);
    const storageSaved = config?.configured && storageKey(form) === storageKey({ ...saved, secretAccessKey: "" });
    if (!storageSaved) return;
    api.shareLinkConfigSet({ ...next, secretAccessKey: undefined }).then(setShareConfig, (e) =>
      toast({ tone: "error", message: "Couldn't save the share-link settings", detail: asCommandError(e).message }),
    );
  };

  const remove = async () => {
    setConfirmRemove(false);
    try {
      const c = await api.shareLinkConfigClear();
      setShareConfig(c);
      setForm(blankForm(c));
      setReport(null);
      setPassedFor(null);
      toast({ message: "Removed the share-link storage", detail: "The secret is gone from the Keychain." });
    } catch (e) {
      setError(asCommandError(e).message);
    }
  };

  const cancelPending = () => setPendingShare(null);
  const on = !!config?.configured;

  return (
    <Section id="sharing" icon="cloud" title="Share links" badge={<span className={"badge " + (on ? "t-green" : "t-gray")}>{on ? "On" : "Off"}</span>}>
      {pending ? (
        <div className="share-pending" role="status">
          <p>
            Share links upload a file to your own storage (for example Cloudflare R2) and copy a link that expires. Set it up once:
          </p>
          <p className="st-muted">
            When your storage is saved and its test passes, Penguin uploads <b>{pending.name}</b> and copies its link.{" "}
            <button className="st-link" onClick={openDocs}>
              How to set up storage
            </button>
          </p>
          <button className="btn btn-ghost btn-sm" onClick={cancelPending}>
            Cancel sharing
          </button>
        </div>
      ) : (
        <p className="st-muted share-intro">
          Right-click a picture or attachment → Copy Share Link: Penguin uploads the file to storage you own and copies a link that expires, ready to
          paste to an agent or a person on another machine. Cloudflare R2 or any S3-compatible storage works.{" "}
          <button className="st-link" onClick={openDocs}>
            How to set up storage
          </button>
        </p>
      )}
      <p className="share-note">
        <Icon name="shield" size="xs" />
        <span>
          Anyone who has a link can download the file until the link expires. The bucket itself stays private: without a link nobody can list it or open a
          file. Files leave this Mac only when you choose Copy Share Link.
        </span>
      </p>

      <h3 className="st-sub">Your storage</h3>
      <Field label="Endpoint" id="share-endpoint" note="For R2, the S3 API address without the bucket">
        <input
          ref={firstField}
          id="share-endpoint"
          className="input share-input mono"
          placeholder="https://<account id>.r2.cloudflarestorage.com"
          spellCheck={false}
          autoComplete="off"
          value={form.endpoint}
          onChange={(e) => edit({ endpoint: e.target.value })}
          onBlur={() => edit(tidyEndpoint(form.endpoint, form.bucket))}
        />
      </Field>
      <Field label="Bucket" id="share-bucket">
        <input id="share-bucket" className="input share-input mono" placeholder="penguin-shares" spellCheck={false} autoComplete="off" value={form.bucket} onChange={(e) => edit({ bucket: e.target.value })} />
      </Field>
      <Field label="Region" id="share-region" note="auto for R2; the bucket's region elsewhere">
        <input id="share-region" className="input share-input mono" placeholder="auto" spellCheck={false} autoComplete="off" value={form.region} onChange={(e) => edit({ region: e.target.value })} />
      </Field>
      <Field label="Access key ID" id="share-key">
        <input id="share-key" className="input share-input mono" spellCheck={false} autoComplete="off" value={form.accessKeyId} onChange={(e) => edit({ accessKeyId: e.target.value })} />
      </Field>
      <Field label="Secret access key" id="share-secret" note="Kept in the macOS Keychain, never in Penguin's settings or logs.">
        <input
          id="share-secret"
          className="input share-input mono"
          type="password"
          placeholder={config?.hasSecret ? "Saved in the Keychain" : ""}
          spellCheck={false}
          autoComplete="off"
          value={form.secretAccessKey ?? ""}
          onChange={(e) => edit({ secretAccessKey: e.target.value })}
        />
      </Field>
      {error ? <p className="st-error share-error">{error}</p> : null}
      <div className="share-actions">
        <button className="btn btn-primary btn-sm" disabled={!!missing || busy !== null || (!dirty && on && !pending)} title={missing ?? undefined} onClick={() => void save()}>
          {busy === "save" ? "Saving…" : busy === "share" ? `Sharing ${pending?.name ?? ""}…` : pending ? `Save and share ${pending.name}` : "Save"}
        </button>
        <button className="btn btn-secondary btn-sm" disabled={!!missing || busy !== null} title={missing ?? "Upload a small file, open it through a link, check the bucket is private, delete it"} onClick={() => void runTest()}>
          {busy === "test" ? "Testing…" : "Test"}
        </button>
        {on ? (
          <button className="btn btn-ghost btn-sm share-remove" disabled={busy !== null} onClick={() => setConfirmRemove(true)}>
            Remove storage
          </button>
        ) : null}
      </div>
      {report ? <TestResult report={report} /> : null}

      <h3 className="st-sub">Links</h3>
      <div className="setting-row">
        <div>
          <span className="setting-label">Link lifetime</span>
          <p className="st-muted">How long a copied link works. 7 days is the most S3-style links allow.</p>
        </div>
        <Choice label="Link lifetime" value={form.lifetime} options={LIFETIMES} onChange={(lifetime) => option({ lifetime })} />
      </div>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Delete uploads when their links expire</span>
          <p className="st-muted">Penguin deletes each file from your bucket once its link stops working, retrying later if it can't. A bucket lifecycle rule is a good backstop.</p>
        </div>
        <Switch label="Delete uploads when their links expire" on={form.deleteOnExpiry} onChange={(deleteOnExpiry) => option({ deleteOnExpiry })} />
      </div>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Let agents (CLI and MCP) create share links</span>
          <p className="st-muted">
            An agent could then upload an attachment and get a link to it. A link makes that file downloadable by anyone who has it, and text inside an email
            can try to trick an agent into sharing things. Leave this off unless you trust what your agents read. Agents also need Read, organize and draft or
            higher in{" "}
            <button className="st-link" onClick={() => openSettings("diagnostics")}>
              Settings → Developer → Agents
            </button>
            .
          </p>
        </div>
        <Switch label="Let agents (CLI and MCP) create share links" on={form.allowAgents} onChange={(allowAgents) => option({ allowAgents })} />
      </div>
      {confirmRemove ? (
        <ConfirmDialog
          title="Remove share-link storage?"
          body="Penguin forgets the endpoint, bucket and key, and deletes the secret from the Keychain. Links already copied keep working until they expire; the files stay in your bucket (a lifecycle rule removes them)."
          confirm="Remove"
          onConfirm={() => void remove()}
          onCancel={() => setConfirmRemove(false)}
        />
      ) : null}
    </Section>
  );
}

function Field({ label, id, note, children }: { label: string; id: string; note?: string; children: ReactNode }) {
  return (
    <div className="setting-row share-field">
      <div className="min0">
        <label className="setting-label" htmlFor={id}>
          {label}
        </label>
        {note ? <p className="st-muted">{note}</p> : null}
      </div>
      {children}
    </div>
  );
}

function TestResult({ report }: { report: ShareTestReport }) {
  return (
    <div className={"share-test" + (report.ok ? " is-ok" : " is-bad")} role="status">
      <div className="share-test-head">{report.ok ? "Your storage works." : "The test didn't pass."}</div>
      <ul>
        {report.steps.map((s) => (
          <li key={s.step} className={"is-" + s.status}>
            <Icon name={STEP_ICON[s.status]} size="xs" />
            <span className="share-test-step">{STEP_LABEL[s.step]}</span>
            <span className="share-test-msg">{s.message}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
