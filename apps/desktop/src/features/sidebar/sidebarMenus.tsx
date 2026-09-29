// Right-click menus for the sidebar: accounts, labels, and profiles in the
// account switcher. OWNER: menus agent. Rename happens in place: the row
// swaps its name for <RenameField>, driven by the small `renaming` store
// here so Sidebar.tsx only has to ask "am I being renamed?".
import { memo, useEffect, useRef, useState, useSyncExternalStore, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import type { Account, Label, Profile, ThreadRef } from "../../lib/types";
import { api, asCommandError } from "../../lib/api";
import { getUi, setUi } from "../../lib/ui";
import { LABEL_COLORS } from "../../lib/labelColors";
import { num } from "../../lib/format";
import { toast } from "../../components/Toast";
import { copyText } from "../../lib/clipboard";
import { showTargetMenu, type MenuEntries } from "../../components/ContextMenu";
import { AccountAvatar, AccountBar, AccountDot, LabelSwatch, accountName } from "../../components/Identity";
import { loadLabels, meta } from "../../app/store";
import { goTo, switchAccount } from "../../app/shortcuts";
import { getProfiles, saveProfiles, switchProfile } from "../../app/profiles";
import { markRead } from "../../app/actions";
import {
  MAX_NICKNAME,
  gmailInboxUrl,
  moveAccountBy,
  patchAccount,
  removeAccount,
  setShownInAll,
  useShownInAll,
} from "../../app/accountActions";
import { canMove } from "../../app/accountOrder";
import { currentSettings } from "../../lib/settings";
import { Icon } from "../../components/Icon";
import { colorSubmenu } from "../../components/ColorGrid";
import { confirmAction } from "../../app/confirm";
import { openSettings } from "../settings/state";
import { canHide, copySyncDetails, hideSyncAlerts, runSyncFix, syncFixFor, type SyncFix } from "../settings/syncFix";
import { activeHide, syncHealth } from "../../lib/syncHealth";
import { getSyncHides, unhideSync } from "../../lib/syncHides";
import { openLogViewer } from "../logs/LogViewer";
import { UnreadBadge } from "./UnreadBadge";
import { labelEditing, mailboxUnaffected, serviceName } from "../../lib/capabilities";
import "./sidebarMenus.css";

// ---------------------------------------------------------------------------
// In-place rename
// ---------------------------------------------------------------------------
let renaming: string | null = null;
const subs = new Set<() => void>();
function setRenaming(key: string | null) {
  renaming = key;
  subs.forEach((f) => f());
}

export const renameKey = {
  account: (id: string) => `account:${id}`,
  label: (accountId: string, id: string) => `label:${accountId}:${id}`,
  profile: (id: string) => `profile:${id}`,
  /** A pinned search in the sidebar's views (features/smart). */
  view: (id: string) => `view:${id}`,
};

/** Show a row's rename field (rows outside this file: the sidebar's pinned searches). */
export function startRename(key: string) {
  setRenaming(key);
}

/** True while the row with this key shows its rename field. */
export function useRenaming(key: string): boolean {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => renaming === key,
  );
}

/**
 * The text field that stands in for a row's name. Enter saves, Esc or an
 * unchanged value cancels, blur saves. `onSave` gets the trimmed value.
 */
export function RenameField({
  initial,
  placeholder,
  maxLength,
  label,
  className,
  onSave,
}: {
  initial: string;
  placeholder?: string;
  maxLength?: number;
  label: string;
  className?: string;
  onSave: (value: string) => void;
}) {
  const [draft, setDraft] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  const ended = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const end = (save: boolean) => {
    if (ended.current) return;
    ended.current = true;
    setRenaming(null);
    const v = draft.trim().replace(/\s+/g, " ");
    if (save && v !== initial.trim()) onSave(v);
  };
  return (
    <input
      ref={ref}
      className={"sb-rename" + (className ? " " + className : "")}
      value={draft}
      maxLength={maxLength}
      placeholder={placeholder}
      aria-label={label}
      spellCheck={false}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => end(true)}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        // The field owns every key while it's open (no triage shortcuts).
        e.stopPropagation();
        if (e.key === "Enter") {
          e.preventDefault();
          end(true);
        } else if (e.key === "Escape") {
          e.preventDefault();
          end(false);
        }
      }}
    />
  );
}

