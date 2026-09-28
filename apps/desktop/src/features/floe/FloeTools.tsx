// What the list header adds in Floe mode, where there is no sidebar: the
// view's count beside the title, then search, compose and the ⌘K menu (views,
// labels, everything else) on the left of the list's own buttons, and your
// photo (Settings) on the right. The header itself is the list's
// (features/inbox/ThreadList); these slide in and out with the mode.
import { useUi, setUi } from "../../lib/ui";
import { num } from "../../lib/format";
import { isMac } from "../../lib/keyboard";
import { useSetting } from "../../lib/settings";
import { Icon } from "../../components/Icon";
import { accountInScope, list, meta } from "../../app/store";
import { openCompose } from "../../app/actions";
import { useKeyTip } from "../../lib/shortcutHints";
import { meName } from "../../lib/me";
import { Avatar } from "../../components/Identity";
import { openSettings } from "../settings/state";
import { Keys } from "../../components/Kbd";
import { currentSplit } from "../../app/store";

const MOD = isMac ? "⌘" : "Ctrl+";

/** Unread in the current view where Gmail keeps a count, else the loaded total. */
function useCount(): string | null {
  const view = useUi((s) => s.view);
  const labels = meta.use((m) => m.labels);
  const items = list.use((l) => l.items.length);
  const hasMore = list.use((l) => l.hasMore);
  // Re-count when the scope changes.
  useUi((s) => `${s.accountFilter}|${s.profileId}`);
  useSetting("profiles");
  useSetting("hiddenFromAll");
  const labelId = view.kind === "inbox" ? "INBOX" : view.kind === "label" ? view.labelId : null;
  if (labelId) {
    let n = 0;
    for (const l of labels) if (l.id === labelId && accountInScope(l.accountId)) n += l.unreadCount ?? 0;
    return n > 0 ? num(n) : null;
  }
  return items > 0 ? `${num(items)}${hasMore ? "+" : ""}` : null;
}

/** The count beside the title (Floe only; the Split Inbox's tabs carry their own). */
export function FloeCount() {
  const count = useCount();
  const splits = useSetting("inboxTabs");
  useUi((s) => s.view.kind);
  if (splits && currentSplit() !== null) return null;
  return count ? <span className="floe-count tnum">{count}</span> : null;
}

/**
 * The fly-through keys under the Floe list: the loop in one line (move,
 * open, done, snooze, reply later, next split, everything else). Hidden
 * with "Show keyboard shortcut hints" off, and while rows are selected
 * (the selection bar sits there).
 */
export function FloeKeys() {
  const splits = useSetting("inboxTabs");
  const view = useUi((s) => s.view.kind);
  const keys: [string, string][] = [
    ["j k", "move"],
    ["enter", "open"],
    ["e", "done"],
    ["h", "snooze"],
    ["y", "reply later"],
    ...(splits && view === "inbox" ? ([["tab", "next split"]] as [string, string][]) : []),
    ["mod+k", "everything"],
  ];
  return (
    <div className="floe-keys" aria-hidden="true">
      {keys.map(([k, what]) => (
        <span key={k} className="floe-key">
          <Keys keys={k} then={false} />
          {what}
        </span>
      ))}
    </div>
  );
}

/** Search, compose, ⌘K; inert and collapsed outside Floe. */
export function FloeTools({ on }: { on: boolean }) {
  const tip = useKeyTip();
  return (
    <div className="floe-tools" inert={!on} aria-hidden={!on}>
      <button className="btn btn-ghost btn-icon" title={tip("Search", "/")} aria-label="Search" onClick={() => setUi({ overlay: "search", searchPrefill: null })}>
        <Icon name="search" size="sm" />
      </button>
      <button className="btn btn-ghost btn-icon" title={tip("New message", "C")} aria-label="New message" onClick={() => openCompose("new")}>
        <Icon name="compose" size="sm" />
      </button>
      <button
        className="btn btn-ghost btn-icon"
        title={tip("Views, labels and commands", `${MOD}K`)}
        aria-label="Views, labels and commands"
        onClick={() => setUi({ overlay: "command" })}
      >
        <Icon name="command" size="sm" />
      </button>
      <span className="floe-sep" />
    </div>
  );
}

/** You (Settings → You): your photo; opens Settings at You. Collapsed outside Floe. */
export function FloeMe({ on }: { on: boolean }) {
  const accounts = meta.use((m) => m.accounts);
  const acct = accounts.find((a) => a.displayName) ?? accounts[0];
  if (!acct) return null;
  const name = meName(accounts) ?? acct.email;
  return (
    <div className="floe-me-slot" inert={!on} aria-hidden={!on}>
      <button
        className="btn btn-ghost btn-icon floe-me"
        title={`${name} — Settings`}
        aria-label={`${name}: Settings`}
        onClick={() => openSettings("you")}
      >
        <Avatar person={{ name, email: acct.email }} size="sm" tone="gray" />
      </button>
    </div>
  );
}
