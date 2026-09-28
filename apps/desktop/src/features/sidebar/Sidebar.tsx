// Left sidebar: account switcher, compose/search, views, calendar, accounts, labels,
// and the signed-in footer. Matches design/01-inbox.html.
import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Account, Label, MailboxView, Profile } from "../../lib/types";
import { setUi, useUi } from "../../lib/ui";
import { showMockBadge } from "../../lib/api";
import { num } from "../../lib/format";
import { accountTone } from "../../lib/accountColor";
import { setSectionHidden, toggleSection, useLayout, type SidebarSection } from "../../lib/layout";
import { Icon, type IconName } from "../../components/Icon";
import { Kbd, Keys } from "../../components/Kbd";
import { AccountAvatar, AccountBar, Avatar, accountName } from "../../components/Identity";
import { useAccountPhoto } from "../../lib/accountPhotos";
import { toast } from "../../components/Toast";
import { meta } from "../../app/store";
import { PROFILE_MOD, goTo, goToCalendar, profileKeys, switchAccount } from "../../app/shortcuts";
import { inboxUnread, switchProfile, useProfiles } from "../../app/profiles";
import { openCompose } from "../../app/actions";
import { SyncProgress } from "./SyncProgress";
import { UnreadBadge, badgeText } from "./UnreadBadge";
import { useTriageCounts } from "../triage/state";
import "../triage/triage.css";
import { openSettings } from "../settings/state";
import { PenguinMark } from "../../components/PenguinMark";
import { devParam } from "../search/dev";
import { openAddAccount } from "../onboarding/AddAccountModal";
import "./profiles.css";
import { RenameField, SidebarAccountRow, SidebarLabelRow, profileMenu, renameKey, renameProfile, useRenaming } from "./sidebarMenus";
import { showContextMenu } from "../../components/ContextMenu";
import { meName } from "../../lib/me";
import { useKeyTip } from "../../lib/shortcutHints";
import { isMac } from "../../lib/keyboard";
import { useDismiss } from "../../lib/dismiss";
import { useSnoozedCount } from "../snooze/state";
import { useSetting } from "../../lib/settings";
import { mailScope } from "../../app/allInboxes";
import { isFolderId } from "../../lib/capabilities";
import { moveAccountTo } from "../../app/accountActions";
import { useDragReorder } from "../../components/useDragReorder";
import { SmartNav } from "../smart/SmartNav";

interface ViewItem {
  view: MailboxView;
  label: string;
  icon: IconName;
  keys?: string;
  disabled?: string;
}

const VIEWS: ViewItem[] = [
  { view: { kind: "inbox" }, label: "Inbox", icon: "inbox", keys: "g i" },
  { view: { kind: "replyLater" }, label: "Reply Later", icon: "replyLater", keys: "g y" },
  { view: { kind: "followUp" }, label: "Follow up", icon: "followUp", keys: "g f" },
  { view: { kind: "starred" }, label: "Starred", icon: "star", keys: "g s" },
  { view: { kind: "snoozed" }, label: "Snoozed", icon: "snooze", keys: "g h" },
  { view: { kind: "sent" }, label: "Sent", icon: "send", keys: "g t" },
  { view: { kind: "drafts" }, label: "Drafts", icon: "draft", keys: "g d" },
  { view: { kind: "done" }, label: "Done", icon: "done", keys: "g e" },
  { view: { kind: "trash" }, label: "Trash", icon: "trash", keys: "g #" },
];

/** Conversations (not unread) in Reply Later / Follow up: an outlined capsule. */
function ItemsBadge({ n, what }: { n: number; what: string }) {
  const full = `${num(n)} ${n === 1 ? "conversation" : "conversations"} ${what}`;
  return (
    <span className="count badge-items" aria-label={full} title={full}>
      {badgeText(n)}
    </span>
  );
}

function sameView(a: MailboxView, b: MailboxView) {
  return a.kind === b.kind && (a.kind !== "label" || (b.kind === "label" && a.labelId === b.labelId));
}

