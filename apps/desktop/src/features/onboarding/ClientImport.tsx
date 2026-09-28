// "Add the client": drop, choose or paste the Desktop app client JSON.
// It's checked here first so the common mistakes (a Web application client,
// half a file, the wrong file) get a plain-words answer before the backend
// validates and saves it.
import { useEffect, useRef, useState, type DragEvent } from "react";
import { api, asCommandError } from "../../lib/api";
import type { OAuthClientStatus } from "../../lib/types";
import { Icon } from "../../components/Icon";
import type { Problem } from "./parts";

export const clientProblems: Problem[] = [
  {
    symptom: "Can't find the file",
    fix: "Look in Downloads for a file starting with client_secret_. If you closed Google's dialog without downloading, create another Desktop app client (the “Create the client” step) and download that one.",
  },
  {
    symptom: "“This is a Web application client”",
    fix: "The client was created with the wrong Application type. Create a new one with Desktop app (the “Create the client” step); you can delete the web client.",
  },
  {
    symptom: "⌘V does nothing",
    fix: "Copy the file's contents first (open it in TextEdit, ⌘A, ⌘C), or use Choose file… instead.",
  },
];

type Check = { ok: true; clientId: string; projectId: string | null } | { ok: false; message: string };

/** Friendly pre-check of a client JSON; the backend has the final say. */
export function checkClientJson(text: string): Check {
  const t = text.trim();
  if (!t) return { ok: false, message: "That's empty. Use the JSON file you downloaded from Google Cloud." };
  let parsed: unknown;
  try {
    parsed = JSON.parse(t);
  } catch {
    return {
      ok: false,
      message: t.startsWith("{")
        ? "That JSON is incomplete or damaged. Choose the downloaded file itself instead of copying part of it."
        : "That isn't a client JSON. Use the client_secret_….json file you downloaded in the “Create the client” step.",
    };
  }
  const obj = (parsed && typeof parsed === "object" ? parsed : {}) as Record<string, unknown>;
  if ("web" in obj && !("installed" in obj)) {
    return {
      ok: false,
      message: 'This is a "Web application" client. Penguin needs a "Desktop app" client: go back to the “Create the client” step, create one with Application type → Desktop app, and use its JSON.',
    };
  }
  if ("type" in obj && obj.type === "service_account") {
    return { ok: false, message: "This is a service account key, not an OAuth client. Create a Desktop app client in the “Create the client” step instead." };
  }
  const inst = obj.installed as { client_id?: unknown; project_id?: unknown } | undefined;
  if (!inst || typeof inst !== "object") {
    return { ok: false, message: 'This JSON has no "installed" section, so it isn\'t a Desktop app client. Download the JSON of the client you created in the “Create the client” step.' };
  }
  const id = typeof inst.client_id === "string" ? inst.client_id.trim() : "";
  if (!id) return { ok: false, message: "This client JSON has no client_id. Download it again from Google Cloud." };
  // Google includes the project in the download; later links use it.
  return { ok: true, clientId: id, projectId: typeof inst.project_id === "string" ? inst.project_id : null };
}

