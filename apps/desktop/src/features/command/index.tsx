// OWNER: ui-search agent. Mounted once by App; renders when ui.overlay === "command".
//
// ⌘K palette (design/05-command.html): every registered shortcut with its key,
// plus navigation (views, labels, accounts), settings and "Search mail for …".
// Opens and closes with no animation.
import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent, type ReactNode } from "react";
import { api } from "../../lib/api";
import { getShortcuts, subscribeShortcuts, type Shortcut } from "../../lib/keyboard";
import { Keys } from "../../components/Kbd";
import { getUi, setUi, useUi } from "../../lib/ui";
import type { Account, Label, MailboxView } from "../../lib/types";
import { toneForColor } from "../../lib/format";
import { accountTone } from "../../lib/accountColor";
import { Icon, type IconName } from "../../components/Icon";
import { openSearch } from "../search";
import { applyDevParams, devParam } from "../search/dev";
import { accountName } from "../../components/Identity";
import { matchPeople, seedPeople, usePeople } from "../search/people";
import { fuzzyScore } from "./fuzzy";
import { goTo, profileKeys, switchAccount } from "../../app/shortcuts";
import { switchProfile, useActiveProfile, useProfiles } from "../../app/profiles";
import { listOrderedAccounts } from "../../app/store";
import { openSettings } from "../settings/state";
import { openWelcome } from "../welcome/state";
import { isDemo, setDemoMode } from "../../app/demoMode";
import { useSetting } from "../../lib/settings";
import { sidebarSmartItems, smartDef } from "../smart/catalog";
import { composeCommands } from "../compose/commands";
import { useWriterReady } from "../compose/ai";
import { splitPaletteCommands } from "../split/palette";
import { isMainWindow } from "../../lib/windowBus";
import "./command.css";

export function CommandPalette() {
  const overlay = useUi((s) => s.overlay);
  useEffect(applyDevParams, []);
  if (overlay !== "command") return null;
  return <Palette />;
}

// ---------------------------------------------------------------------------

interface Cmd {
  id: string;
  group: string;
  label: string;
  meta?: string;
  keys?: string;
  icon?: IconName;
  dotTone?: string;
  /** Extra words that should match (e.g. "archive" for Done). */
  alias?: string;
  run: () => void;
}

const GROUP_ORDER = ["Suggested", "Best matches", "People", "Search", "Go to", "Split Inbox", "Views", "Triage", "Compose", "Labels", "Accounts", "App", "Settings"];

/** Shown first when the palette opens (only those active right now). */
const SUGGESTED = ["k:compose.reply", "k:triage.done", "k:compose.new", "k:search.open", "k:triage.label", "k:triage.move"];

// Views without a registered shortcut still belong in "Go to".
const VIEWS: Array<[MailboxView["kind"], string, IconName, string?]> = [
  ["inbox", "Go to Inbox", "inbox"],
  ["replyLater", "Go to Reply Later", "replyLater", "owe reply later respond"],
  ["followUp", "Go to Follow up", "followUp", "waiting awaiting nudge no reply unanswered"],
  ["starred", "Go to Starred", "star"],
  ["snoozed", "Go to Snoozed", "snooze", "later remind"],
  ["sent", "Go to Sent", "send"],
  ["drafts", "Go to Drafts", "draft"],
  ["done", "Go to Done", "done", "archive archived"],
  ["all", "Go to All mail", "archive"],
  ["spam", "Go to Spam", "shield"],
  ["trash", "Go to Trash", "trash", "deleted bin"],
];

function iconFor(s: Shortcut): IconName {
  const byId: Record<string, IconName> = {
    "go.inbox": "inbox", "go.replyLater": "replyLater", "go.followUp": "followUp", "triage.replyLater": "replyLater", "go.starred": "star", "go.snoozed": "snooze", "go.sent": "send", "go.drafts": "draft", "go.done": "done",
    "go.all": "archive", "go.trash": "trash", "triage.done": "done", "triage.trash": "trash", "triage.star": "star",
    "triage.read": "unread", "triage.unread": "unread", "triage.unsubscribe": "belloff", "triage.label": "tag", "triage.move": "folder", "triage.snooze": "snooze", "triage.undo": "undo", "thread.copy": "copy",
    "compose.new": "compose", "compose.reply": "reply", "compose.replyAll": "replyall", "compose.forward": "forward",
    "search.open": "search", "app.shortcuts": "keyboard", "app.theme": getUi().theme === "dark" ? "sun" : "moon",
    "app.sync": "refresh", "acct.all": "users", "ctx.toggle": "columns", "thread.openWindow": "window", "compose.newWindow": "window",
  };
  return byId[s.id] ?? GROUP_ICON[s.group ?? ""] ?? "command";
}

