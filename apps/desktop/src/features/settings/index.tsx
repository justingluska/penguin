// Settings: a large modal over the mail UI (which stays mounted underneath).
// Left nav of pages with a search field on top; one page shows at a time
// (only its section is mounted). Esc / backdrop / ✕ to close, focus trapped
// inside. Every control writes through lib/settings.ts, which persists to
// settings.json and broadcasts penguin://settings-changed.
// OWNER: settings agent.
import { SmartViewsSection } from "../smart/SmartSettings";
import { InboxSection } from "../split/SplitSettings";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type RefObject } from "react";
import { api, asCommandError } from "../../lib/api";
import { setTrustedImageSender, updateSettings, useSetting, useSettings } from "../../lib/settings";
import { SummarySettings } from "../summary/SummarySettings";
import { AskAiSettings } from "../search/AskSettings";
import { ComposeAiSettings } from "../compose/settings";
import type { Account, Diagnostics, RemoteImages, SwipeAction } from "../../lib/types";
import { FOLLOW_UP_DAYS, FOLLOW_UP_LOOKBACK_DAYS } from "../../lib/types";
import { chordOf, isMac } from "../../lib/keyboard";
import { setUi, useUi } from "../../lib/ui";
import { bytes, num } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { Kbd, Keys } from "../../components/Kbd";
import { toast } from "../../components/Toast";
import { meta } from "../../app/store";
import { closeSettings, takeFocusSection, useSettingsRequest, type SettingsSection } from "./state";
import { openWelcome } from "../welcome/state";
import { SETTINGS_PAGES, flattenResults, labelMatches, searchSettings, type SettingEntry } from "./catalog";
import { DiagnosticsPanel } from "./Diagnostics";
import { SyncActions, isHidden, syncFixFor } from "./syncFix";
import { retryingText, syncHealth } from "../../lib/syncHealth";
import { useSyncHides } from "../../lib/syncHides";
import { SignInSheetRow } from "./SignInSheet";
import { openAddAccount } from "../onboarding/AddAccountModal";
import { clearPendingSetup, providerName, usePendingSetups } from "../onboarding/pending";
import { ProviderMark } from "../onboarding/ProviderSteps";
import { Choice, ConfirmDialog, Section, Switch, phaseLabel } from "./parts";
import { ProfilesSection } from "./Profiles";
import { WhatsNewSection } from "./WhatsNew";
import { AboutSection } from "./About";
import { YouSection } from "./You";
import { AppearanceSettings } from "./Appearance";
import { SenderPhotosPrivacy, AvatarPlacementRow } from "./SenderPhotos";
import { ListStyleRow } from "./ListStyle";
import { SemanticSearchRow } from "./SemanticSearch";
import { NotificationSettings } from "../notifications/NotificationSettings";
import { ComposeSection } from "./Compose";
import { SignaturesSection } from "../compose/SignatureSettings";
import { RulesSection } from "../rules/RulesSection";
import { SyncSection } from "./Sync";
import { CalendarSection } from "../calendar/CalendarSettings";
import { ShareLinksSettings } from "../share/ShareLinksSettings";
import { AccountColorPicker, AccountNickname } from "./AccountIdentity";
import { AccountAvatar, accountName } from "../../components/Identity";
import { mailboxUnaffected } from "../../lib/capabilities";
import "./settings.css";
import { useKeyTip } from "../../lib/shortcutHints";
import { Select } from "../../components/Select";
import { moveAccountBy, moveAccountTo, removeAccount, setShownInAll } from "../../app/accountActions";
import { useDragReorder } from "../../components/useDragReorder";
import { setSectionHidden, useLayout, type SidebarSection } from "../../lib/layout";
import { clearRecents, useSearchLists } from "../search/storage";
import { coachLearned, resetCoach } from "../../app/ShortcutCoach";

/** Diagnostics feed the Search/Privacy numbers and the live sync rate. */
const POLL_MS = 4000;
/** How long a row picked from search stays highlighted. */
const FLASH_MS = 1600;
/** How long to wait for a picked row whose page is still loading it. */
const WAIT_FOR_ROW_MS = 2000;

/** A search result picked: the row to scroll to and flash (seq re-runs it for the same row). */
type Target = { entry: SettingEntry; seq: number } | null;