export function ClientImport({
  status,
  onStatus,
  onProjectId,
}: {
  status: OAuthClientStatus;
  onStatus: (s: OAuthClientStatus) => void;
  /** The project_id from the client JSON, when it has one. */
  onProjectId?: (id: string) => void;
}) {
  const [replacing, setReplacing] = useState(false);
  const [pasting, setPasting] = useState(false);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [clientId, setClientId] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const pasteRef = useRef<HTMLTextAreaElement>(null);
  const configured = status.configured && !replacing;

  async function submit(json: string) {
    const check = checkClientJson(json);
    if (!check.ok) {
      setError(check.message);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const st = await api.setOauthClient(json.trim());
      if (!st.configured) {
        setError("Penguin couldn't use that client. Check that it's a Desktop app client.");
        return;
      }
      setClientId(check.clientId);
      setReplacing(false);
      setPasting(false);
      setText("");
      onStatus(st);
      if (check.projectId) onProjectId?.(check.projectId);
    } catch (e) {
      const msg = asCommandError(e).message;
      setError(msg.charAt(0).toUpperCase() + msg.slice(1));
    } finally {
      setBusy(false);
    }
  }

  function readFile(f: File | undefined) {
    if (!f) return;
    if (f.size > 64 * 1024) {
      setError(`${f.name} is too large to be a client JSON. Use the client_secret_….json file from the “Create the client” step.`);
      return;
    }
    const r = new FileReader();
    r.onload = () => void submit(String(r.result ?? ""));
    r.onerror = () => setError(`Couldn't read ${f.name}.`);
    r.readAsText(f);
  }

  // ⌘V anywhere on this step pastes the JSON directly.
  useEffect(() => {
    if (configured) return;
    const onPaste = (e: ClipboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "TEXTAREA" || t.tagName === "INPUT")) return;
      const data = e.clipboardData?.getData("text") ?? "";
      if (!data.trim()) return;
      e.preventDefault();
      void submit(data);
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  });

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setDragging(false);
    readFile(e.dataTransfer.files?.[0]);
  };

  if (configured) {
    return (
      <div className="onb-client-done" role="status">
        <span className="onb-client-ok">
          <Icon name="check" size="sm" />
        </span>
        <div className="grow min0">
          <div className="onb-client-title">Client added</div>
          {clientId && (
            <div className="mono truncate onb-client-id" title={clientId}>
              {clientId}
            </div>
          )}
          <div className="mono truncate onb-client-path" title={status.path}>
            Saved to {status.path}
          </div>
        </div>
        <button className="btn btn-ghost btn-sm" onClick={() => setReplacing(true)}>
          Replace
        </button>
      </div>
    );
  }

  return (
    <>
      <div
        className={`oauth-drop onb-drop${dragging ? " dragging" : ""}`}
        role="group"
        aria-label="Add the OAuth client JSON"
        onDragOver={(e) => {
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={onDrop}
      >
        <span className="onb-drop-icon">
          <Icon name="download" size="md" />
        </span>
        <span className="s2-text">
          Drop <span className="mono">client_secret_….json</span> here
        </span>
        <span className="s2-muted">It's in your Downloads folder.</span>
        <div className="s2-action-row">
          <button className="btn btn-secondary" onClick={() => fileRef.current?.click()} disabled={busy}>
            <Icon name="file" size="sm" />
            Choose file…
          </button>
          <button
            className="btn btn-ghost"
            onClick={() => {
              setPasting(true);
              requestAnimationFrame(() => pasteRef.current?.focus());
            }}
            disabled={busy}
          >
            Paste JSON <span className="kbd">⌘V</span>
          </button>
          {replacing && (
            <button className="btn btn-ghost" onClick={() => setReplacing(false)}>
              Cancel
            </button>
          )}
        </div>
        <input
          ref={fileRef}
          type="file"
          accept=".json,application/json"
          hidden
          onChange={(e) => {
            readFile(e.target.files?.[0]);
            e.target.value = "";
          }}
        />
        {pasting && (
          <>
            <textarea
              ref={pasteRef}
              className="oauth-paste"
              value={text}
              onChange={(e) => {
                setText(e.target.value);
                setError(null);
              }}
              onKeyDown={(e) => {
                if ((e.metaKey || e.ctrlKey) && e.key === "Enter") void submit(text);
              }}
              placeholder='{"installed":{"client_id":"….apps.googleusercontent.com", …}}'
              spellCheck={false}
              aria-label="OAuth client JSON"
            />
            <div className="s2-action-row" style={{ alignSelf: "flex-end" }}>
              <button className="btn btn-primary btn-sm" onClick={() => void submit(text)} disabled={busy || !text.trim()}>
                {busy ? "Saving…" : "Add client"}
                <span className="kbd">⌘↵</span>
              </button>
            </div>
          </>
        )}
      </div>
      {error && (
        <div className="onb-error" role="alert">
          <Icon name="info" size="xs" />
          <span>{error}</span>
        </div>
      )}
    </>
  );
}