function sumUnread(labels: Label[], id: string, accountId: string | null): number {
  let n = 0;
  for (const l of labels) if (l.id === id && (!accountId || l.accountId === accountId)) n += l.unreadCount ?? 0;
  return n;
}

export const Sidebar = memo(function Sidebar() {
  const view = useUi((s) => s.view);
  const onCalendar = useUi((s) => s.surface === "calendar");
  const accountFilter = useUi((s) => s.accountFilter);
  const profileId = useUi((s) => s.profileId);
  const accounts = meta.use((m) => m.accounts);
  const labels = meta.use((m) => m.labels);
  const profiles = useProfiles();
  const profile = profiles.find((p) => p.id === profileId) ?? null;
  const scopeIds = profile ? profile.accountIds : null;
  // In a profile the sidebar lists only its accounts, in the profile's order.
  const scopedAccounts = useMemo(
    () => (scopeIds ? scopeIds.map((id) => accounts.find((a) => a.id === id)).filter((a): a is Account => !!a) : accounts),
    [accounts, scopeIds],
  );
  // The accounts whose mail the views show: the profile's, or under All
  // accounts every account not hidden from All Inboxes (null = all).
  const hiddenFromAll = useSetting("hiddenFromAll");
  const accountIds = useMemo(() => accounts.map((a) => a.id), [accounts]);
  const mailIds = useMemo(
    () => mailScope({ accountFilter, profileScope: scopeIds, accountIds, hidden: hiddenFromAll }),
    [accountFilter, scopeIds, accountIds, hiddenFromAll],
  );
  const allIds = useMemo(
    () => mailScope({ accountFilter: null, profileScope: null, accountIds, hidden: hiddenFromAll }),
    [accountIds, hiddenFromAll],
  );

  const userLabels = useMemo(
    () =>
      labels
        .filter(
          (l) =>
            l.kind === "user" &&
            (!accountFilter || l.accountId === accountFilter) &&
            (!mailIds || mailIds.includes(l.accountId)),
        )
        .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" })),
    [labels, accountFilter, mailIds],
  );
  // Labels hidden from the list (Gmail's "hide in label list") sit behind a
  // "N hidden" toggle; the open label always shows.
  const [showHidden, setShowHidden] = useState(false);
  const hiddenCount = userLabels.filter((l) => l.hidden).length;
  const shownLabels = showHidden
    ? userLabels
    : userLabels.filter((l) => !l.hidden || (view.kind === "label" && view.labelId === l.id));
  // Label names that exist in more than one account get an account dot.
  const dupNames = useMemo(() => {
    const seen = new Map<string, number>();
    for (const l of userLabels) seen.set(l.name, (seen.get(l.name) ?? 0) + 1);
    return new Set([...seen].filter(([, n]) => n > 1).map(([k]) => k));
  }, [userLabels]);

  const labelsCollapsed = useLayout((s) => s.collapsedSections.includes("labels"));
  const accountsCollapsed = useLayout((s) => s.collapsedSections.includes("accounts"));
  // Hidden sections (Settings → General) aren't rendered at all.
  const labelsHidden = useLayout((s) => s.hiddenSections.includes("labels"));
  const accountsHidden = useLayout((s) => s.hiddenSections.includes("accounts"));
  const tip = useKeyTip();

  const triage = useTriageCounts(accountFilter, mailIds);
  const counts: Partial<Record<MailboxView["kind"], number>> = {
    inbox: accountFilter ? sumUnread(labels, "INBOX", accountFilter) : inboxUnread(labels, mailIds),
    replyLater: triage.replyLater,
    followUp: triage.followUp,
    drafts: undefined,
    snoozed: useSnoozedCount(accountFilter, mailIds),
  };

  return (
    <aside className="sidebar">
      <AccountSwitcher
        accounts={accounts}
        accountFilter={accountFilter}
        labels={labels}
        profiles={profiles}
        profile={profile}
        allIds={allIds}
        hiddenFromAll={hiddenFromAll}
      />
      <div className="sb-actions">
        <button
          className="btn btn-primary sb-compose"
          data-shortcut="compose.new"
          title={tip("New message", "C")}
          aria-label="New message (C)"
          onClick={() => openCompose("new")}
        >
          <Icon name="compose" size="sm" />
          <span className="sb-compose-label">New</span>
          <Kbd>C</Kbd>
        </button>
        {/* Its "/" cap goes with the other hints when they're off. */}
        <button
          className="btn btn-secondary sb-search"
          data-shortcut="search.open"
          title={tip("Search", isMac ? "/ or ⌘F" : "/ or Ctrl+F")}
          aria-label="Search"
          onClick={() => setUi({ overlay: "search", searchPrefill: null })}
        >
          <Icon name="search" size="sm" />
          <Kbd>/</Kbd>
        </button>
      </div>

      <div className="sb-scroll">
        <nav className="nav">
          {VIEWS.map((v) => {
            const active = !v.disabled && !onCalendar && sameView(view, v.view);
            const n = counts[v.view.kind];
            return (
              <button
                key={v.label}
                className={"nav-item" + (active ? " active" : "") + (v.disabled ? " is-disabled" : "")}
                // Shortcut coach: G then I for Inbox, … (only views with a key).
                data-shortcut={v.disabled ? undefined : `go.${v.view.kind}`}
                title={v.disabled ?? undefined}
                aria-disabled={v.disabled ? true : undefined}
                onClick={() => (v.disabled ? toast({ message: v.disabled }) : goTo(v.view))}
              >
                <Icon name={v.icon} />
                {v.label}
                {v.disabled ? (
                  <span className="count soon">Soon</span>
                ) : n ? (
                  // The capsule means unread; Snoozed counts waiting threads, a plain number;
                  // Reply Later and Follow up count conversations, an outlined capsule.
                  v.view.kind === "inbox" ? (
                    <UnreadBadge n={n} />
                  ) : v.view.kind === "replyLater" || v.view.kind === "followUp" ? (
                    <ItemsBadge n={n} what={v.view.kind === "replyLater" ? "to reply to" : "waiting on a reply"} />
                  ) : (
                    <span className="count">{n}</span>
                  )
                ) : v.keys ? (
                  <span className="hint">
                    <Keys keys={v.keys} then={false} />
                  </span>
                ) : null}
              </button>
            );
          })}
        </nav>

        {/* Smart views (Settings → Views): nothing until one is turned on. */}
        <SmartNav view={view} onCalendar={onCalendar} scope={accountFilter ? [accountFilter] : mailIds} />

        {/* Calendar is a different place, not a mail folder: its own group under a hairline. */}
        <nav className="nav nav-places">
          <button className={"nav-item" + (onCalendar ? " active" : "")} onClick={goToCalendar}>
            <Icon name="calendar" />
            Calendar
            <span className="hint">
              <Keys keys="g c" then={false} />
            </span>
          </button>
        </nav>

        {!accountsHidden && (
          <>
            <SectionHead id="accounts" label={profile ? `${profile.name} accounts` : "Accounts"} collapsed={accountsCollapsed}>
              <button
                className="btn btn-ghost btn-sm btn-icon"
                title="Add account"
                onClick={() => openAddAccount()}
              >
                <Icon name="plus" size="xs" />
              </button>
            </SectionHead>
            {!accountsCollapsed && (
              <SidebarAccounts shown={scopedAccounts} accounts={accounts} accountFilter={accountFilter} labels={labels} />
            )}
          </>
        )}

        {!labelsHidden && userLabels.length > 0 && (
          <>
            <SectionHead
              id="labels"
              // IMAP accounts have folders, not labels.
              label={userLabels.length > 0 && userLabels.every((l) => isFolderId(l.id)) ? "Folders" : "Labels"}
              collapsed={labelsCollapsed}
            />
            {!labelsCollapsed && (
            <nav className="nav">
              {shownLabels.map((l) => {
                const active = !onCalendar && view.kind === "label" && view.labelId === l.id && (accountFilter === null || accountFilter === l.accountId);
                const acct = accounts.find((a) => a.id === l.accountId);
                return (
                  <SidebarLabelRow
                    key={l.accountId + l.id}
                    label={l}
                    active={active}
                    dupAccount={dupNames.has(l.name) && acct ? acct : null}
                  />
                );
              })}
              {hiddenCount > 0 && (
                <button className="nav-item sb-hidden-toggle" onClick={() => setShowHidden((v) => !v)}>
                  <Icon name={showHidden ? "eyeoff" : "eye"} size="sm" />
                  {showHidden ? "Hide hidden labels" : `${hiddenCount} hidden`}
                </button>
              )}
            </nav>
            )}
          </>
        )}
      </div>

      <SidebarFooter accounts={accounts} scoped={scopedAccounts} />
    </aside>
  );
});