/** The Settings screen. Loaded on demand by SettingsModal.tsx (its own chunk). */
export function SettingsDialog() {
  const tip = useKeyTip();
  const [diag, setDiag] = useState<Diagnostics | null>(null);
  const [diagError, setDiagError] = useState<string | null>(null);
  // null: no page picked yet, so the first page shows.
  const [page, setPage] = useState<SettingsSection | null>(null);
  const [target, setTarget] = useState<Target>(null);
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const dialogRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const navRef = useRef<HTMLElement>(null);
  const request = useSettingsRequest();

  const shown: SettingsSection = page ?? SETTINGS_PAGES[0].id;
  const groups = useMemo(() => searchSettings(query), [query]);
  const results = useMemo(() => flattenResults(groups), [groups]);

  const refresh = useCallback(async () => {
    try {
      setDiag(await api.diagnostics());
      setDiagError(null);
    } catch (e) {
      setDiagError(asCommandError(e).message);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const t = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(t);
  }, [refresh]);

  const go = useCallback((id: SettingsSection, entry: SettingEntry | null = null) => {
    setPage(id);
    setTarget((t) => (entry ? { entry, seq: (t?.seq ?? 0) + 1 } : null));
  }, []);

  // openSettings("signatures") and friends: on open, and while open.
  useLayoutEffect(() => {
    const section = takeFocusSection();
    if (section) go(section);
  }, [request, go]);

  useEffect(() => {
    // Restore focus to whatever had it (the gear, the list) on close.
    const prev = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialogRef.current?.focus();
    return () => prev?.focus?.();
  }, []);

  // Each page starts at its top, or at the row a search picked.
  useLayoutEffect(() => {
    const box = scrollRef.current;
    if (!box) return;
    box.scrollTop = 0;
    return target ? revealRow(box, target.entry) : undefined;
  }, [shown, target]);

  const typeQuery = (q: string) => {
    setQuery(q);
    const next = flattenResults(searchSettings(q));
    setCursor(Math.max(0, next.findIndex((h) => h.entry)));
  };

  const pick = (i: number) => {
    const hit = results[i];
    if (!hit) return;
    setCursor(i);
    go(hit.page.id, hit.entry);
  };

  const step = (delta: number, focusNav = false) => {
    const i = SETTINGS_PAGES.findIndex((p) => p.id === shown);
    const next = SETTINGS_PAGES[Math.min(SETTINGS_PAGES.length - 1, Math.max(0, i + delta))];
    go(next.id);
    if (focusNav) requestAnimationFrame(() => navRef.current?.querySelector<HTMLElement>(`[data-page="${next.id}"]`)?.focus());
  };

  const keys = useRef<SettingsKeyHandlers>(null!);
  keys.current = {
    // At once, so keys typed right after ⌘F land in the field.
    focusSearch: () => searchRef.current?.select(),
    step,
    searchKey: (chord) => {
      if (chord === "escape" && query) typeQuery("");
      else if (!query) return false;
      else if (chord === "arrowdown") setCursor((c) => Math.min(results.length - 1, c + 1));
      else if (chord === "arrowup") setCursor((c) => Math.max(0, c - 1));
      else if (chord === "enter") pick(cursor);
      else return false;
      return true;
    },
  };
  useSettingsKeys(dialogRef, searchRef, keys);

  useEffect(() => {
    if (query) document.getElementById(`st-result-${cursor}`)?.scrollIntoView({ block: "nearest" });
  }, [cursor, query]);

  return (
    <div className="st-modal-scrim" onMouseDown={(e) => e.target === e.currentTarget && closeSettings()}>
      <div
        className="st-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
        ref={dialogRef}
        tabIndex={-1}
      >
        <nav className="st-nav" aria-label="Settings pages" ref={navRef}>
          <div className="st-nav-title">Settings</div>
          <div className="st-search">
            <Icon name="search" size="xs" />
            <input
              ref={searchRef}
              className="st-search-input"
              type="text"
              role="combobox"
              aria-label="Search settings"
              aria-expanded={!!query}
              aria-controls="st-results"
              aria-activedescendant={query && results[cursor] ? `st-result-${cursor}` : undefined}
              autoComplete="off"
              spellCheck={false}
              placeholder="Search settings"
              value={query}
              onChange={(e) => typeQuery(e.target.value)}
            />
            {query ? (
              <button className="btn btn-ghost btn-icon st-search-clear" aria-label="Clear search" onClick={() => typeQuery("")}>
                <Icon name="x" size="xs" />
              </button>
            ) : (
              <span className="st-search-key">
                <Keys keys="mod+f" />
              </span>
            )}
          </div>
          <div className="st-nav-list">
            {query ? (
              <div id="st-results" role="listbox" aria-label="Matching settings" className="st-results">
                {results.length === 0 && <p className="st-muted st-no-results">No settings match “{query.trim()}”.</p>}
                {results.map((h, i) =>
                  h.entry ? (
                    <button
                      key={`${h.page.id}:${h.entry.label}`}
                      id={`st-result-${i}`}
                      role="option"
                      aria-selected={i === cursor}
                      tabIndex={-1}
                      className={"st-result" + (i === cursor ? " active" : "")}
                      title={h.entry.desc ?? h.entry.label}
                      onMouseMove={() => i !== cursor && setCursor(i)}
                      onClick={() => pick(i)}
                    >
                      <span className="min0">{h.entry.label}</span>
                    </button>
                  ) : (
                    <button
                      key={h.page.id}
                      id={`st-result-${i}`}
                      role="option"
                      aria-selected={i === cursor}
                      tabIndex={-1}
                      className={"st-result st-result-page" + (i === cursor ? " active" : "")}
                      onMouseMove={() => i !== cursor && setCursor(i)}
                      onClick={() => pick(i)}
                    >
                      <Icon name={h.page.icon} size="xs" />
                      <span className="truncate">{h.page.label}</span>
                    </button>
                  ),
                )}
              </div>
            ) : (
              SETTINGS_PAGES.map((n) => (
                <button
                  key={n.id}
                  data-page={n.id}
                  className={"st-nav-item" + (shown === n.id ? " active" : "")}
                  aria-current={shown === n.id ? "page" : undefined}
                  // One tab stop for the nav (the current page); ↑/↓ move between pages.
                  tabIndex={shown === n.id ? 0 : -1}
                  onClick={() => go(n.id)}
                >
                  <Icon name={n.icon} size="sm" />
                  <span className="grow">{n.label}</span>
                </button>
              ))
            )}
          </div>
          <span className="st-muted st-nav-foot">
            <Icon name="lock" size="xs" />
            Saved on this Mac only
          </span>
        </nav>
        <div className="st-main">
          <div className="st-scroll" ref={scrollRef}>
            <SettingsPage id={shown} diag={diag} diagError={diagError} refresh={refresh} />
          </div>
          <footer className="st-foot">
            <span className="hint">
              <Kbd>Tab</Kbd>Next setting
            </span>
            <span className="hint">
              <Keys keys="mod+[" />
              <Keys keys="mod+]" />
              Pages
            </span>
            <span className="hint">
              <Keys keys="mod+f" />
              Search
            </span>
            <span className="hint">
              <Kbd>Esc</Kbd>Close
            </span>
            <span className="right">
              <Icon name="shield" size="xs" />
              No Penguin servers
            </span>
          </footer>
        </div>
        {/* After the page in tab order; over the page's top strip. */}
        <button className="btn btn-ghost btn-sm btn-icon st-close" aria-label="Close settings" title={tip("Close", "Esc")} onClick={closeSettings}>
          <Icon name="x" size="sm" />
        </button>
      </div>
    </div>
  );
}

/** One page of Settings: only its section is mounted. */
function SettingsPage({
  id,
  diag,
  diagError,
  refresh,
}: {
  id: SettingsSection;
  diag: Diagnostics | null;
  diagError: string | null;
  refresh: () => Promise<void>;
}) {
  switch (id) {
    case "you":
      return <YouSection />;
    case "accounts":
      return <AccountsSection diag={diag} onChanged={refresh} />;
    case "profiles":
      return <ProfilesSection />;
    case "general":
      return <GeneralSection />;
    case "inbox":
      return <InboxSection />;
    case "sync":
      return <SyncSection />;
    case "calendar":
      return <CalendarSection />;
    case "compose":
      return <ComposeSection />;
    case "signatures":
      return <SignaturesSection />;
    case "privacy":
      return <PrivacySection diag={diag} />;
    case "sharing":
      return <ShareLinksSettings />;
    case "search":
      return <SearchSection diag={diag} onChanged={refresh} />;
    case "ai":
      return <AiSection />;
    case "views":
      return <SmartViewsSection />;
    case "rules":
      return <RulesSection />;
    case "keyboard":
      return <KeyboardSection />;
    case "diagnostics":
      return <DiagnosticsPanel diag={diag} error={diagError} onRefresh={refresh} />;
    case "whatsnew":
      return <WhatsNewSection />;
    case "about":
      return <AboutSection />;
  }
}

/** The rendered row for a search result: by `find`, else by its label's text. */
function findRow(box: HTMLElement, entry: SettingEntry): HTMLElement | null {
  if (entry.find) {
    const el = box.querySelector<HTMLElement>(entry.find);
    return el ? (el.closest<HTMLElement>(".setting-row, .setting-account") ?? el) : null;
  }
  const label = [...box.querySelectorAll<HTMLElement>(".setting-label, .st-sub")].find((e) => labelMatches(entry, e.textContent ?? ""));
  if (!label || label.matches(".st-sub")) return label ?? null;
  // A label sits in its row, or in a text column (label + help) inside it. Rows
  // can nest (Calendar's "Answer invitations" is inside "Google Calendar"), so
  // take the nearest, not the outermost .setting-row.
  const col = label.parentElement;
  if (!col || col.matches(".setting-row")) return col;
  const row = col.parentElement;
  return row && !row.matches(".settings-section") ? row : col;
}

/**
 * Scroll a picked search result into view and flash it. Some rows appear only
 * once their data loads (calendar status, rules, the sign-in method), so a row
 * that isn't there yet is watched for until it renders, for a moment.
 */
function revealRow(box: HTMLElement, entry: SettingEntry): () => void {
  let flash: ReturnType<typeof setTimeout> | undefined;
  const show = (el: HTMLElement) => {
    el.scrollIntoView({ block: "center" });
    el.classList.add("st-flash");
    flash = setTimeout(() => el.classList.remove("st-flash"), FLASH_MS);
  };
  const cleanup = (el: HTMLElement | null) => () => {
    clearTimeout(flash);
    el?.classList.remove("st-flash");
  };
  const now = findRow(box, entry);
  if (now) {
    show(now);
    return cleanup(now);
  }
  let found: HTMLElement | null = null;
  const watch = new MutationObserver(() => {
    found = findRow(box, entry);
    if (!found) return;
    watch.disconnect();
    show(found);
  });
  watch.observe(box, { childList: true, subtree: true });
  const giveUp = setTimeout(() => watch.disconnect(), WAIT_FOR_ROW_MS);
  return () => {
    watch.disconnect();
    clearTimeout(giveUp);
    cleanup(found)();
  };
}

const FOCUSABLE = 'button:not([disabled]), a[href], input:not([disabled]), select, textarea, [tabindex]:not([tabindex="-1"])';

interface SettingsKeyHandlers {
  focusSearch: () => void;
  /** Previous / next page; focusNav moves focus along in the nav too. */
  step: (delta: number, focusNav?: boolean) => void;
  /** A key in the search field: true when it was used. */
  searchKey: (chord: string) => boolean;
}

/**
 * While Settings is open the mail UI underneath must not react to keys
 * (e, #, j/k would act on the hidden selection). A capture-phase listener runs
 * before the global keyboard handler: Esc closes, Tab is trapped in the
 * dialog, ⌘F searches, ⌘[ / ⌘] (and ↑/↓ in the nav) change pages, and only
 * app-level keys (⌘-combos, ?) pass through.
 */
const GRID_KEYS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "Enter", " "]);

