// Settings → Developer → "Gmail quota (units per minute per account)".
// Persisted as settings.gmailUnitsPerMin and applied live to the sync's quota
// limiter. OWNER: settings agent.
import { useEffect, useState } from "react";
import { asCommandError } from "../../lib/api";
import { updateSettings, useSetting } from "../../lib/settings";
import type { Diagnostics } from "../../lib/types";
import { num } from "../../lib/format";
import { toast } from "../../components/Toast";

const MIN = 600;
const MAX = 1_000_000;

export function GmailQuotaField({ diag }: { diag: Diagnostics | null }) {
  const saved = useSetting("gmailUnitsPerMin");
  const [draft, setDraft] = useState(String(saved));
  useEffect(() => setDraft(String(saved)), [saved]);

  const value = Number(draft.replace(/[,\s_]/g, ""));
  const valid = Number.isInteger(value) && value >= MIN && value <= MAX;
  const dirty = valid && value !== saved;

  const commit = () => {
    if (!valid) {
      setDraft(String(saved));
      return;
    }
    if (!dirty) return;
    updateSettings({ gmailUnitsPerMin: value }).then(
      () => toast({ message: `Gmail quota set to ${num(value)} units/min per account` }),
      (e) => toast({ tone: "error", message: `Couldn't save: ${asCommandError(e).message}` }),
    );
  };

  const envOverride = diag?.gmailUnitsEnvOverride ?? null;

  return (
    <div className="setting-row setting-tall st-quota-field">
      <div>
        <label className="setting-label" htmlFor="st-gmail-units">
          Gmail quota (units per minute per account)
        </label>
        <p className="st-muted">Raise this only after Google approves a quota increase for your Cloud project.</p>
        {envOverride != null ? (
          <p className="st-muted is-warn">
            Sync is using {num(envOverride)} units/min: PENGUIN_GMAIL_UNITS_PER_MIN overrides this setting.
          </p>
        ) : null}
        {!valid ? <p className="st-error">Enter a whole number from {num(MIN)} to {num(MAX)}.</p> : null}
      </div>
      <input
        id="st-gmail-units"
        className="st-number tnum"
        inputMode="numeric"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
        }}
        aria-invalid={!valid}
      />
    </div>
  );
}