/**
 * The Accounts section: the accounts in the user's order (a profile's
 * accounts in a profile), dragged to reorder. A drag inside a profile only
 * moves its accounts relative to each other (app/accountOrder.ts).
 */
function SidebarAccounts({
  shown,
  accounts,
  accountFilter,
  labels,
}: {
  shown: Account[];
  accounts: Account[];
  accountFilter: string | null;
  labels: Label[];
}) {
  const ids = useMemo(() => shown.map((a) => a.id), [shown]);
  const { drag, listRef, rowRef, onPointerDown, rowStyle } = useDragReorder(ids, (id, to) => {
    const a = shown.find((x) => x.id === id);
    if (a) void moveAccountTo(a, ids, to);
  });
  return (
    <nav className={"nav sb-accounts reorder-list" + (drag ? " is-dragging" : "")} ref={listRef} aria-label="Accounts">
      {shown.map((a) => (
        <SidebarAccountRow
          key={a.id}
          account={a}
          accounts={accounts}
          visible={ids}
          active={accountFilter === a.id}
          unread={sumUnread(labels, "INBOX", a.id)}
          drag={{
            ref: rowRef(a.id),
            onPointerDown: (e) => onPointerDown(a.id, e),
            style: rowStyle(a.id),
            lifted: drag?.id === a.id,
          }}
        />
      ))}
      {drag && <div className="reorder-line" style={{ transform: `translateY(${drag.lineY}px)` }} aria-hidden="true" />}
    </nav>
  );
}