// ---------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------
/**
 * `visible`: the account list the menu was opened from (every account, or a
 * profile's), for Move up / Move down.
 */
export function accountMenu(a: Account, visible: string[] = meta.get().accounts.map((x) => x.id)): MenuEntries {
  const all = meta.get().accounts.map((x) => x.id);
  const movable = visible.filter((id) => all.includes(id)).length > 1;
  const ui = getUi();
  const status = meta.get().sync[a.id];
  // A failure still retrying quietly can be retried now too.
  const retrying = syncHealth(status) === "retrying";
  const fix: SyncFix | null =
    syncFixFor(status) ?? (retrying ? { kind: "retry", label: "Retry now", message: "", detail: status?.error ?? null } : null);
  const hidden = activeHide(getSyncHides(), status);
  const profiles = getProfiles();
  const filtered = ui.accountFilter === a.id;
  const inAll = !currentSettings().hiddenFromAll.includes(a.id);
  return [
    filtered
      ? { label: "Show all accounts", icon: "mails", onSelect: () => switchAccount(null) }
      : { label: "Show only this account", icon: "inbox", onSelect: () => switchAccount(a.id) },
    inAll
      ? { label: "Hide from All Inboxes", icon: "eyeoff", onSelect: () => void hideFromAll(a) }
      : { label: "Show in All Inboxes", icon: "eye", onSelect: () => void setShownInAll(a, true) },
    { type: "separator" },
    { label: "Rename…", icon: "pencil", onSelect: () => setRenaming(renameKey.account(a.id)) },
    {
      label: "Color",
      icon: "palette",
      ...colorSubmenu(a.color, (hex) => void patchAccount(a, { color: hex })),
    },
    profiles.length > 0 && {
      label: "Profiles",
      icon: "users",
      submenu: () => [
        ...profiles.map((p) => {
          const member = p.accountIds.includes(a.id);
          return {
            label: p.name,
            checked: member,
            onSelect: () => void toggleProfileMember(p, a, !member),
          };
        }),
        { type: "separator" as const },
        { label: "Manage profiles…", onSelect: () => openSettings("profiles") },
      ],
    },
    // The keyboard way to reorder (the sidebar also drags).
    movable && {
      label: "Move up",
      icon: "arrowup",
      disabled: !canMove(all, visible, a.id, -1),
      onSelect: () => void moveAccountBy(a, visible, -1),
    },
    movable && {
      label: "Move down",
      icon: "arrowdown",
      disabled: !canMove(all, visible, a.id, 1),
      onSelect: () => void moveAccountBy(a, visible, 1),
    },
    { type: "separator" },
    fix
      ? {
          label: fix.label,
          icon: fix.kind === "reconnect" ? "lock" : "refresh",
          onSelect: () => void runSyncFix(fix, a.id, { announce: true }),
        }
      : {
          label: "Check for new mail",
          icon: "refresh",
          onSelect: () => void api.syncNow().catch((e) => toast({ tone: "error", message: asCommandError(e).message })),
        },
    fix && { label: "Copy sync details", icon: "copy", onSelect: () => void copySyncDetails([a.id]) },
    hidden
      ? { label: "Show sync alert again", icon: "eye", onSelect: () => unhideSync(a.id, hidden.kind) }
      : canHide(status) && { label: "Hide sync alert for 6 hours", icon: "eyeoff", onSelect: () => hideSyncAlerts([a.id]) },
    fix && { label: "Open logs", icon: "file", onSelect: openLogViewer },
    // Reconnect is always available (e.g. to grant a scope again), not only when sync asks.
    fix?.kind !== "reconnect" && {
      label: "Sign in again…",
      icon: "lock",
      onSelect: () => void runSyncFix({ kind: "reconnect", label: "Reconnect", message: "", detail: null }, a.id),
    },
    // Also for Gmail quick setup (IMAP), whose inbox is still at mail.google.com.
    serviceName(a) === "Gmail" && { label: "Open in Gmail", icon: "external", onSelect: () => void api.openExternal(gmailInboxUrl(a.email)) },
    { label: "Copy address", icon: "copy", onSelect: () => void copyText(a.email, "Address copied") },
    { type: "separator" },
    { label: "Account settings…", icon: "settings", onSelect: () => openSettings("accounts") },
    {
      label: "Remove account…",
      icon: "logout",
      danger: true,
      onSelect: async () => {
        const ok = await confirmAction({
          title: `Remove ${a.email}?`,
          body: `This signs out and deletes its local mail and index from this Mac. ${mailboxUnaffected(a)}`,
          confirm: "Remove",
        });
        if (ok) await removeAccount(a);
      },
    },
  ];
}

