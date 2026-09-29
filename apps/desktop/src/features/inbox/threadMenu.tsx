// Right-click menu for a conversation row (the list and Floe). OWNER: menus
// agent. A row that's part of the multi-selection acts on the whole
// selection (app/selection.ts), like E or # would; any other row acts on
// itself alone and leaves the selection and cursor where they were (so
// right-clicking doesn't mark anything read).
import type { Label, ThreadRef, ThreadSummary } from "../../lib/types";
import { api } from "../../lib/api";
import { getUi, setUi } from "../../lib/ui";
import { num } from "../../lib/format";
import { copyText } from "../../lib/clipboard";
import type { MenuEntries } from "../../components/ContextMenu";
import { LabelSwatch } from "../../components/Identity";
import { accountById, list, meta, selectThread } from "../../app/store";
import { archive, moveToInbox, markRead, markUnread, notSpam, openCompose, reportSpam, setLabel, toggleStar, trash, unsnooze } from "../../app/actions";
import { openSnooze } from "../snooze/SnoozePicker";
import { openSelected } from "../../app/shortcuts";
import { isSelected, selectionCount, selectionTargets, toggleSelect } from "../../app/selection";
import { gmailThreadUrl } from "../../app/accountActions";
import { ruleFromThread } from "../rules/state";
import { isLabelChoice, threadAbilities } from "../../lib/capabilities";
import { folderOptions, moveToOption } from "./MovePicker";
import { dismissFollowUps, inReplyLater, leaveReplyLater, toggleReplyLater } from "../triage/actions";
import { COPY_CONVERSATION_KEYS, copyConversation } from "../thread/copy";
import { openThreadWindow } from "../../app/windows";

const keyOf = (r: ThreadRef) => r.accountId + "\u0000" + r.threadId;
const sameRef = (a: ThreadRef | null, b: ThreadRef) => !!a && keyOf(a) === keyOf(b);

function summaries(refs: ThreadRef[]): ThreadSummary[] {
  const want = new Set(refs.map(keyOf));
  return list.get().items.filter((t) => want.has(keyOf(t)));
}

/** Label ▸: the account's user labels, checked when every target has it. */
function labelSubmenu(refs: () => Promise<ThreadRef[]>, loaded: ThreadSummary[], single: ThreadSummary | null): MenuEntries {
  const accounts = new Set(loaded.map((t) => t.accountId));
  // One entry per label name; a selection across accounts applies it in each account that has it.
  const byName = new Map<string, Label[]>();
  for (const l of meta.get().labels) {
    if (!accounts.has(l.accountId) || !isLabelChoice(l, accountById(l.accountId))) continue;
    byName.set(l.name, [...(byName.get(l.name) ?? []), l]);
  }
  const names = [...byName.keys()].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));
  const pick = () => {
    if (single) selectThread({ accountId: single.accountId, threadId: single.threadId });
    setUi({ overlay: "label" });
  };
  if (names.length === 0) return [{ label: "No labels yet", disabled: true }, { type: "separator" }, { label: "Choose labels…", keys: "l", onSelect: pick }];
  return [
    ...names.slice(0, 30).map((name) => {
      const ls = byName.get(name)!;
      const has = (t: ThreadSummary) => ls.some((l) => l.accountId === t.accountId && t.labelIds.includes(l.id));
      const on = loaded.length > 0 && loaded.every(has);
      return {
        label: name,
        lead: <LabelSwatch color={ls[0].color} />,
        checked: on,
        onSelect: () =>
          void refs().then((rs) => {
            for (const l of ls) {
              const mine = rs.filter((r) => r.accountId === l.accountId);
              if (mine.length) setLabel(mine, l.id, !on);
            }
          }),
      };
    }),
    { type: "separator" },
    { label: names.length > 30 ? "All labels…" : "Choose labels…", keys: "l", onSelect: pick },
  ];
}

/** Move to ▸: the Inbox and the accounts' folders, checked where every target already is. */
function moveSubmenu(refs: () => Promise<ThreadRef[]>, loaded: ThreadSummary[], single: ThreadSummary | null): MenuEntries {
  const options = folderOptions(meta.get().labels, new Set(loaded.map((t) => t.accountId)));
  const pick = () => {
    if (single) selectThread({ accountId: single.accountId, threadId: single.threadId });
    setUi({ overlay: "move" });
  };
  const shown = options.slice(0, 31);
  return [
    ...shown.map((o) => {
      const here = loaded.length > 0 && loaded.every((t) => t.labelIds.includes(o.idByAccount.get(t.accountId) ?? "\u0000"));
      return {
        label: o.name,
        icon: o.inbox ? ("inbox" as const) : ("folder" as const),
        checked: here,
        onSelect: () => void refs().then((rs) => rs.length && moveToOption(rs, o)),
      };
    }),
    { type: "separator" },
    { label: options.length > shown.length ? "All folders…" : "Choose folder…", keys: "v", onSelect: pick },
  ];
}