const SECTION_NAME: Record<SidebarSection, string> = { labels: "Labels", accounts: "Accounts" };

/** Take a section out of the sidebar; Settings → General (or Undo) brings it back. */
function hideSection(id: SidebarSection) {
  setSectionHidden(id, true);
  toast({
    kind: "action",
    message: `${SECTION_NAME[id]} hidden from the sidebar`,
    detail: "Show it again in Settings → General.",
    key: `sb-hide-${id}`,
    action: { label: "Undo", run: () => setSectionHidden(id, false) },
  });
}

/** Section header that collapses its list (persisted); extra controls go on the right. */
function SectionHead({
  id,
  label,
  collapsed,
  children,
}: {
  id: SidebarSection;
  label: string;
  collapsed: boolean;
  children?: ReactNode;
}) {
  return (
    <div
      className={"nav-section" + (collapsed ? " is-collapsed" : "")}
      onContextMenu={(e) =>
        showContextMenu(
          e,
          [
            { label: collapsed ? "Expand" : "Collapse", icon: collapsed ? "down" : "up", onSelect: () => toggleSection(id) },
            { label: "Hide section", icon: "eyeoff", onSelect: () => hideSection(id) },
            { type: "separator" },
            { label: "Sidebar settings…", icon: "settings", onSelect: () => openSettings("general") },
          ],
          { label: `${SECTION_NAME[id]} section` },
        )
      }
    >
      <button className="sec-toggle" aria-expanded={!collapsed} onClick={() => toggleSection(id)}>
        {label}
        <Icon name="down" size="xs" className="sec-chev" />
      </button>
      {children}
    </div>
  );
}

/** Colored square for a profile: its emoji, else its initial. */
export function ProfileTile({ profile, size }: { profile: Pick<Profile, "name" | "color" | "emoji">; size?: "sm" }) {
  return (
    <span className={`acct-tile profile-tile t-${accountTone(profile.color)}${size === "sm" ? " tile-sm" : ""}`} aria-hidden="true">
      {profile.emoji || profile.name.trim().slice(0, 1).toUpperCase() || "·"}
    </span>
  );
}

