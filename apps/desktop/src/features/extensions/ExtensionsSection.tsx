// Settings → Extensions: what's in the extensions folder, each with an Allow
// switch, exactly what its actions run and what they receive, and the
// folders that aren't valid extensions. OWNER: extensions.
import { useCallback, useEffect, useState } from "react";
import type { ExtensionInfo } from "../../lib/types";
import { api, asCommandError } from "../../lib/api";
import { Icon } from "../../components/Icon";
import { toast } from "../../components/Toast";
import { ConfirmDialog, Section, Switch } from "../settings/parts";
import { ExtensionIcon } from "./ExtensionIcon";
import { loadExtensions, setExtensions, useExtensions } from "./state";
import "./extensions.css";

export function ExtensionsSection() {
  const data = useExtensions();
  const [asking, setAsking] = useState<ExtensionInfo | null>(null);
  const reload = useCallback(() => void loadExtensions(), []);
  useEffect(reload, [reload]);
  const allow = (e: ExtensionInfo, on: boolean) =>
    api.setExtensionAllowed(e.id, on).then(setExtensions, (err) => toast({ kind: "error", message: `Couldn't save: ${asCommandError(err).message}` }));

  return (
    <Section id="extensions" icon="zap" title="Extensions">
      <p className="st-muted">
        Extensions add buttons to your emails that run a program on this Mac: make a task from an email, save it somewhere, anything a
        script can do. Each one is a folder with an <span className="mono">extension.json</span> in the extensions folder. Nothing runs
        until you allow it here, and editing an extension's files turns it off until you allow it again.
      </p>
      <div className="row-flex" style={{ gap: 8, margin: "10px 0 16px" }}>
        <button type="button" className="btn btn-secondary btn-sm" onClick={() => void api.openExtensionsFolder().catch((e) => toast({ kind: "error", message: asCommandError(e).message }))}>
          <Icon name="folder" size="xs" />
          Open extensions folder
        </button>
        <button type="button" className="btn btn-ghost btn-sm" onClick={reload}>
          <Icon name="refresh" size="xs" />
          Reload
        </button>
      </div>

      {data && data.extensions.length === 0 && data.problems.length === 0 && (
        <p className="st-muted ext-empty">No extensions yet. Put an extension's folder in {data.dir} and it shows up here.</p>
      )}
      <div className="ext-list">
        {data?.extensions.map((e) => (
          <div key={e.id} className="ext-card">
            <div className="ext-card-head">
              <ExtensionIcon icon={e.icon} svg={e.iconSvg} size="sm" />
              <span className="ext-card-name grow truncate">{e.name}</span>
              <Switch label={`Allow ${e.name}`} on={e.state === "on"} onChange={(v) => (v ? setAsking(e) : void allow(e, false))} />
            </div>
            {e.description && <p className="st-muted ext-card-desc">{e.description}</p>}
            {e.state === "changed" && <p className="ext-changed">Its files changed since you allowed it, so it's off. Check what changed, then allow it again.</p>}
            <div className="ext-card-actions">
              {e.actions.map((a) => (
                <div key={a.id} className="ext-card-action">
                  <ExtensionIcon icon={a.icon} svg={a.iconSvg} size="2xs" />
                  <span>{a.title}:</span>
                  <span className="mono">{a.command.join(" ")}</span>
                  {a.includeThread ? (
                    <span>· receives the whole conversation, with its text</span>
                  ) : null}
                  {a.includeBody && <span>· gets the email's text</span>}
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>
      {data && data.problems.length > 0 && (
        <>
          <h3 className="st-sub">Not loaded</h3>
          {data.problems.map((p) => (
            <p key={p.folder} className="ext-problem">
              <span className="mono">{p.folder}</span>: {p.error}
            </p>
          ))}
        </>
      )}
      {asking && (
        <ConfirmDialog
          title={`Allow ${asking.name}?`}
          body={`Its buttons run ${[...new Set(asking.actions.map((a) => a.command.join(" ")))].join(", ")} on this Mac, as you, and send it the email's details${
            asking.actions.some((a) => a.includeThread)
              ? " and the whole conversation, with its text,"
              : asking.actions.some((a) => a.includeBody)
                ? " including its text"
                : ""
          } each time you click one. Only allow extensions you wrote or trust.`}
          confirm="Allow"
          onCancel={() => setAsking(null)}
          onConfirm={() => {
            const e = asking;
            setAsking(null);
            void allow(e, true);
          }}
        />
      )}
    </Section>
  );
}