const GROUP_ICON: Record<string, IconName> = {
  Navigate: "right",
  Triage: "done",
  Compose: "compose",
  Search: "search",
  App: "command",
  "Split Inbox": "columns",
};

function useShortcuts(): Shortcut[] {
  const snap = useRef<Shortcut[]>(getShortcuts());
  return useSyncExternalStore(
    (cb) =>
      subscribeShortcuts(() => {
        snap.current = getShortcuts();
        cb();
      }),
    () => snap.current,
  );
}

function Palette() {
  const [q, setQ] = useState(() => devParam("q") ?? "");
  const [sel, setSel] = useState(0);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [labels, setLabels] = useState<Label[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const shortcuts = useShortcuts();
  const accountFilter = useUi((s) => s.accountFilter);
  const profiles = useProfiles();
  const profile = useActiveProfile();
  // `when` guards read the selection and view, so re-evaluate when they change.
  const context = useUi((s) => `${s.selected?.accountId}/${s.selected?.threadId}|${s.threadOpen}|${JSON.stringify(s.view)}`);
  const people = usePeople();
  const smartViews = useSetting("smartViews");
  // Compose commands (features/compose/commands.ts): instant replies, reply with AI.
  const instantReplies = useSetting("instantReplies");
  const writerReady = useWriterReady();
  // Split Inbox and Get to zero entries follow their settings.
  const splitSettings = `${useSetting("inboxTabs")}|${JSON.stringify(useSetting("inboxSplits"))}|${useSetting("getToZero")}`;

  useEffect(() => {
    inputRef.current?.focus();
    seedPeople();
    listOrderedAccounts().then(setAccounts).catch(() => setAccounts([]));
    api.listLabels(null).then(setLabels).catch(() => setLabels([]));
  }, []);

  const all = useMemo<Cmd[]>(() => {
    const out: Cmd[] = [];
    const seen = new Set<string>();
    const add = (c: Cmd) => {
      const k = c.label.toLowerCase();
      if (seen.has(k)) return;
      seen.add(k);
      out.push(c);
    };
    // Registered shortcuts first: they carry the keys the palette teaches.
    for (const s of shortcuts) {
      // Profile keys are listed below with their colors, in the Profiles group.
      if (s.id === "command.open" || s.hidden || s.id.startsWith("profile.")) continue;
      if (s.when && !safeWhen(s)) continue;
      const group = s.group === "Navigate" ? "Go to" : s.group ?? "App";
      add({ id: `k:${s.id}`, group, label: s.label, keys: s.keys, icon: iconFor(s), run: s.run });
    }
    for (const c of composeCommands(writerReady)) add(c);
    // A conversation window lists what it can do to its conversation; views,
    // labels, accounts and Settings belong to the main window.
    if (!isMainWindow) return out;
    for (const [kind, label, icon, alias] of VIEWS) {
      add({ id: `v:${kind}`, group: "Go to", label, icon, alias, run: () => goTo({ kind } as MailboxView) });
    }
    // Split Inbox: its tabs, adding the selected sender to a split, Get to zero.
    for (const c of splitPaletteCommands()) add(c);
    // Smart views that are on (Settings → Views), in sidebar order.
    for (const it of sidebarSmartItems(smartViews)) {
      const d = it.custom ? null : smartDef(it.key);
      add({
        id: `sv:${it.key}`,
        group: "Views",
        label: `Go to ${it.label}`,
        meta: it.custom ? it.custom.query : undefined,
        icon: it.icon,
        alias: ["view smart", ...(d?.keywords ?? ["saved search pinned"])].join(" "),
        run: () => goTo(it.view),
      });
    }
    add({
      id: "sv:manage",
      group: "Views",
      label: smartViews.shown.length ? "Manage views…" : "Add views to the sidebar…",
      meta: "Receipts, Travel, Packages, Bills…",
      icon: "list",
      alias: "smart views sidebar receipts travel packages bills subscriptions settings",
      run: () => openSettings("views"),
    });
    const labelSeen = new Set<string>();
    for (const l of labels) {
      if (l.kind !== "user" || labelSeen.has(`${l.accountId}/${l.id}`)) continue;
      labelSeen.add(`${l.accountId}/${l.id}`);
      const acc = accounts.find((a) => a.id === l.accountId);
      const dup = labels.filter((x) => x.kind === "user" && x.name === l.name).length > 1;
      out.push({
        id: `l:${l.accountId}/${l.id}`,
        group: "Labels",
        label: l.name,
        meta: dup && acc ? accountName(acc, accounts) : undefined,
        dotTone: toneForColor(l.color),
        alias: "label",
        run: () => {
          if (dup) switchAccount(l.accountId);
          goTo({ kind: "label", labelId: l.id });
        },
      });
    }
    profiles.forEach((p, i) =>
      add({
        id: `prof:${p.id}`,
        group: "Profiles",
        label: `Switch to profile: ${p.name}`,
        meta: `${p.accountIds.length} ${p.accountIds.length === 1 ? "account" : "accounts"}`,
        keys: profileKeys(i),
        dotTone: accountTone(p.color),
        alias: "profile group workspace",
        run: () => switchProfile(p.id),
      }),
    );
    add({
      id: "prof:manage",
      group: "Profiles",
      label: "Manage profiles…",
      meta: "Group accounts by company or role",
      icon: "settings",
      alias: "profile group accounts settings",
      run: () => openSettings("profiles"),
    });
    add({
      id: "acc:all",
      group: "Accounts",
      label: "All accounts",
      meta: "Unified inbox",
      icon: "users",
      run: () => switchProfile(null),
    });
    accounts.forEach((a, i) =>
      add({
        id: `acc:${a.id}`,
        group: "Accounts",
        label: `Switch to ${accountName(a, accounts)}`,
        meta: accountName(a, accounts) === a.email ? undefined : a.email,
        keys: i < 9 ? `alt+${i + 1}` : undefined,
        dotTone: accountTone(a.color),
        alias: "account",
        run: () => switchAccount(a.id),
      }),
    );
    add({
      id: "set:theme",
      group: "Settings",
      label: "Switch theme",
      meta: "Light · Dark",
      icon: getUi().theme === "dark" ? "sun" : "moon",
      alias: "dark light mode appearance",
      run: () => setUi((s) => ({ theme: s.theme === "dark" ? "light" : "dark" })),
    });
    add({
      id: "set:welcome",
      group: "Settings",
      label: "Set up Penguin…",
      meta: "Search by meaning, AI, how much mail, look, keys",
      icon: "sliders",
      alias: "welcome onboarding setup preferences first run",
      run: openWelcome,
    });
    add({ id: "set:shortcuts", group: "Settings", label: "Keyboard shortcuts", keys: "?", icon: "keyboard", alias: "help keys", run: () => setUi({ overlay: "shortcuts" }) });
    add({ id: "set:sync", group: "Settings", label: "Sync now", meta: "Fetch new mail for every account", icon: "refresh", run: () => void api.syncNow() });
    add(
      isDemo
        ? { id: "set:demo", group: "Settings", label: "Exit demo mode", meta: "Back to your mail", icon: "logout", alias: "demo screenshots fake", run: () => void setDemoMode(false) }
        : { id: "set:demo", group: "Settings", label: "Enter demo mode", meta: "Fictional mail, for screenshots", icon: "eye", alias: "demo screenshots fake", run: () => void setDemoMode(true) },
    );
    add({
      id: "set:guide",
      group: "Settings",
      label: "Google setup guide",
      meta: "Opens in your browser",
      icon: "external",
      alias: "oauth help docs",
      run: () => void api.openExternal("https://github.com/justingluska/penguin/blob/main/docs/google-setup.md"),
    });
    return out;
  }, [shortcuts, labels, accounts, context, profiles, smartViews, instantReplies, writerReady, splitSettings]);

  const list = useMemo<Cmd[]>(() => {
    const text = q.trim();
    let base: Cmd[];
    if (!text) {
      // Browsing: a few context actions up top, and no list-movement keys
      // (j/k, tabs); those are one keystroke already and still match by name.
      base = all
        .filter((c) => !/^k:(nav|tab|msg|ctx|split)\./.test(c.id))
        .map((c) => {
          const i = SUGGESTED.indexOf(c.id);
          return i === -1 ? c : { ...c, group: "Suggested" };
        })
        .sort((a, b) => groupRank(a.group) - groupRank(b.group) || SUGGESTED.indexOf(a.id) - SUGGESTED.indexOf(b.id));
    } else {
      const lower = text.toLowerCase();
      base = all
        .map((c) => ({ c, s: Math.max(fuzzyScore(text, c.label), `${c.group} ${c.alias ?? ""}`.toLowerCase().includes(lower) ? 60 : 0) }))
        .filter((x) => x.s > 0)
        .sort((a, b) => b.s - a.s)
        .slice(0, 8)
        .map((x) => ({ ...x.c, group: "Best matches" }));
      // Search opens in the main window's search overlay.
      if (!isMainWindow) return base;
      for (const a of matchPeople(people, text, new Set(), 3)) {
        base.push({
          id: `p:${a.email}`,
          group: "People",
          label: `Mail from ${a.name ?? a.email}`,
          meta: a.email,
          icon: "user",
          run: () => openSearch(`from:${a.email}`),
        });
      }
      base.push({ id: "search", group: "Search", label: `Search mail for “${text}”`, icon: "search", keys: "mod+enter", run: () => openSearch(text) });
    }
    return base;
  }, [q, all, people]);

  const selIdx = Math.min(sel, Math.max(0, list.length - 1));
  useEffect(() => setSel(0), [q]);
  useEffect(() => {
    document.querySelector(`[data-cid="${CSS.escape(list[selIdx]?.id ?? "")}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selIdx, list]);

  function runCmd(c: Cmd) {
    setUi({ overlay: null });
    c.run();
  }

  function onKey(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Escape") {
      e.preventDefault();
      if (q) setQ("");
      else setUi({ overlay: null });
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp" || (e.ctrlKey && (e.key === "n" || e.key === "p"))) {
      e.preventDefault();
      if (!list.length) return;
      const d = e.key === "ArrowDown" || e.key === "n" ? 1 : -1;
      setSel((selIdx + d + list.length) % list.length);
    } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      if (q.trim() && isMainWindow) {
        setUi({ overlay: null });
        openSearch(q.trim());
      }
    } else if (e.key === "Enter") {
      e.preventDefault();
      const c = list[selIdx];
      if (c) runCmd(c);
    } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      setUi({ overlay: null });
    }
  }

  const scope = accounts.find((a) => a.id === accountFilter);
  let lastGroup = "";
  return (
    <>
      <div className="scrim soft" onMouseDown={() => setUi({ overlay: null })} />
      <div className="overlay-host cx-host" onMouseDown={(e) => e.target === e.currentTarget && setUi({ overlay: null })}>
        <section className="palette panel" role="dialog" aria-label="Command palette">
          <div className="pal-input">
            <Icon name="command" size="sm" className="faint" />
            <input
              ref={inputRef}
              className="cx-input"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={onKey}
              placeholder="Type a command, a person, or search mail…"
              spellCheck={false}
              autoComplete="off"
              aria-label="Command"
            />
            <span className="chip pal-scope" title="Palette scope">
              {!scope && profile ? (
                <>
                  <i className={`dot dot-sm t-${accountTone(profile.color)}`} />
                  {profile.name}
                </>
              ) : scope ? (
                <>
                  <i className={`dot dot-sm t-${accountTone(scope.color)}`} />
                  {accountName(scope, accounts)}
                </>
              ) : (
                <>
                  {accounts.slice(0, 3).map((a, i) => (
                    <i key={a.id} className={`dot dot-sm t-${accountTone(a.color)}`} style={i ? { marginLeft: -2 } : undefined} />
                  ))}
                  All accounts
                </>
              )}
            </span>
          </div>

          <div className="pal-list" role="listbox">
            {list.length === 0 && <div className="cx-none">Type to search commands and mail.</div>}
            {list.map((c, i) => {
              const head = c.group !== lastGroup ? <div className="pal-group">{c.group}</div> : null;
              lastGroup = c.group;
              return (
                <div key={c.id} style={{ display: "contents" }}>
                  {head}
                  <a
                    data-cid={c.id}
                    role="option"
                    aria-selected={i === selIdx}
                    className={`menu-item${i === selIdx ? " active" : ""}`}
                    onMouseMove={() => i !== selIdx && setSel(i)}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => runCmd(c)}
                  >
                    {c.dotTone ? <i className={`dot t-${c.dotTone} pal-dot`} /> : <Icon name={c.icon ?? "command"} size="sm" />}
                    <span className="cx-label">{highlight(c.label, q.trim())}</span>
                    {c.meta && <span className="pal-meta truncate">{c.meta}</span>}
                    {c.keys && <Keys keys={c.keys} then={false} />}
                  </a>
                </div>
              );
            })}
          </div>

          <footer className="pal-foot">
            <span className="hint faint keys-always">
              <Keys keys="mod+k" /> opens this anywhere
            </span>
            <span className="grow" />
            <span className="hint">
              <span className="kbd-group">
                <span className="kbd">↑</span>
                <span className="kbd">↓</span>
              </span>
              Select
            </span>
            <span className="hint">
              <span className="kbd">↵</span>Run
            </span>
            <span className="hint">
              <span className="kbd">Esc</span>Close
            </span>
          </footer>
        </section>
      </div>
    </>
  );
}

/** `when` guards run as-is; a throwing predicate shouldn't hide the command. */
function safeWhen(s: Shortcut): boolean {
  try {
    return s.when!();
  } catch {
    return true;
  }
}

function groupRank(g: string) {
  const i = GROUP_ORDER.indexOf(g);
  return i === -1 ? GROUP_ORDER.length : i;
}

function highlight(label: string, q: string): ReactNode {
  if (!q) return label;
  const idx = label.toLowerCase().indexOf(q.toLowerCase());
  if (idx === -1) return label;
  return (
    <>
      {label.slice(0, idx)}
      <b className="cx-hl">{label.slice(idx, idx + q.length)}</b>
      {label.slice(idx + q.length)}
    </>
  );
}