function AccountSwitcher({
  accounts,
  accountFilter,
  labels,
  profiles,
  profile,
  allIds,
  hiddenFromAll,
}: {
  accounts: Account[];
  accountFilter: string | null;
  labels: Label[];
  profiles: Profile[];
  profile: Profile | null;
  /** The accounts "All accounts" shows (null = every one). */
  allIds: string[] | null;
  hiddenFromAll: string[];
}) {
  const [open, setOpen] = useState(() => devParam("switcher") === "1");
  const ref = useRef<HTMLDivElement>(null);
  const current = accounts.find((a) => a.id === accountFilter);
  // The picked account's Google photo, when it has one; else its letter tile.
  const currentPhoto = useAccountPhoto(current?.id);

  // A right-click menu (or its confirm) opened from a profile row floats outside the switcher.
  useDismiss(open, () => setOpen(false), [ref], ".cm, .app-confirm");
  useEffect(() => {
    if (!open) return;
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        setOpen(false);
      }
    };
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  const pickAccount = (id: string) => {
    switchAccount(id);
    setOpen(false);
  };
  const pickProfile = (id: string | null) => {
    switchProfile(id);
    setOpen(false);
  };
  const allUnread = inboxUnread(labels, allIds);
  const title = current
    ? current.email + (profile ? ` · ${profile.name}` : "")
    : profile
      ? `${profile.name}: ${profile.accountIds.length} ${profile.accountIds.length === 1 ? "account" : "accounts"}`
      : `All accounts (${accounts.length})`;

  return (
    <div className="switcher-wrap" ref={ref}>
      <button className="switcher" onClick={() => setOpen((o) => !o)} aria-expanded={open} title={title}>
        {current && currentPhoto ? (
          <AccountAvatar account={current} accounts={accounts} className="switcher-av" />
        ) : current ? (
          <span className={`acct-tile t-${accountTone(current.color)}`}>
            {accountName(current, accounts).slice(0, 1).toUpperCase()}
          </span>
        ) : profile ? (
          <ProfileTile profile={profile} />
        ) : (
          <span className="acct-tile t-gray">
            <Icon name="mails" size="sm" />
          </span>
        )}
        <span className="truncate">{current ? accountName(current, accounts) : profile ? profile.name : "All accounts"}</span>
        {!current && profile && profile.accountIds.length > 1 && (
          <span className="switcher-count tnum">· {profile.accountIds.length}</span>
        )}
        {!current && !profile && accounts.length > 1 && <span className="switcher-count tnum">· {accounts.length}</span>}
        <Icon name="updown" size="sm" className="chev" />
      </button>
      {open && (
        <div className="panel menu switcher-menu" role="menu">
          <button
            className={"menu-item" + (!accountFilter && !profile ? " active" : "")}
            role="menuitem"
            onClick={() => pickProfile(null)}
          >
            <Icon name="mails" size="sm" className="sw-ico" />
            <span className="grow truncate">
              All accounts <span className="faint tnum">· {accounts.length}</span>
            </span>
            {allUnread > 0 && <span className="sw-count tnum">{num(allUnread)}</span>}
            <Keys keys={profiles.length > 0 ? `${PROFILE_MOD}+0` : "alt+0"} />
          </button>
          {profiles.length > 0 && (
            <>
              <div className="menu-sep" />
              <div className="menu-head">Profiles</div>
              {profiles.map((p, i) => {
                const n = inboxUnread(labels, p.accountIds);
                const keys = profileKeys(i);
                return (
                  <SwitcherProfileRow key={p.id} profile={p} active={profile?.id === p.id && !accountFilter}>
                  <button
                    role="menuitem"
                    className={"menu-item sw-profile" + (profile?.id === p.id && !accountFilter ? " active" : "")}
                    onClick={() => pickProfile(p.id)}
                    onContextMenu={(e) => showContextMenu(e, profileMenu(p, profile?.id === p.id), { label: `Profile ${p.name}` })}
                    title={p.accountIds.map((id) => accounts.find((a) => a.id === id)?.email ?? id).join("\n")}
                  >
                    <ProfileTile profile={p} size="sm" />
                    <span className="grow truncate">
                      {p.name}{" "}
                      <span className="faint tnum">
                        · {p.accountIds.length} {p.accountIds.length === 1 ? "account" : "accounts"}
                      </span>
                    </span>
                    {n > 0 && <span className="sw-count tnum">{num(n)}</span>}
                    {keys && <Keys keys={keys} />}
                  </button>
                  </SwitcherProfileRow>
                );
              })}
            </>
          )}
          <div className="menu-sep" />
          <div className="menu-head">Accounts</div>
          {accounts.map((a, i) => {
            const n = sumUnread(labels, "INBOX", a.id);
            return (
              <button
                key={a.id}
                role="menuitem"
                className={"menu-item" + (accountFilter === a.id ? " active" : "")}
                onClick={() => pickAccount(a.id)}
                title={hiddenFromAll.includes(a.id) ? `${a.email}
Hidden from All Inboxes` : a.email}
              >
                <AccountAvatar account={a} accounts={accounts} />
                <AccountBar color={a.color} />
                <span className="grow truncate">
                  {accountName(a, accounts)}
                  {accountName(a, accounts) !== a.email && <span className="faint sw-email"> · {a.email}</span>}
                </span>
                {hiddenFromAll.includes(a.id) && (
                  <span className="sw-hidden-all" role="img" aria-label="Hidden from All Inboxes">
                    <Icon name="eyeoff" size="xs" />
                  </span>
                )}
                {n > 0 && !hiddenFromAll.includes(a.id) && <span className="sw-count tnum">{num(n)}</span>}
                {i < 9 && <Keys keys={`alt+${i + 1}`} />}
              </button>
            );
          })}
          <div className="menu-sep" />
          <button
            className="menu-item sw-manage"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              openSettings("profiles");
            }}
          >
            <Icon name="settings" size="sm" className="sw-ico" />
            <span className="grow">{profiles.length ? "Manage profiles…" : "Group accounts into profiles…"}</span>
          </button>
        </div>
      )}
    </div>
  );
}

