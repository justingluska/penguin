// Settings → What's new: every release's changes, newest first, from
// src/generated/changelog.json (scripts/changelog.mjs writes it from git at
// build time; dev and mock runs have none and say so). Bundled, so it works
// offline and shows exactly what this build contains.
import { Section } from "./parts";
import { shortDate } from "../../lib/format";
import { api } from "../../lib/api";
import { toast } from "../../components/Toast";
import { Icon } from "../../components/Icon";
import { debugInfo } from "../../lib/debugInfo";
import { copyTextLater } from "../../lib/clipboard";

interface Changelog {
  version: string;
  releases: { version: string; date: string; current: boolean; changes: { hash: string; text: string }[] }[];
}

// Optional at build time: an empty glob when the file wasn't generated.
const files = import.meta.glob<Changelog>("../../generated/changelog.json", { eager: true, import: "default" });
const changelog: Changelog | null = Object.values(files)[0] ?? null;

export function WhatsNewSection() {
  return (
    <Section id="whatsnew" icon="news" title="What's new">
      <div className="setting-row">
        <div>
          <span className="setting-label">Updates</span>
          <p className="st-muted">
            Penguin checks for a new version a minute after it opens and every 6 hours, and asks before restarting.
          </p>
        </div>
        <button
          className="btn btn-secondary btn-sm"
          onClick={() => api.checkForUpdates().catch((e) => toast({ tone: "error", message: String(e?.message ?? e) }))}
        >
          <Icon name="refresh" size="xs" />
          Check for updates
        </button>
      </div>
      <div className="setting-row">
        <div>
          <span className="setting-label">Debug info</span>
          <p className="st-muted">Version, accounts, sign-in clients and recent problems, for a bug report. No mail or passwords.</p>
        </div>
        <button className="btn btn-secondary btn-sm" onClick={() => void copyTextLater(debugInfo(), "Debug info copied")}>
          <Icon name="copy" size="xs" />
          Copy debug info
        </button>
      </div>
      {!changelog || changelog.releases.length === 0 ? (
        <p className="st-muted wn-empty">The changelog is added to release builds.</p>
      ) : (
        <ol className="wn-list">
          {changelog.releases.map((r) => (
            <li key={r.version + r.date} className="wn-release">
              <div className="wn-head">
                <span className="wn-version">{r.version === "local build" ? "This build" : `Penguin ${r.version}`}</span>
                {r.current && <span className="badge t-gray">Installed</span>}
                <span className="grow" />
                <span className="st-muted tnum">{shortDate(Date.parse(r.date))}</span>
              </div>
              {r.changes.length ? (
                <ul className="wn-changes">
                  {r.changes.map((c) => (
                    <li key={c.hash}>{c.text}</li>
                  ))}
                </ul>
              ) : (
                <p className="st-muted">Fixes and internal changes.</p>
              )}
            </li>
          ))}
        </ol>
      )}
    </Section>
  );
}
