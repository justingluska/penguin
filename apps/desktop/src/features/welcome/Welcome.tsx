// "Welcome to Penguin": a short setup shown once on a new install, right
// after the first account is added, while mail downloads behind it. Five or
// six short screens (Apple Intelligence only when this Mac has it), one
// topic each, every answer preselected with the default and applied as soon
// as it's picked. ↵ next, ⇧↵ back, Esc or "Use defaults" ends it with
// whatever is chosen so far. Reopen: ⌘K "Set up Penguin…" or Settings →
// General. Why this shape, and the sources: docs/ONBOARDING.md.
//
// The search model (≈217 MB) isn't downloaded on a new install until this
// has asked: leaving the first screen with search by meaning on starts it
// (start_model_download), and finishing or skipping allows it too
// (welcomeCompleted, src-tauri/src/semantic).
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { api, asCommandError } from "../../lib/api";
import { currentSettings, updateSettings, useSettings } from "../../lib/settings";
import { DARK_SHADES, SIDEBAR_THEMES } from "../../lib/themes";
import { LIST_STYLES } from "../../lib/listStyle";
import { SYNC_WINDOW_CHOICES, type InboxSplit, type SemanticIndexStatus, type SettingsPatch, type SyncWindowMonths } from "../../lib/types";
import { bytes } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { PenguinMark } from "../../components/PenguinMark";
import { toast } from "../../components/Toast";
import { meta } from "../../app/store";
import { refreshAvailability, useAvailability } from "../summary/state";
import { SPLIT_PRESETS, splitFromPreset } from "../split/presets";
import { SYSTEM_SETTINGS_HELP } from "../notifications/rules";
import { closeWelcome } from "./state";
import { startsDownload, welcomeScreens, type WelcomeScreen } from "./model";
import "./welcome.css";