function useSettingsKeys(
  dialogRef: RefObject<HTMLDivElement | null>,
  searchRef: RefObject<HTMLInputElement | null>,
  handlers: RefObject<SettingsKeyHandlers>,
) {
  const overlay = useUi((s) => s.overlay);
  useEffect(() => {
    if (overlay) return; // an overlay (⌘K, ?) owns the keyboard
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing) return;
      // A dialog above Settings (confirm, reconnect, add account) handles its own keys.
      if (document.querySelector(".st-confirm, .onb-modal-scrim, .rl-editor")) return;
      // An inline editor or popover (account rename/color) cancels on Esc itself.
      if (e.key === "Escape" && document.querySelector("[data-esc-owner]")) return;
      // A color grid with focus owns movement and Enter (components/ColorGrid.tsx
      // stops them from reaching the app itself).
      if (GRID_KEYS.has(e.key) && document.activeElement?.closest("[data-grid-keys]")) return;
      // ⌥↑/⌥↓ in a reorderable row (Settings → Accounts) move it.
      if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown") && document.activeElement?.closest("[data-reorder-keys]")) return;
      const chord = chordOf(e);
      if (!chord) return;
      const h = handlers.current;
      const consume = () => {
        e.preventDefault();
        e.stopPropagation();
      };
      if (document.activeElement === searchRef.current && h.searchKey(chord)) return consume();
      if (chord === "mod+f") {
        consume();
        h.focusSearch();
        return;
      }
      if (chord === "mod+[" || chord === "mod+]") {
        consume();
        h.step(chord === "mod+[" ? -1 : 1, !!document.activeElement?.closest(".st-nav-item"));
        return;
      }
      if ((chord === "arrowup" || chord === "arrowdown") && document.activeElement?.closest(".st-nav-item")) {
        consume();
        h.step(chord === "arrowup" ? -1 : 1, true);
        return;
      }
      if (chord === "escape") {
        consume();
        closeSettings();
        return;
      }
      if (chord === "tab" || chord === "shift+tab") {
        const dialog = dialogRef.current;
        if (!dialog) return;
        const items = [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null);
        if (items.length === 0) return;
        const first = items[0];
        const last = items[items.length - 1];
        const inside = dialog.contains(document.activeElement);
        if (chord === "tab" && (!inside || document.activeElement === last || document.activeElement === dialog)) {
          e.preventDefault();
          first.focus();
        } else if (chord === "shift+tab" && (!inside || document.activeElement === first || document.activeElement === dialog)) {
          e.preventDefault();
          last.focus();
        }
        e.stopPropagation();
        return;
      }
      if (chord.startsWith("mod+") || chord === "?") return;
      // Native controls (Space, Enter on buttons) still work; single-key
      // mail shortcuts don't reach the global handler.
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [overlay, dialogRef, searchRef, handlers]);
}