export function threadMenu(t: ThreadSummary): MenuEntries {
  const ref: ThreadRef = { accountId: t.accountId, threadId: t.threadId };
  const multi = isSelected(ref) && selectionCount() > 1;
  const refs = multi ? selectionTargets : () => Promise.resolve([ref]);
  const run = (f: (rs: ThreadRef[]) => unknown) => () => void refs().then((rs) => rs.length && f(rs));
  const loaded = multi ? summaries([...list.get().items].filter((x) => isSelected(x))) : [t];
  const one = multi ? "Select one conversation" : false;
  const view = getUi().view.kind;
  const allUnread = loaded.every((x) => x.unread);
  const allStarred = loaded.every((x) => x.starred);
  const inInbox = loaded.every((x) => x.labelIds.includes("INBOX"));
  const spam = view === "spam" || (loaded.length > 0 && loaded.every((x) => x.labelIds.includes("SPAM")));
  const account = accountById(t.accountId);
  const can = threadAbilities([...new Set(loaded.map((x) => x.accountId))].map(accountById));
  // Gmail links only make sense for accounts synced through the Gmail API (thread ids are Gmail's).
  const gmail = account?.provider === "gmail" ? account : undefined;
  const compose =(mode: "reply" | "replyAll" | "forward") => () => {
    selectThread(ref);
    openCompose(mode);
  };

  return [
    multi && { type: "header", label: `${num(selectionCount())} conversations` },
    !multi && {
      label: view === "drafts" ? "Edit draft" : "Open",
      icon: view === "drafts" ? "draft" : "expand",
      keys: "enter",
      onSelect: () => {
        selectThread(ref);
        openSelected();
      },
    },
    // Leaves the list and cursor where they are (the window marks it read when it opens).
    !multi &&
      view !== "drafts" && {
        label: "Open in new window",
        icon: "window",
        keys: sameRef(getUi().selected, ref) ? "shift+o" : undefined,
        onSelect: () => void openThreadWindow(ref),
      },
    { type: "separator" },
    { label: "Reply", icon: "reply", keys: "r", disabled: one, onSelect: compose("reply") },
    { label: "Reply all", icon: "replyall", keys: "a", disabled: one, onSelect: compose("replyAll") },
    { label: "Forward", icon: "forward", keys: "f", disabled: one, onSelect: compose("forward") },
    { type: "separator" },
    view === "followUp" && { label: "Dismiss", icon: "x", keys: "e", onSelect: run(dismissFollowUps) },
    view === "replyLater" && { label: "Done: out of Reply Later", icon: "done", keys: "e", onSelect: run(leaveReplyLater) },
    view === "followUp" || view === "replyLater"
      ? false
      : view === "trash"
      ? { label: "Restore from Trash", icon: "undo", keys: "#", onSelect: run(trash) }
      : spam
        ? { label: "Not spam", icon: "inbox", keys: "!", onSelect: run(notSpam) }
      : view === "snoozed" || loaded.every((x) => x.snoozedUntil != null)
        ? { label: "Unsnooze", icon: "inbox", keys: view === "snoozed" ? "e" : undefined, onSelect: run(unsnooze) }
      : inInbox || view === "inbox"
        ? { label: "Archive", icon: "archive", keys: "e", onSelect: run(archive) }
        : { label: "Move to inbox", icon: "inbox", keys: view === "done" ? "e" : undefined, onSelect: run(moveToInbox) },
    allUnread
      ? { label: "Mark as read", icon: "mail", keys: "u", onSelect: run((rs) => markRead(rs)) }
      : { label: "Mark as unread", icon: "unread", keys: "shift+u", onSelect: run((rs) => markUnread(rs)) },
    { label: allStarred ? "Unstar" : "Star", icon: "star", keys: "s", onSelect: run(toggleStar) },
    { label: "Snooze…", icon: "snooze", keys: "h", onSelect: () => openSnooze(multi ? undefined : [ref]) },
    view !== "trash" &&
      (loaded.length > 0 && loaded.every((x) => inReplyLater(x))
        ? { label: "Back to the inbox", icon: "inbox", keys: "y", onSelect: run(toggleReplyLater) }
        : { label: "Reply later", icon: "replyLater", keys: "y", onSelect: run(toggleReplyLater) }),
    can.label && { label: "Label", icon: "tag", submenu: () => labelSubmenu(refs, loaded, multi ? null : t) },
    can.move && { label: "Move to", icon: "folder", submenu: () => moveSubmenu(refs, loaded, multi ? null : t) },
    { type: "separator" },
    !multi && {
      label: isSelected(ref) ? "Deselect" : "Select",
      icon: "squarecheck",
      keys: "x",
      onSelect: () => toggleSelect(ref),
    },
    !multi && {
      label: "Always do this…",
      icon: "wand",
      keys: "shift+a",
      onSelect: () => void ruleFromThread(ref),
    },
    !multi && {
      label: "Copy conversation",
      icon: "copy",
      // ⇧C copies the cursor's conversation; show it only when that's this row.
      keys: sameRef(getUi().selected, ref) ? COPY_CONVERSATION_KEYS : undefined,
      onSelect: () => copyConversation(ref),
    },
    !multi && gmail && {
      label: "Copy Gmail link",
      icon: "link",
      onSelect: () => void copyText(gmailThreadUrl(gmail.email, t.threadId), "Link copied"),
    },
    !multi && gmail && {
      label: "Open in Gmail",
      icon: "external",
      onSelect: () => void api.openExternal(gmailThreadUrl(gmail.email, t.threadId)),
    },
    { type: "separator" },
    // Drafts are your own, and Trash is on its way out: nothing to report there.
    !spam && view !== "trash" && view !== "drafts" && { label: "Report spam", icon: "shield", keys: "!", onSelect: run(reportSpam) },
    view !== "trash" && { label: "Move to Trash", icon: "trash", keys: "#", danger: true, onSelect: run(trash) },
  ];
}