function save(patch: SettingsPatch) {
  return updateSettings(patch).catch((e) => toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` }));
}

const FOCUSABLE = 'button:not([disabled]), a[href], input:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function Welcome() {
  const availability = useAvailability();
  const aiAvailable = availability === null ? null : availability.available;
  const screens = welcomeScreens(aiAvailable);
  const [id, setId] = useState<WelcomeScreen>("search");
  const idx = Math.max(0, screens.findIndex((s) => s.id === id));
  const last = idx === screens.length - 1;
  const boxRef = useRef<HTMLDivElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  /** The Keys screen's "try it" takes J/K while it's showing. */
  const tryKey = useRef<((key: string) => boolean) | null>(null);
  const status = useSemanticStatus();

  useEffect(() => void refreshAvailability(true), []);

  // Each screen starts with focus on its heading, so ↵ means Next.
  useLayoutEffect(() => {
    headingRef.current?.focus({ preventScroll: true });
  }, [id]);

  const finish = () => {
    void save({ welcomeCompleted: true });
    closeWelcome();
  };
  const go = (to: number) => {
    const next = screens[Math.max(0, Math.min(screens.length - 1, to))];
    if (next) setId(next.id);
  };
  const forward = () => {
    if (startsDownload(id, currentSettings().semanticSearch)) void api.startModelDownload().catch(() => {});
    if (last) finish();
    else go(idx + 1);
  };

  const keys = useRef({ forward, back: () => go(idx - 1), finish });
  keys.current = { forward, back: () => go(idx - 1), finish };
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.isComposing) return;
      // Nothing reaches the app's single-key shortcuts while this is up.
      e.stopPropagation();
      const t = e.target as HTMLElement | null;
      const onControl = !!t?.closest("button, a, input, textarea, select");
      if (e.key === "Escape") {
        e.preventDefault();
        keys.current.finish();
      } else if (e.key === "Enter" && !e.altKey && (!onControl || e.shiftKey)) {
        e.preventDefault();
        if (e.shiftKey) keys.current.back();
        else keys.current.forward();
      } else if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && t?.closest("[role=radiogroup]")) {
        // Arrows move the choice inside a group of options, as native radios do.
        const group = t.closest("[role=radiogroup]")!;
        const radios = [...group.querySelectorAll<HTMLButtonElement>("[role=radio]:not([disabled])")];
        const i = radios.indexOf(t.closest("[role=radio]") as HTMLButtonElement);
        const next = radios[(i + (e.key === "ArrowRight" ? 1 : -1) + radios.length) % radios.length];
        if (next) {
          e.preventDefault();
          next.focus();
          next.click();
        }
      } else if (!e.altKey && tryKey.current?.(e.key)) {
        e.preventDefault();
      } else if (e.key === "Tab") {
        const box = boxRef.current;
        const items = box ? [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null) : [];
        if (!items.length) return;
        const first = items[0];
        const lastEl = items[items.length - 1];
        const active = document.activeElement;
        if (e.shiftKey && (active === first || !box?.contains(active))) {
          e.preventDefault();
          lastEl.focus();
        } else if (!e.shiftKey && (active === lastEl || !box?.contains(active))) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    // Capture phase: ahead of the app's keyboard layer.
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      if (before && document.contains(before)) before.focus({ preventScroll: true });
    };
  }, []);

  return (
    <div className="wl-scrim">
      <div className="wl keys-always" ref={boxRef} role="dialog" aria-modal="true" aria-labelledby="wl-title">
        <header className="wl-head">
          <PenguinMark className="wl-logo" />
          <div className="wl-head-text">
            <span className="wl-kicker">Welcome to Penguin</span>
            <span className="wl-sub">A few choices while your mail downloads. Each one can be changed later in Settings.</span>
          </div>
        </header>
        <nav className="wl-steps" aria-label="Setup steps">
          {screens.map((s, i) => (
            <button
              key={s.id}
              className={"wl-step" + (i === idx ? " is-current" : "") + (i < idx ? " is-done" : "")}
              aria-current={i === idx ? "step" : undefined}
              tabIndex={-1}
              onClick={() => go(i)}
            >
              <span className="wl-step-mark">{i < idx ? <Icon name="check" size="2xs" /> : i + 1}</span>
              {s.label}
            </button>
          ))}
        </nav>

        <main className="wl-body" key={id}>
          {id === "search" && <SearchScreen headingRef={headingRef} status={status} />}
          {id === "ai" && <AiScreen headingRef={headingRef} />}
          {id === "mail" && <MailScreen headingRef={headingRef} />}
          {id === "inbox" && <InboxScreen headingRef={headingRef} />}
          {id === "look" && <LookScreen headingRef={headingRef} />}
          {id === "keys" && <KeysScreen headingRef={headingRef} tryKey={tryKey} />}
        </main>

        <footer className="wl-foot">
          <button className="btn btn-ghost" onClick={finish} title="Keep the defaults for everything not chosen yet">
            Use defaults
            <span className="kbd">esc</span>
          </button>
          <DownloadLine status={status} />
          <span className="grow" />
          <button className="btn btn-ghost" onClick={() => go(idx - 1)} disabled={idx === 0}>
            Back
            <span className="kbd">⇧</span>
            <span className="kbd">↵</span>
          </button>
          <button className="btn btn-primary" onClick={forward}>
            {last ? "Start using Penguin" : "Next"}
            <span className="kbd">↵</span>
          </button>
        </footer>
      </div>
    </div>
  );
}

/** semantic_status while the setup is open (local and instant; every second for the download line). */
function useSemanticStatus(): SemanticIndexStatus | null {
  const [status, setStatus] = useState<SemanticIndexStatus | null>(null);
  useEffect(() => {
    let alive = true;
    const tick = () => api.semanticStatus().then((s) => alive && setStatus(s), () => {});
    void tick();
    const t = window.setInterval(tick, 1000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, []);
  return status;
}

function DownloadLine({ status }: { status: SemanticIndexStatus | null }) {
  if (!status || status.state !== "downloading") return <span className="wl-dl" aria-live="polite" />;
  const pct = status.downloadTotal > 0 ? Math.floor((status.downloadDone / status.downloadTotal) * 100) : 0;
  return (
    <span className="wl-dl tnum" aria-live="polite">
      <Icon name="download" size="xs" />
      Search model {bytes(status.downloadDone)} of {bytes(status.downloadTotal)}
      <span className="wl-dl-bar" aria-hidden="true">
        <span style={{ width: `${pct}%` }} />
      </span>
    </span>
  );
}

// ---------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------
type HeadingRef = React.RefObject<HTMLHeadingElement | null>;

function Screen({ headingRef, title, lead, children }: { headingRef: HeadingRef; title: string; lead: ReactNode; children: ReactNode }) {
  return (
    <>
      <h1 id="wl-title" className="wl-title" ref={headingRef} tabIndex={-1}>
        {title}
      </h1>
      <p className="wl-lead">{lead}</p>
      <div className="wl-content">{children}</div>
    </>
  );
}

/** A big either/or with a line under each option. */
function Options<T extends string>({
  label,
  value,
  options,
  onChange,
  className,
}: {
  label: string;
  value: T;
  options: { value: T; title: string; note?: ReactNode; preview?: ReactNode; tag?: string }[];
  onChange: (v: T) => void;
  className?: string;
}) {
  return (
    <div className={"wl-options" + (className ? " " + className : "")} role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <button
          key={o.value}
          role="radio"
          aria-checked={value === o.value}
          tabIndex={value === o.value ? 0 : -1}
          className={"wl-option" + (value === o.value ? " selected" : "")}
          onClick={() => onChange(o.value)}
        >
          {o.preview}
          <span className="wl-option-title">
            <span className="wl-radio" aria-hidden="true" />
            {o.title}
            {o.tag && <span className="wl-tag">{o.tag}</span>}
          </span>
          {o.note && <span className="wl-option-note">{o.note}</span>}
        </button>
      ))}
    </div>
  );
}

function Toggle({ label, note, on, onChange, disabled, children }: { label: string; note: ReactNode; on: boolean; onChange: (v: boolean) => void; disabled?: boolean; children?: ReactNode }) {
  return (
    <div className="wl-row">
      <div className="wl-row-text">
        <span className="wl-row-label">{label}</span>
        <span className="wl-row-note">{note}</span>
        {children}
      </div>
      <button className="wl-switch" role="switch" aria-checked={on} aria-label={label} disabled={disabled} onClick={() => onChange(!on)}>
        <span className={"toggle" + (on ? " on" : "")}>
          <span />
        </span>
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------
function SearchScreen({ headingRef, status }: { headingRef: HeadingRef; status: SemanticIndexStatus | null }) {
  const s = useSettings();
  const model = status?.model ?? "EmbeddingGemma 300M";
  const license = status?.modelLicense ?? "Gemma Terms of Use";
  return (
    <Screen
      headingRef={headingRef}
      title="Search by meaning"
      lead="Finds mail by what it's about, not just its words: “rent going up” finds the lease renewal that never says rent."
    >
      <Options
        label="Search by meaning"
        value={s.semanticSearch ? "on" : "off"}
        onChange={(v) => void save({ semanticSearch: v === "on" })}
        options={[
          {
            value: "on",
            title: "On",
            tag: "Recommended",
            note: "Runs on this Mac: a one-time ~217 MB download of Google's EmbeddingGemma model, then indexing in the background (paused on battery saver and when the Mac is hot). No mail or search leaves your computer.",
          },
          { value: "off", title: "Off", note: "Search finds the words you type. Nothing is downloaded. You can turn it on later in Settings → Search." },
        ]}
      />
      <p className="wl-fine">
        Model: {model} by Google, downloaded from Hugging Face and provided under the {license}.{" "}
        {s.semanticSearch ? "The download starts when you continue." : ""}
      </p>
    </Screen>
  );
}

function AiScreen({ headingRef }: { headingRef: HeadingRef }) {
  const s = useSettings();
  const instant = s.instantReplies;
  return (
    <Screen
      headingRef={headingRef}
      title="Apple Intelligence"
      lead="These use Apple's on-device model: no account, no API key, and nothing leaves your Mac. Each runs only when you ask."
    >
      <div className="wl-rows">
        <Toggle label="Thread summaries" note="Summarize a long conversation with ⇧S." on={s.summaries} onChange={(summaries) => void save({ summaries })} />
        <Toggle label="Write with AI" note="Draft from a few words, or make a selection shorter or friendlier (⌘⇧J)." on={s.writeWithAi} onChange={(writeWithAi) => void save({ writeWithAi })} />
        <Toggle
          label="Suggested replies"
          note="Three short replies offered when you reply. It reads the conversation when the reply opens."
          on={instant.aiSuggestions && instant.enabled}
          disabled={!instant.enabled}
          onChange={(aiSuggestions) => void save({ instantReplies: { ...instant, aiSuggestions } })}
        />
        <Toggle label="Ask reads your question" note="When Ask can't read a question itself, the model turns it into a search. Penguin counts the answer from your mail." on={s.askWithAi} onChange={(askWithAi) => void save({ askWithAi })} />
      </div>
    </Screen>
  );
}

function windowName(m: SyncWindowMonths): string {
  if (m === 0) return "Everything";
  if (m === 1) return "1 month";
  if (m === 12) return "1 year";
  if (m === 24) return "2 years";
  return `${m} months`;
}

function MailScreen({ headingRef }: { headingRef: HeadingRef }) {
  const s = useSettings();
  return (
    <Screen
      headingRef={headingRef}
      title="How much mail to keep on this Mac"
      lead="Mail from this period is downloaded in full, so every word is searchable offline. Older mail keeps its sender and subject, and its body downloads when you open it."
    >
      <Options
        label="Mail downloaded in full"
        className="wl-options-3"
        value={String(s.syncWindowMonths)}
        onChange={(v) => void save({ syncWindowMonths: Number(v) as SyncWindowMonths })}
        options={SYNC_WINDOW_CHOICES.map((m) => ({ value: String(m), title: windowName(m), tag: m === 6 ? "Default" : undefined }))}
      />
      <div className="wl-tradeoff">
        <span>
          <Icon name="database" size="xs" /> Shorter: less disk space and a quicker first download.
        </span>
        <span>
          <Icon name="search" size="xs" /> Longer: full-text search reaches further back, offline.
        </span>
      </div>
      <p className="wl-fine">Shrinking it later never deletes anything; Settings → Sync can free up space.</p>
    </Screen>
  );
}

/** The splits a new Split Inbox starts with, by preset keys. */
const SPLIT_SETS: { value: string; title: string; keys: string[] }[] = [
  { value: "standard", title: "Important · Calendar · News", keys: ["important", "calendar", "news"] },
  { value: "people", title: "People · Notifications · News", keys: ["people", "tools", "news"] },
];

function splitSetOf(splits: InboxSplit[]): string | null {
  const ids = splits.map((x) => x.id).join(",");
  return SPLIT_SETS.find((set) => set.keys.join(",") === ids)?.value ?? null;
}

function InboxScreen({ headingRef }: { headingRef: HeadingRef }) {
  const s = useSettings();
  const accounts = meta.use((m) => m.accounts);
  const set = splitSetOf(s.inboxSplits);
  const pickSet = (value: string) => {
    const keys = SPLIT_SETS.find((x) => x.value === value)?.keys ?? [];
    const mine = accounts.map((a) => a.email);
    const splits = keys
      .map((k) => SPLIT_PRESETS.find((p) => p.key === k))
      .map((p) => (p ? splitFromPreset(p, p.key, mine) : null))
      .filter((x): x is InboxSplit => !!x);
    void save({ inboxSplits: splits });
  };
  return (
    <Screen headingRef={headingRef} title="Your inbox" lead="How the list looks, and whether mail is sorted into tabs.">
      <span className="wl-group-label">List style</span>
      <Options
        label="List style"
        className="wl-options-grid"
        value={s.listStyle}
        onChange={(listStyle) => void save({ listStyle })}
        options={LIST_STYLES.map((st) => ({
          value: st.id,
          title: st.name,
          tag: st.id === "quiet" ? "Default" : undefined,
          preview: <MiniList style={st.id} />,
          note: st.blurb,
        }))}
      />
      <div className="wl-rows">
        <Toggle
          label="Split Inbox"
          note="Tabs above the inbox; each conversation goes to the first tab that matches, the rest to Other. Tab moves between them."
          on={s.inboxTabs}
          onChange={(inboxTabs) => void save({ inboxTabs })}
        >
          {s.inboxTabs && (
            <Options
              label="Split Inbox tabs"
              className="wl-options-row wl-options-sm"
              value={set ?? "custom"}
              onChange={pickSet}
              options={[
                ...SPLIT_SETS.map((x) => ({ value: x.value, title: x.title })),
                ...(set ? [] : [{ value: "custom", title: "Your own tabs" }]),
              ]}
            />
          )}
        </Toggle>
        <Toggle
          label="Start in Floe mode"
          note="One column at a time: the list, then the conversation, with nothing beside it. Good for small screens and focus. ⌘⇧F switches any time."
          on={s.floeMode}
          onChange={(floeMode) => void save({ floeMode })}
        />
      </div>
    </Screen>
  );
}

/** A three-row stand-in for the conversation list in one list style (welcome.css). */
function MiniList({ style }: { style: string }) {
  return (
    <span className="wl-mini" data-mini={style} aria-hidden="true">
      {[true, false, true].map((unread, i) => (
        <span key={i} className={"wl-mini-row" + (unread ? " unread" : "")}>
          <span className={`wl-mini-face f${i}`} />
          <span className="wl-mini-lines">
            <i />
            <i />
          </span>
        </span>
      ))}
    </span>
  );
}

function LookScreen({ headingRef }: { headingRef: HeadingRef }) {
  const s = useSettings();
  return (
    <Screen headingRef={headingRef} title="Look" lead="Penguin follows your Mac's light or dark mode unless you pick one. T switches for the moment.">
      <Options
        label="Theme"
        className="wl-options-row"
        value={s.theme}
        onChange={(theme) => void save({ theme })}
        options={[
          { value: "system", title: "System", tag: "Default", preview: <span className="wl-theme wl-theme-system" aria-hidden="true" /> },
          { value: "light", title: "Light", preview: <span className="wl-theme wl-theme-light" aria-hidden="true" /> },
          { value: "dark", title: "Dark", preview: <span className="wl-theme wl-theme-dark" aria-hidden="true" /> },
        ]}
      />
      {s.theme !== "light" && (
        <>
          <span className="wl-group-label">Dark mode shade</span>
          <div className="sh-grid wl-shades" role="radiogroup" aria-label="Dark mode shade">
            {DARK_SHADES.map((d) => {
              const on = (s.darkShade ?? "black") === d.id;
              return (
                <button
                  key={d.id}
                  role="radio"
                  aria-checked={on}
                  tabIndex={on ? 0 : -1}
                  className={"th-swatch" + (on ? " selected" : "")}
                  onClick={() => void save({ darkShade: d.id })}
                >
                  <span className="sh-preview" data-theme="dark" data-dark-shade={d.id} aria-hidden="true">
                    <span className="sh-side">
                      <i />
                      <i className="on" />
                      <i />
                    </span>
                    <span className="sh-list">
                      <i className="unread" />
                      <i className="sel" />
                      <i />
                    </span>
                    <span className="sh-pane">
                      <b />
                      <i />
                      <i />
                    </span>
                  </span>
                  <span className="th-name">{d.name}</span>
                </button>
              );
            })}
          </div>
        </>
      )}
      <span className="wl-group-label">Sidebar</span>
      <div className="th-grid wl-swatches" role="radiogroup" aria-label="Sidebar theme">
        {SIDEBAR_THEMES.map((t) => {
          const on = s.sidebarTheme === t.id;
          return (
            <button
              key={t.id}
              role="radio"
              aria-checked={on}
              tabIndex={on ? 0 : -1}
              data-sidebar-theme={t.id}
              className={"th-swatch" + (on ? " selected" : "")}
              onClick={() => void save({ sidebarTheme: t.id })}
            >
              <span className="th-preview" aria-hidden="true">
                <span className="th-side">
                  <i />
                  <i className="on" />
                  <i />
                </span>
                <span className="th-list">
                  <i className="unread" />
                  <i />
                  <i className="unread" />
                </span>
              </span>
              <span className="th-name">
                <span className="th-dot" />
                {t.name}
              </span>
            </button>
          );
        })}
      </div>
      <span className="wl-group-label">Sidebar text size</span>
      <Options
        label="Sidebar text size"
        className="wl-options-row wl-options-sm"
        value={String(s.sidebarTextSize ?? 0)}
        onChange={(v) => void save({ sidebarTextSize: Number(v) })}
        options={[
          { value: "-2", title: "Smallest" },
          { value: "-1", title: "Smaller" },
          { value: "0", title: "Default" },
          { value: "1", title: "Larger" },
          { value: "2", title: "Largest" },
        ]}
      />
    </Screen>
  );
}

const TRY_ROWS = ["Priya Raman · Launch checklist", "Northwind billing · Your receipt", "Leo Park · Saturday?", "Harbor Weekly · This week"];

function KeysScreen({ headingRef, tryKey }: { headingRef: HeadingRef; tryKey: React.RefObject<((key: string) => boolean) | null> }) {
  const s = useSettings();
  const [row, setRow] = useState(0);
  const [pressed, setPressed] = useState<string | null>(null);
  const [permission, setPermission] = useState<string | null>(null);
  useEffect(() => {
    tryKey.current = (key) => {
      const k = key.toLowerCase();
      if (k !== "j" && k !== "k") return false;
      setRow((r) => Math.max(0, Math.min(TRY_ROWS.length - 1, r + (k === "j" ? 1 : -1))));
      setPressed(k.toUpperCase());
      return true;
    };
    return () => {
      tryKey.current = null;
    };
  }, [tryKey]);

  const notify = async (on: boolean) => {
    await save({ notifications: { ...s.notifications, enabled: on } });
    if (!on) return;
    // Asked only now, because they turned it on (Apple HIG: ask in context).
    try {
      setPermission(await api.requestNotificationPermission());
    } catch (e) {
      toast({ tone: "error", message: `Couldn't ask for permission: ${asCommandError(e).message}` });
    }
  };

  return (
    <Screen headingRef={headingRef} title="Keys and notifications" lead="Everything in Penguin has a key. ⌘K finds any command, and ? shows them all.">
      <div className="wl-try" aria-live="polite">
        <div className="wl-try-list">
          {TRY_ROWS.map((r, i) => (
            <div key={r} className={"wl-try-row" + (i === row ? " is-selected" : "")}>
              {r}
            </div>
          ))}
        </div>
        <p className="wl-try-note">
          {pressed ? (
            <>
              <span className="kbd">{pressed}</span> {pressed === "J" ? "moves down" : "moves up"}. In your inbox, <span className="kbd">E</span> archives and{" "}
              <span className="kbd">↵</span> opens.
            </>
          ) : (
            <>
              Try it: press <span className="kbd">J</span> and <span className="kbd">K</span>.
            </>
          )}
        </p>
      </div>
      <div className="wl-rows">
        <Toggle
          label="Show key hints"
          note="Key caps on buttons, rows and menus. Off keeps things quiet; the keys work either way."
          on={s.showShortcutHints}
          onChange={(showShortcutHints) => void save({ showShortcutHints })}
        />
        <Toggle
          label="Shortcut coach"
          note="Click something that has a key, and the key shows for a moment. Keys you already use are never shown again."
          on={s.shortcutCoach}
          onChange={(shortcutCoach) => void save({ shortcutCoach })}
        />
        <Toggle
          label="Notify me about new mail"
          note={
            permission === "denied"
              ? `Notifications are turned off for Penguin. To see them, open ${SYSTEM_SETTINGS_HELP}.`
              : "Sender and subject only, never the message. macOS asks for permission when you turn this on."
          }
          on={s.notifications.enabled}
          onChange={(on) => void notify(on)}
        />
      </div>
    </Screen>
  );
}