function save(patch: Parameters<typeof updateSettings>[0]) {
  updateSettings(patch).catch((e) => toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` }));
}

// ---------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------

function AccountsSection({ diag, onChanged }: { diag: Diagnostics | null; onChanged: () => void }) {
  const accounts = meta.use((m) => m.accounts);
  const sync = meta.use((m) => m.sync);
  useSyncHides(); // re-render when a sync alert is hidden or shown again
  const [confirming, setConfirming] = useState<Account | null>(null);
  // Drag the handle to reorder; ⌥↑/⌥↓ with focus in a row moves it one place.
  const ids = accounts.map((a) => a.id);
  const { drag, listRef, rowRef, onPointerDown, rowStyle } = useDragReorder(ids, (id, to) => {
    const a = accounts.find((x) => x.id === id);
    if (a) void moveAccountTo(a, ids, to);
  });
  const grips = useRef(new Map<string, HTMLButtonElement>());
  const reorderable = accounts.length > 1;
  const moveKey = (a: Account, e: ReactKeyboardEvent) => {
    if (!e.altKey || e.metaKey || e.ctrlKey || e.shiftKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
    // ⌥↑ in the nickname field moves its caret, as usual.
    if ((e.target as HTMLElement).closest("input, textarea, [contenteditable]")) return;
    e.preventDefault();
    e.stopPropagation();
    void moveAccountBy(a, ids, e.key === "ArrowUp" ? -1 : 1).then(() =>
      // Moving the row can move its focused handle out of the DOM and back.
      requestAnimationFrame(() => grips.current.get(a.id)?.focus()),
    );
  };

  const remove = async (a: Account) => {
    setConfirming(null);
    if (await removeAccount(a)) onChanged();
  };

  return (
    <Section
      id="accounts"
      icon="users"
      title="Accounts"
      badge={<span className="badge t-gray">{accounts.length === 1 ? "1 account" : `${accounts.length} accounts`}</span>}
    >
      <div className={"reorder-list st-accounts" + (drag ? " is-dragging" : "")} ref={listRef}>
      {accounts.map((a, i) => {
        const s = sync[a.id];
        const stored = diag?.accounts.find((d) => d.accountId === a.id)?.messagesStored ?? s?.indexed ?? 0;
        const fix = syncFixFor(s);
        return (
          <div
            className={"setting-account" + (drag?.id === a.id ? " is-lifted" : "")}
            key={a.id}
            ref={rowRef(a.id)}
            style={rowStyle(a.id)}
            data-reorder-keys=""
            onKeyDown={reorderable ? (e) => moveKey(a, e) : undefined}
          >
            {reorderable && (
              <button
                ref={(el) => {
                  if (el) grips.current.set(a.id, el);
                  else grips.current.delete(a.id);
                }}
                className="btn btn-ghost btn-sm btn-icon account-grip"
                aria-label={`Move ${a.email}, position ${i + 1} of ${accounts.length}`}
                aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
                title={`Drag to reorder, or ${isMac ? "⌥↑ ⌥↓" : "Alt+↑ Alt+↓"}`}
                onPointerDown={(e) => onPointerDown(a.id, e)}
                onDragStart={(e) => e.preventDefault()}
              >
                <Icon name="grip" size="xs" />
              </button>
            )}
            <AccountAvatar account={a} accounts={accounts} size="md" />
            <AccountColorPicker account={a} />
            <div className="grow min0">
              <AccountNickname account={a} accounts={accounts} />
              <div className="account-mail truncate">{a.email}</div>
            </div>
            <div className="account-meta tnum">
              <span className={"st-phase" + (fix && !isHidden(s) ? " is-bad" : "")} title={fix?.detail ?? s?.error ?? undefined}>
                {fix ? fix.message : s && syncHealth(s) === "retrying" ? retryingText(s) : s ? phaseLabel(s.phase) : "Idle"}
              </span>
              <span className="st-muted">{num(stored)} messages</span>
            </div>
            <SyncActions status={s} hide={false} />
            <button
              className="btn btn-ghost btn-sm btn-icon account-remove"
              aria-label={`Remove ${a.email}`}
              title="Remove account"
              onClick={() => setConfirming(a)}
            >
              <Icon name="trash" size="xs" />
            </button>
          </div>
        );
      })}
      {drag && <div className="reorder-line" style={{ transform: `translateY(${drag.lineY}px)` }} aria-hidden="true" />}
      </div>
      <PendingSetupRows accounts={accounts} onChanged={onChanged} />
      <div className="settings-note row-flex" data-setting="add-account">
        <span className="grow" />
        <button className="btn btn-ghost btn-sm" onClick={() => openAddAccount(() => onChanged())}>
          <Icon name="plus" size="xs" />
          Add account
        </button>
      </div>
      {accounts.length > 1 && <AllInboxesSettings accounts={accounts} />}
      <SignInSheetRow />
      {confirming ? (
        <ConfirmDialog
          title={`Remove ${confirming.email}?`}
          body={`Signs out and deletes its mail from this Mac. ${mailboxUnaffected(confirming)}`}
          confirm="Remove"
          onConfirm={() => void remove(confirming)}
          onCancel={() => setConfirming(null)}
        />
      ) : null}
    </Section>
  );
}

/** Unfinished Add account setups: "sam@… · Setting up · Yahoo", with Resume and Remove. */
function PendingSetupRows({ accounts, onChanged }: { accounts: Account[]; onChanged: () => void }) {
  const pending = usePendingSetups().filter((p) => !accounts.some((a) => a.email.toLowerCase() === p.email));
  return (
    <>
      {pending.map((p) => (
        <div className="setting-account is-pending" key={p.email}>
          <ProviderMark setup={p.kind} />
          <div className="grow min0">
            <div className="account-title truncate">{p.email}</div>
            <div className={"account-mail truncate" + (p.lastError ? " st-error-text" : "")} title={p.lastError ?? undefined}>
              {p.lastError ?? providerName(p.kind)}
            </div>
          </div>
          <span className="badge t-gray">Setting up</span>
          <button className="btn btn-secondary btn-sm" onClick={() => openAddAccount(() => onChanged(), p)}>
            Resume
          </button>
          <button
            className="btn btn-ghost btn-sm btn-icon account-remove"
            aria-label={`Remove ${p.email}`}
            title="Remove"
            onClick={() => clearPendingSetup(p.email)}
          >
            <Icon name="trash" size="xs" />
          </button>
        </div>
      ))}
    </>
  );
}

/** Settings.hiddenFromAll, one switch per account (app/allInboxes.ts). */
function AllInboxesSettings({ accounts }: { accounts: Account[] }) {
  const hidden = useSettings().hiddenFromAll;
  return (
    <div className="st-all-inboxes">
      <h3 className="st-sub">Show in All Inboxes</h3>
      <p className="st-muted">
        Off keeps the account syncing but leaves its mail out of All accounts. Pick it in the sidebar to read it.
      </p>
      {accounts.map((a) => {
        const on = !hidden.includes(a.id);
        const name = accountName(a, accounts);
        return (
          <div className="setting-row" key={a.id}>
            <div className="row-flex min0">
              <AccountAvatar account={a} accounts={accounts} />
              <span className="setting-label truncate">{name}</span>
              {name !== a.email && <span className="st-muted truncate">{a.email}</span>}
            </div>
            <Switch label={`Show ${a.email} in All Inboxes`} on={on} onChange={(v) => void setShownInAll(a, v)} />
          </div>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// General
// ---------------------------------------------------------------------------
const SWIPE_OPTIONS: { value: SwipeAction; label: string }[] = [
  { value: "toggleRead", label: "Mark read / unread" },
  { value: "star", label: "Star / unstar" },
  { value: "archive", label: "Archive" },
  { value: "trash", label: "Trash" },
  { value: "none", label: "Nothing" },
];

/** Two-finger swipes on list rows (features/inbox/useRowSwipe.ts). */
function SwipeSettings() {
  const s = useSettings();
  const rows: { key: "swipeRight" | "swipeLeft" | "swipeLeftLong"; label: string }[] = [
    { key: "swipeRight", label: "Swipe right" },
    { key: "swipeLeft", label: "Swipe left" },
    { key: "swipeLeftLong", label: "Long swipe left" },
  ];
  return (
    <div className="setting-row setting-tall st-swipe">
      <div>
        <span className="setting-label">Swipe actions</span>
        <p className="st-muted">Two-finger swipe on a conversation in the list. A long swipe goes past 60% of the row.</p>
      </div>
      <div className="st-swipe-grid">
        {rows.map((r) => (
          <div key={r.key} className="st-swipe-row">
            <span className="st-muted">{r.label}</span>
            <Select<SwipeAction>
              className="st-select"
              label={r.label}
              value={s[r.key]}
              options={SWIPE_OPTIONS}
              onChange={(v) => save({ [r.key]: v })}
            />
          </div>
        ))}
      </div>
    </div>
  );
}

const FOLLOW_UP_OPTIONS = Array.from({ length: FOLLOW_UP_DAYS.max - FOLLOW_UP_DAYS.min + 1 }, (_, i) => {
  const n = FOLLOW_UP_DAYS.min + i;
  return { value: String(n), label: n === 1 ? "1 day" : n === 7 ? "1 week" : n === 14 ? "2 weeks" : `${n} days` };
});

/** How long sent mail waits before it's listed in Follow up (features/triage). */
function FollowUpRow() {
  const days = useSetting("followUpDays");
  return (
    <div className="setting-row setting-tall">
      <div>
        <span className="setting-label">Follow up after</span>
        <p className="st-muted">
          Mail you sent that gets no reply shows in Follow up after this long (up to {FOLLOW_UP_LOOKBACK_DAYS} days back), so you can nudge. Messages
          only to no-reply addresses or to yourself, and calendar replies, are left out.
        </p>
      </div>
      <Select<string>
        className="st-select"
        label="Follow up after"
        value={String(days)}
        options={FOLLOW_UP_OPTIONS}
        onChange={(v) => save({ followUpDays: Number(v) })}
      />
    </div>
  );
}

function GeneralSection() {
  const s = useSettings();
  return (
    <Section id="general" icon="sliders" title="General">
      <div className="setting-row">
        <span className="setting-label">Theme</span>
        <Choice
          label="Theme"
          value={s.theme}
          onChange={(theme) => save({ theme })}
          options={[
            { value: "dark", label: "Dark", icon: "moon" },
            { value: "light", label: "Light", icon: "sun" },
            { value: "system", label: "System" },
          ]}
        />
      </div>
      <AppearanceSettings />
      <SidebarSectionRow id="accounts" label="Accounts in the sidebar" note="Off removes the section. The switcher above it still picks an account." />
      <SidebarSectionRow
        id="labels"
        label="Labels in the sidebar"
        note={`Off removes the section rather than folding it. ${isMac ? "⌘K" : "Ctrl+K"} still goes to any label.`}
      />
      <ListStyleRow />
      <div className="setting-row">
        <span className="setting-label">Density</span>
        <Choice
          label="Density"
          value={s.density}
          onChange={(density) => save({ density })}
          options={[
            { value: "compact", label: "Compact" },
            { value: "comfortable", label: "Comfortable" },
          ]}
        />
      </div>
      <AvatarPlacementRow />
      <FollowUpRow />
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Show keyboard shortcut hints</span>
          <p className="st-muted">Key caps like E and ⌘K on buttons, rows and menus. Shortcuts keep working when hidden, and ? always lists them.</p>
        </div>
        <Switch
          label="Show keyboard shortcut hints"
          on={s.showShortcutHints}
          onChange={(showShortcutHints) => save({ showShortcutHints })}
        />
      </div>
      <NotificationSettings />
      <SwipeSettings />
      <div className="setting-row setting-tall" data-setting="welcome">
        <div>
          <span className="setting-label">Welcome setup</span>
          <p className="st-muted">The first-run questions again: search by meaning, Apple Intelligence, how much mail to keep, inbox style, look and keys.</p>
        </div>
        <button className="btn btn-secondary" onClick={() => { closeSettings(); openWelcome(); }}>
          Set up Penguin…
        </button>
      </div>
    </Section>
  );
}

/** Show or hide a sidebar section. Per device, like the sidebar's width and folds (lib/layout.ts). */
function SidebarSectionRow({ id, label, note }: { id: SidebarSection; label: string; note: string }) {
  const shown = useLayout((s) => !s.hiddenSections.includes(id));
  return (
    <div className="setting-row setting-tall">
      <div>
        <span className="setting-label">{label}</span>
        <p className="st-muted">{note}</p>
      </div>
      <Switch label={label} on={shown} onChange={(on) => setSectionHidden(id, !on)} />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Privacy
// ---------------------------------------------------------------------------
const IMAGES_NOTE = {
  ask: "Blocked until you choose Load images.",
  always: "Loaded when you open a message. Senders can see when you read.",
  never: "Never loaded, and the Load images button is hidden.",
} as const;

function pixelsNote(block: boolean, images: RemoteImages): string {
  if (block) return "Known email trackers and invisible 1×1 images are removed, even when images load.";
  return images === "never"
    ? "Trackers are treated like other images. With images never loading, they never load either."
    : "Trackers are treated like other images: when images load, the sender can tell you opened the email.";
}

function PrivacySection({ diag }: { diag: Diagnostics | null }) {
  const s = useSettings();
  const trackers = diag?.trackersRemovedSession ?? 0;
  return (
    <Section id="privacy" icon="shield" title="Privacy">
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Remote images</span>
          <p className="st-muted">{IMAGES_NOTE[s.remoteImages]}</p>
        </div>
        <Choice
          label="Remote images"
          value={s.remoteImages}
          onChange={(remoteImages) => save({ remoteImages })}
          options={[
            { value: "ask", label: "Ask" },
            { value: "always", label: "Always" },
            { value: "never", label: "Never" },
          ]}
        />
      </div>
      {s.remoteImages === "ask" ? (
        <div className="setting-row setting-tall st-senders">
          <div className="min0 grow">
            <span className="setting-label">Always load images from</span>
            <p className="st-muted">Only when Google verifies the message really came from that sender.</p>
            {s.trustedImageSenders.length === 0 ? (
              <p className="st-muted">No senders yet. Choose “Always from this sender” on a message.</p>
            ) : (
              <ul className="st-sender-list">
                {s.trustedImageSenders.map((email) => (
                  <li key={email}>
                    <span className="truncate">{email}</span>
                    <button
                      className="btn btn-ghost btn-sm btn-icon"
                      aria-label={`Stop loading images from ${email}`}
                      title="Remove"
                      onClick={() =>
                        setTrustedImageSender(email, false).catch((e) =>
                          toast({ tone: "error", message: asCommandError(e).message }),
                        )
                      }
                    >
                      <Icon name="x" size="xs" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      ) : null}
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Block tracking pixels</span>
          <p className="st-muted">{pixelsNote(s.blockTrackingPixels, s.remoteImages)}</p>
        </div>
        <div className="st-inline">
          {s.blockTrackingPixels ? (
            <span className="badge t-green" title="Trackers removed from messages opened since Penguin started">
              <Icon name="shield" size="xs" />
              {num(trackers)} this session
            </span>
          ) : null}
          <Switch label="Block tracking pixels" on={s.blockTrackingPixels} onChange={(blockTrackingPixels) => save({ blockTrackingPixels })} />
        </div>
      </div>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Remove tracking from links</span>
          <p className="st-muted">
            Takes the parts of a link that identify you or your click (like mc_eid, _hsenc and fbclid) out of links before you open them. Campaign
            tags like utm_source stay. Links that go through a sender's click tracker still report the click.
          </p>
        </div>
        <Switch label="Remove tracking from links" on={s.stripLinkTracking} onChange={(stripLinkTracking) => save({ stripLinkTracking })} />
      </div>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Ask for read receipts</span>
          <p className="st-muted">
            Mail you send asks the recipient's mail app for a read receipt, in the standard, visible way. Their app decides, and usually asks
            them first. Personal Gmail and Apple Mail never send one, so no receipt doesn't mean unread. When one comes back, your message
            shows “Read by …”. Penguin doesn't hide tracking pixels in your mail, and never sends receipts for mail you get.
          </p>
        </div>
        <Switch label="Ask for read receipts" on={s.requestReadReceipts} onChange={(requestReadReceipts) => save({ requestReadReceipts })} />
      </div>
      <SenderPhotosPrivacy />
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Show unsubscribe button</span>
          <p className="st-muted">
            On mailing-list mail, next to the tracker count. One click asks the sender to stop, sends their unsubscribe email, or opens
            their page. It always asks first.
          </p>
        </div>
        <Switch label="Show unsubscribe button" on={s.unsubscribeButton} onChange={(unsubscribeButton) => save({ unsubscribeButton })} />
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------
function tildePath(p: string): string {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

function SearchSection({ diag, onChanged }: { diag: Diagnostics | null; onChanged: () => void }) {
  const [optimizing, setOptimizing] = useState(false);
  const optimize = async () => {
    setOptimizing(true);
    const started = performance.now();
    try {
      await api.optimizeIndex();
      toast({ message: `Search index optimized in ${((performance.now() - started) / 1000).toFixed(1)} s` });
      onChanged();
    } catch (e) {
      toast({ tone: "error", message: `Couldn't optimize: ${asCommandError(e).message}` });
    } finally {
      setOptimizing(false);
    }
  };
  return (
    <Section id="search" icon="search" title="Search" badge={<span className="badge t-gray">On this Mac</span>}>
      <SemanticSearchRow />
      <p className="index-size tnum">
        {diag ? `${bytes(diag.dbBytes + diag.walBytes)} for ${num(diag.totalMessages)} messages` : "Measuring…"}
      </p>
      <p className="index-path mono">{diag ? tildePath(diag.dbPath) : " "}</p>
      <div className="index-actions">
        <button className="st-link" onClick={() => void api.revealPath("data").catch((e) => toast({ tone: "error", message: asCommandError(e).message }))}>
          Show in Finder <Icon name="right" size="xs" />
        </button>
        <button className="btn btn-secondary" data-setting="optimize-index" disabled={optimizing} onClick={() => void optimize()}>
          <Icon name="refresh" size="xs" />
          {optimizing ? "Optimizing…" : "Optimize index"}
        </button>
      </div>
      <p className="st-muted settings-note">Search stays local. Optimizing merges index segments; search keeps working meanwhile.</p>
      <RecentSearchesRow />
    </Section>
  );
}