/** Leave the account out of All accounts, with an Undo. */
async function hideFromAll(a: Account) {
  if (!(await setShownInAll(a, false))) return;
  toast({
    kind: "action",
    message: `${accountName(a, meta.get().accounts)} is hidden from All Inboxes`,
    detail: "It keeps syncing. Click it in the sidebar to see its mail.",
    key: `hide-all-${a.id}`,
    action: { label: "Undo", run: () => void setShownInAll(a, true) },
  });
}

export function renameAccount(a: Account, value: string) {
  void patchAccount(a, { nickname: value || null });
}
export { MAX_NICKNAME };

async function toggleProfileMember(p: Profile, a: Account, on: boolean) {
  // Profiles are saved as a whole list; read the latest so quick toggles stack.
  const next = getProfiles().map((x) =>
    x.id !== p.id ? x : { ...x, accountIds: on ? [...x.accountIds.filter((id) => id !== a.id), a.id] : x.accountIds.filter((id) => id !== a.id) },
  );
  try {
    await saveProfiles(next);
  } catch (e) {
    toast({ tone: "error", message: `Couldn't update ${p.name}: ${asCommandError(e).message}` });
  }
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------
async function labelFail(what: string, e: unknown) {
  toast({ tone: "error", message: `Couldn't ${what}`, detail: asCommandError(e).message });
}

async function updateLabel(l: Label, patch: Parameters<typeof api.updateLabel>[2], what: string) {
  try {
    await api.updateLabel(l.accountId, l.id, patch);
    await loadLabels();
  } catch (e) {
    void labelFail(what, e);
  }
}

/** Every unread conversation carrying the label in its account, paged. */
async function unreadInLabel(l: Label): Promise<ThreadRef[]> {
  const out: ThreadRef[] = [];
  let before: number | null = null;
  for (;;) {
    const page = await api.listThreads({ view: { kind: "label", labelId: l.id }, tab: null, accountId: l.accountId, limit: 1000, before });
    for (const t of page) if (t.unread) out.push({ accountId: t.accountId, threadId: t.threadId });
    if (page.length < 1000 || page[page.length - 1].lastDate === before) break;
    before = page[page.length - 1].lastDate;
  }
  return out;
}

async function markLabelRead(l: Label) {
  try {
    const refs = await unreadInLabel(l);
    if (refs.length === 0) return;
    markRead(refs);
    toast({ message: `Marked ${refs.length === 1 ? "1 conversation" : `${refs.length} conversations`} as read` });
  } catch (e) {
    void labelFail("mark the label read", e);
  }
}

export function renameLabel(l: Label, value: string) {
  if (!value) return;
  void updateLabel(l, { name: value }, "rename the label");
}

export function labelMenu(l: Label): MenuEntries {
  const account = meta.get().accounts.find((a) => a.id === l.accountId);
  const where = account ? ` for ${account.email}` : "";
  // Rename, hide and delete need labelEdit (Gmail); colors need labelColors.
  // IMAP folders and Microsoft categories get Open and Mark all as read only.
  const can = labelEditing(account);
  return [
    {
      label: "Open",
      icon: "tag",
      onSelect: () => {
        goTo({ kind: "label", labelId: l.id });
        // Several accounts can have a label with this id; show this one's.
        if (getUi().accountFilter && getUi().accountFilter !== l.accountId) setUi({ accountFilter: l.accountId });
      },
    },
    {
      label: "Mark all as read",
      icon: "check",
      disabled: l.unreadCount ? false : "Nothing unread",
      onSelect: () => void markLabelRead(l),
    },
    { type: "separator" },
    can.edit && { label: "Rename…", icon: "pencil", onSelect: () => setRenaming(renameKey.label(l.accountId, l.id)) },
    can.colors && {
      label: "Color",
      icon: "palette",
      submenu: () => [
        ...LABEL_COLORS.map((c) => ({
          label: c.name,
          lead: <span className="cm-swatch" style={{ ["--sw" as string]: c.hex }} />,
          checked: (l.color ?? "").toLowerCase() === c.hex.toLowerCase(),
          onSelect: () => void updateLabel(l, { color: c.hex }, "change the color"),
        })),
        { type: "separator" as const },
        { label: "No color", checked: !l.color, onSelect: () => void updateLabel(l, { color: null }, "remove the color") },
      ],
    },
    can.edit &&
      (l.hidden
        ? { label: "Show in label list", icon: "eye", onSelect: () => void updateLabel(l, { hidden: false }, "show the label") }
        : { label: "Hide from label list", icon: "eyeoff", onSelect: () => void updateLabel(l, { hidden: true }, "hide the label") }),
    { type: "separator" },
    can.edit && {
      label: "Delete label…",
      icon: "trash",
      danger: true,
      onSelect: async () => {
        const ok = await confirmAction({
          title: `Delete the label “${l.name}”?`,
          body: `This deletes it in Gmail${where}. Conversations with it stay where they are and keep their other labels.`,
          confirm: "Delete label",
        });
        if (!ok) return;
        try {
          await api.deleteLabel(l.accountId, l.id);
          const v = getUi().view;
          if (v.kind === "label" && v.labelId === l.id) goTo({ kind: "inbox" });
          await loadLabels();
          toast({ message: `Deleted “${l.name}”` });
        } catch (e) {
          void labelFail("delete the label", e);
        }
      },
    },
  ];
}

// ---------------------------------------------------------------------------
// Profiles (account switcher)
// ---------------------------------------------------------------------------
export function renameProfile(p: Profile, value: string) {
  if (!value) return;
  const next = getProfiles().map((x) => (x.id === p.id ? { ...x, name: value } : x));
  saveProfiles(next).catch((e) => toast({ tone: "error", message: `Couldn't rename: ${asCommandError(e).message}` }));
}

export function profileMenu(p: Profile, active: boolean): MenuEntries {
  return [
    { label: active ? "Current profile" : "Switch to profile", icon: "users", disabled: active, onSelect: () => switchProfile(p.id) },
    { type: "separator" },
    { label: "Rename…", icon: "pencil", onSelect: () => setRenaming(renameKey.profile(p.id)) },
    {
      label: "Color",
      icon: "palette",
      ...colorSubmenu(p.color, (hex) =>
        void saveProfiles(getProfiles().map((x) => (x.id === p.id ? { ...x, color: hex } : x))).catch((e) =>
          toast({ tone: "error", message: asCommandError(e).message }),
        ),
      ),
    },
    { label: "Edit profile…", icon: "settings", onSelect: () => openSettings("profiles") },
    { type: "separator" },
    {
      label: "Delete profile…",
      icon: "trash",
      danger: true,
      onSelect: async () => {
        const ok = await confirmAction({
          title: `Delete the profile “${p.name}”?`,
          body: "Its accounts stay signed in and keep their mail. Only the grouping goes away.",
          confirm: "Delete profile",
        });
        if (!ok) return;
        if (getUi().profileId === p.id) switchProfile(null);
        saveProfiles(getProfiles().filter((x) => x.id !== p.id)).catch((e) =>
          toast({ tone: "error", message: `Couldn't delete: ${asCommandError(e).message}` }),
        );
      },
    },
  ];
}

// ---------------------------------------------------------------------------
// Rows (rendered by Sidebar.tsx): the nav-item markup plus its menu and the
// in-place rename.
// ---------------------------------------------------------------------------
// Memoized: the sidebar re-renders on every unread-count change, and label
// objects that didn't change keep their identity (store.setLabels).
export const SidebarLabelRow = memo(function SidebarLabelRow({ label: l, active, dupAccount }: { label: Label; active: boolean; dupAccount: Account | null }) {
  const editing = useRenaming(renameKey.label(l.accountId, l.id));
  if (editing) {
    return (
      <div className="nav-item is-renaming">
        <LabelSwatch color={l.color} />
        <RenameField initial={l.name} label={`Rename label ${l.name}`} maxLength={225} onSave={(v) => renameLabel(l, v)} />
      </div>
    );
  }
  return (
    <button
      className={"nav-item" + (active ? " active" : "") + (l.hidden ? " is-hidden-label" : "")}
      onClick={() => goTo({ kind: "label", labelId: l.id })}
      onContextMenu={(e) => showTargetMenu(e, labelMenu(l), { label: `Label ${l.name}` })}
      title={l.hidden ? "Hidden from the label list" : undefined}
    >
      <LabelSwatch color={l.color} />
      <span className="truncate">{l.name}</span>
      {dupAccount && <AccountDot color={dupAccount.color} size="sm" className="lbl-acct" />}
      {l.unreadCount ? <span className="count">{num(l.unreadCount)}</span> : null}
    </button>
  );
});

/** Wiring from the list's drag-to-reorder (components/useDragReorder.ts). */
export interface RowDrag {
  ref: (el: HTMLElement | null) => void;
  onPointerDown: (e: ReactPointerEvent) => void;
  style: CSSProperties | undefined;
  /** This row is the one being dragged. */
  lifted: boolean;
}

export function SidebarAccountRow({
  account: a,
  accounts,
  visible,
  active,
  unread,
  drag,
}: {
  account: Account;
  accounts: Account[];
  /** The ids the list shows, in order (for Move up / Move down). */
  visible: string[];
  active: boolean;
  unread: number;
  drag?: RowDrag;
}) {
  const editing = useRenaming(renameKey.account(a.id));
  const inAll = useShownInAll(a.id);
  if (editing) {
    return (
      <div className="nav-item is-renaming">
        <AccountAvatar account={a} accounts={accounts} />
        <AccountBar color={a.color} />
        <RenameField
          initial={a.nickname ?? ""}
          placeholder={accountName({ ...a, nickname: null }, accounts)}
          maxLength={MAX_NICKNAME}
          label={`Nickname for ${a.email}`}
          onSave={(v) => renameAccount(a, v)}
        />
      </div>
    );
  }
  return (
    <button
      ref={drag?.ref}
      className={"nav-item" + (active ? " active" : "") + (inAll ? "" : " is-hidden-all") + (drag?.lifted ? " is-lifted" : "")}
      style={drag?.style}
      onPointerDown={drag?.onPointerDown}
      // The avatar image would otherwise start a native drag and cancel ours.
      onDragStart={(e) => e.preventDefault()}
      onClick={() => switchAccount(active ? null : a.id)}
      onContextMenu={(e) => showTargetMenu(e, accountMenu(a, visible), { label: a.email })}
      title={inAll ? a.email : `${a.email}
Hidden from All Inboxes: its mail shows only here`}
    >
      <AccountAvatar account={a} accounts={accounts} />
      <AccountBar color={a.color} />
      <span className="truncate">{accountName(a, accounts)}</span>
      {!inAll && (
        <span className="sb-hidden-all" role="img" aria-label="Hidden from All Inboxes">
          <Icon name="eyeoff" size="xs" />
        </span>
      )}
      {/* A hidden account stays quiet: no unread count to pull you in. */}
      {inAll && <UnreadBadge n={unread} />}
    </button>
  );
}