/** A profile row in the switcher, or its rename field while renaming. */
function SwitcherProfileRow({ profile: p, children }: { profile: Profile; active: boolean; children: ReactNode }) {
  const editing = useRenaming(renameKey.profile(p.id));
  if (!editing) return <>{children}</>;
  return (
    <div className="menu-item sw-profile is-renaming">
      <ProfileTile profile={p} size="sm" />
      <RenameField initial={p.name} label={`Rename profile ${p.name}`} maxLength={40} onSave={(v) => renameProfile(p, v)} />
    </div>
  );
}

function SidebarFooter({ accounts, scoped }: { accounts: Account[]; scoped: Account[] }) {
  const tip = useKeyTip();
  // You: the first account's Google profile name (your photo is Settings → You).
  const acct = accounts.find((a) => a.displayName) ?? accounts[0];
  const who = acct ? { name: meName(accounts) ?? acct.email, email: acct.email } : null;
  return (
    <div className="sb-foot">
      <SyncProgress accounts={scoped} />
      <div className="sb-bottom">
        {/* One target: you, and Settings (which opens on You). */}
        <button
          className="sb-me grow"
          data-shortcut="app.settings"
          data-shortcut-label="Settings"
          title={(who ? `${who.name}\n` : "") + tip("Settings", "⌘,")}
          aria-label="Settings"
          onClick={() => openSettings()}
        >
          {who && <Avatar person={who} size="sm" tone="gray" />}
          <span className="who truncate grow">{who?.name ?? "Penguin"}</span>
          {showMockBadge && <span className="badge t-amber mock-badge">Mock</span>}
          <Icon name="settings" size="sm" className="sb-me-gear" />
        </button>
      </div>
    </div>
  );
}

export function PenguinLogo() {
  return <PenguinMark />;
}