/** Recent searches and Ask questions are one list (features/search/storage.ts). */
function RecentSearchesRow() {
  const n = useSearchLists().recent.length;
  return (
    <div className="setting-row setting-tall st-recents">
      <div>
        <span className="setting-label">Recent searches</span>
        <p className="st-muted">
          {n ? `${n} recent ${n === 1 ? "search or question" : "searches and questions"}, kept on this Mac.` : "None right now."} Saved
          searches stay.
        </p>
      </div>
      <button className="btn btn-secondary" disabled={!n} onClick={() => clearRecents()}>
        Clear
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// AI: on-device features (summaries, Ask reading, Write with AI, suggested replies)
// ---------------------------------------------------------------------------
function AiSection() {
  return (
    <Section id="ai" icon="sparkles" title="AI" className="ai-settings">
      <p className="st-muted ai-intro">
        Everything here runs on this Mac with Apple Intelligence: no API key, no server, and nothing leaves your Mac. Each
        feature can be turned off on its own.
      </p>
      <SummarySettings />
      <AskAiSettings />
      <ComposeAiSettings />
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------
function KeyboardSection() {
  const s = useSettings();
  const [learned, setLearned] = useState(() => coachLearned());
  return (
    <Section id="keyboard" icon="keyboard" title="Keyboard">
      <button className="shortcuts-link" onClick={() => setUi({ overlay: "shortcuts" })}>
        <div className="grow">
          <strong>Shortcuts</strong>
          <p className="st-muted">Open the keyboard cheat sheet</p>
        </div>
        <Kbd>?</Kbd>
        <Icon name="right" size="xs" />
      </button>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Shortcut coach</span>
          <p className="st-muted">
            When you click something that has a key, the key shows by it for a moment. At most one every 20 seconds, and a key you've pressed
            twice yourself is never shown again.
            {learned > 0 ? ` You know ${learned} so far.` : ""}
          </p>
        </div>
        <div className="st-inline">
          {learned > 0 ? (
            <button
              className="btn btn-ghost btn-sm"
              title="Show hints again for every key"
              onClick={() => {
                resetCoach();
                setLearned(0);
              }}
            >
              Start over
            </button>
          ) : null}
          <Switch label="Shortcut coach" on={s.shortcutCoach} onChange={(shortcutCoach) => save({ shortcutCoach })} />
        </div>
      </div>
    </Section>
  );
}


