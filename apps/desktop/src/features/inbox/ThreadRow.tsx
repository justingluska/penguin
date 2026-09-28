// One list row (design/01-inbox.html `.msg`): a single line in a wide pane,
// or three stacked lines (sender · time / subject / snippet) in a narrow one.
// Memoized; each row subscribes to "am I selected" on its own so moving the
// cursor re-renders two rows, not the list.
import { memo, useRef } from "react";
import type { ThreadRef, ThreadSummary } from "../../lib/types";
import { useUi } from "../../lib/ui";
import { displayName, listTime } from "../../lib/format";
import { accountTone } from "../../lib/accountColor";
import { Icon, type IconName } from "../../components/Icon";
import { Kbd } from "../../components/Kbd";
import { Avatar, LabelChip } from "../../components/Identity";
import { useAvatarPlacement } from "../../lib/avatars";
import { isMe, labelById, meta, useLabelLook } from "../../app/store";
import { selectRange, toggleSelect, useIsSelected } from "../../app/selection";
import { showTargetMenu } from "../../components/ContextMenu";
import { threadMenu } from "./threadMenu";
import { OtpPill } from "../otp/OtpPill";
import { rowCode, useNow } from "../otp/otp";
import { InviteChip } from "../calendar/InviteChip";
import { fmtUntil, fmtWake } from "../snooze/presets";
import "../snooze/snooze.css";
import { waitingText } from "../triage/format";
import { SmartLine } from "../smart/SmartParts";
import "../triage/triage.css";

/** The row's time: when it wakes for a snoozed thread, else its latest message. */
function RowTime({ t }: { t: ThreadSummary }) {
  if (t.snoozedUntil == null) return <span className="time">{listTime(t.lastDate)}</span>;
  return (
    <span className="time row-snoozed" title={`Snoozed ${fmtUntil(t.snoozedUntil)}`}>
      <Icon name="snooze" size="xs" />
      {fmtWake(t.snoozedUntil)}
    </span>
  );
}

export interface RowHandlers {
  onSelect: (ref: ThreadRef) => void;
  onOpen: (ref: ThreadRef) => void;
  /** Reopen this thread's saved draft in compose. */
  onDraft: (ref: ThreadRef) => void;
  onAction: (ref: ThreadRef, action: "done" | "snooze" | "reply" | "label" | "move" | "trash" | "replyLater" | "dismiss") => void;
}

/**
 * Counts changes of `v` while this row stays mounted (0 on mount), so a state
 * flip can play a small animation that a row merely scrolling into view
 * doesn't: the class alternates (…0/…1) to restart it on every flip.
 */
function useFlips(v: boolean): number {
  const r = useRef({ v, n: 0 });
  if (r.current.v !== v) r.current = { v, n: r.current.n + 1 };
  return r.current.n;
}
const flipClass = (name: string, n: number) => (n ? ` ${name}${n % 2}` : "");

/** Smart rows whose own chip (invite, code) stays in place of the snippet. */
const SELF_CHIPPED = new Set(["invite", "code", "link"]);

/** Whose face the row shows: the first participant who isn't you. */
function avatarPerson(t: ThreadSummary) {
  return t.participants.find((p) => !isMe(p.email)) ?? t.participants[0] ?? null;
}

function senderLabel(t: ThreadSummary): string {
  const others = t.participants.filter((p) => !isMe(p.email));
  if (others.length === 0) return t.participants.length ? "me" : "(no sender)";
  return displayName(others[0]);
}

type RowAction = { key: string; icon: IconName; action: Parameters<RowHandlers["onAction"]>[1]; title: string; disabled?: boolean };
// Done sits beside Trash at the end, away from where the pointer lands on
// hover, so a stray click doesn't archive.
const ACTIONS: RowAction[] = [
  { key: "Y", icon: "replyLater", action: "replyLater", title: "Reply later (Y)" },
  { key: "H", icon: "snooze", action: "snooze", title: "Snooze (H)" },
  { key: "R", icon: "reply", action: "reply", title: "Reply (R)" },
  { key: "L", icon: "tag", action: "label", title: "Label (L)" },
  { key: "E", icon: "done", action: "done", title: "Done (E)" },
  { key: "#", icon: "trash", action: "trash", title: "Trash (#)" },
];
// Accounts whose labels are folders (IMAP) move instead of labeling; L does the same there.
const ACTIONS_MOVE = ACTIONS.map((a) => (a.action === "label" ? { ...a, key: "V", icon: "folder" as IconName, action: "move" as const, title: "Move to (V)" } : a));
const ACTIONS_NEITHER = ACTIONS.filter((a) => a.action !== "label");
// Reply Later: reply, done here (the label comes off), back to the inbox.
const ACTIONS_REPLY_LATER: RowAction[] = [
  { key: "R", icon: "reply", action: "reply", title: "Reply (R)" },
  { key: "Y", icon: "inbox", action: "replyLater", title: "Back to the inbox (Y)" },
  { key: "H", icon: "snooze", action: "snooze", title: "Snooze (H)" },
  { key: "E", icon: "done", action: "done", title: "Done: out of Reply Later (E)" },
  { key: "#", icon: "trash", action: "trash", title: "Trash (#)" },
];
// Follow up: nudge, snooze, dismiss.
const ACTIONS_FOLLOW_UP: RowAction[] = [
  { key: "R", icon: "reply", action: "reply", title: "Nudge: reply (R)" },
  { key: "H", icon: "snooze", action: "snooze", title: "Snooze (H)" },
  { key: "E", icon: "x", action: "dismiss", title: "Dismiss until something new (E)" },
];

export const ThreadRow = memo(function ThreadRow({
  t,
  handlers,
  stacked,
  floe = false,
}: {
  t: ThreadSummary;
  handlers: RowHandlers;
  stacked: boolean;
  /** Floe mode (features/floe): the one-line markup, laid out as two calm lines. */
  floe?: boolean;
}) {
  const selected = useUi((s) => s.selected !== null && s.selected.threadId === t.threadId && s.selected.accountId === t.accountId);
  // The account bar (styles/themes.css) only says something in the unified
  // inbox of several accounts.
  const unified = useUi((s) => s.accountFilter === null);
  const multiAccount = meta.use((m) => m.accounts.length > 1);
  const showAccount = unified && multiAccount;
  // Re-render when labels load or change look (chip names/colors), not on
  // count changes: marking a conversation read would re-render every row.
  useLabelLook();
  const inDrafts = useUi((s) => s.view.kind === "drafts");
  const hasDraft = !inDrafts && t.labelIds.includes("DRAFT");
  // Subscribed so a recolor (Settings, the sidebar Color grid) repaints the bar.
  const accountColor = meta.use((m) => m.accounts.find((a) => a.id === t.accountId)?.color ?? null);
  const caps = meta.use((m) => m.accounts.find((a) => a.id === t.accountId)?.capabilities);
  const viewKind = useUi((s) => s.view.kind);
  const followUp = viewKind === "followUp";
  const actions =
    viewKind === "replyLater"
      ? ACTIONS_REPLY_LATER
      : followUp
        ? ACTIONS_FOLLOW_UP
        : !caps || caps.labels
          ? ACTIONS
          : caps.folders
            ? ACTIONS_MOVE
            : ACTIONS_NEITHER;
  // Settings → Appearance → "Sender photos appear": the monogram/photo
  // slot. Without it the row keeps its original grid (no empty gutter).
  const showPhotos = useAvatarPlacement().list;
  const person = showPhotos ? avatarPerson(t) : null;
  const mine = person !== null && isMe(person.email);
  const avatar = person && (
    <Avatar
      person={person}
      size={stacked ? "row" : floe ? "md" : "sm"}
      tone={mine ? "gray" : undefined}
      photo={!mine}
    />
  );
  const ref = { accountId: t.accountId, threadId: t.threadId };
  const checked = useIsSelected(ref);
  const starFlips = useFlips(t.starred);
  const unreadFlips = useFlips(t.unread);
  // A recent verification code replaces the snippet (features/otp).
  const otp = rowCode(t.otp, useNow(!!t.otp));
  // A smart view row shows its fact there (features/smart); invites and
  // codes keep their own chip.
  const smart = t.smart && !SELF_CHIPPED.has(t.smart.kind) ? t.smart : null;
  // A calendar invitation shows its event chip there instead (features/calendar).
  const invite = !otp && !smart && t.invite ? t.invite : null;
  const chips = [];
  for (const id of t.labelIds) {
    const l = labelById(t.accountId, id);
    // Reply Later's own view doesn't repeat its label on every row.
    if (l && l.kind === "user" && !(viewKind === "replyLater" && l.name.toLowerCase() === "reply later")) chips.push(l);
    if (chips.length === (stacked ? 1 : 2)) break;
  }
  const meta_ = (
    <span className="meta">
      {hasDraft && (
        <button
          className="badge draft-chip"
          title="Edit draft"
          onMouseDown={(e) => e.stopPropagation()}
          onClick={(e) => {
            e.stopPropagation();
            handlers.onDraft(ref);
          }}
        >
          <Icon name="draft" size="xs" />
          Draft
        </button>
      )}
      {t.hasAttachments && <Icon name="clip" size="xs" />}
      {t.starred && <Icon name="star" size="xs" fill className="star" />}
      {chips.map((l) => (
        <LabelChip key={l.id} label={l} />
      ))}
    </span>
  );

  // The multi-select checkbox (app/selection.ts) rides on the face: a badge on
  // a large face's corner, in place of a small one. Rows without faces show it
  // in selection mode only, when a gutter opens for it (styles/app.css).
  const check = (
    <span
      className="row-check"
      role="checkbox"
      aria-checked={checked}
      aria-label="Select conversation"
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        e.stopPropagation();
        if (e.shiftKey) selectRange(ref);
        else toggleSelect(ref);
      }}
    >
      {checked && <Icon name="check" size="xs" />}
    </span>
  );

  return (
    <div
      className={
        "msg" +
        (stacked ? " stacked" : "") +
        (floe ? " floe" : "") +
        (t.unread ? " unread" : "") +
        (selected ? " is-selected" : "") +
        (checked ? " is-checked" : "") +
        (showAccount ? ` has-acct t-${accountTone(accountColor)}` : "") +
        (avatar ? " has-avatar" : "") +
        (otp ? " has-otp" : "") +
        (invite ? " has-invite" : "") +
        (smart ? " has-smart" : "") +
        flipClass("star-flip", starFlips) +
        flipClass("unread-flip", unreadFlips)
      }
      role="option"
      aria-selected={selected}
      data-account={t.accountId}
      data-thread={t.threadId}
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        // ⌘-click toggles, shift-click extends from the anchor (no text selection).
        if (e.shiftKey) {
          e.preventDefault();
          selectRange(ref);
        } else if (e.metaKey || e.ctrlKey) {
          toggleSelect(ref);
        }
        handlers.onSelect(ref);
      }}
      onDoubleClick={() => handlers.onOpen(ref)}
      onContextMenu={(e) => showTargetMenu(e, threadMenu(t), { label: t.subject || "Conversation" })}
    >
      {!avatar && check}
      {avatar && (stacked || floe) && (
        <span className="row-av">
          {avatar}
          {check}
        </span>
      )}
      <span className="from">
        {avatar && !stacked && !floe && (
          <span className="row-av">
            {avatar}
            {check}
          </span>
        )}
        <span className="name">{senderLabel(t)}</span>
        {t.messageCount > 1 && <span className="n">{t.messageCount}</span>}
      </span>
      {stacked ? (
        <>
          <RowTime t={t} />
          <span className="subj">{t.subject || "(no subject)"}</span>
          {meta_}
          {followUp ? (
            <span className="fu-wait">{waitingText(t.lastDate)}</span>
          ) : smart ? (
            <SmartLine row={smart} stacked />
          ) : otp ? (
            <span className="otp-slot">
              <OtpPill otp={otp} />
            </span>
          ) : invite ? (
            <span className="inv-slot">
              <InviteChip chip={invite} accountId={t.accountId} threadId={t.threadId} />
            </span>
          ) : (
            <span className="snip">{t.snippet}</span>
          )}
        </>
      ) : (
        <>
          <span className="line">
            <span className="subj">{t.subject || "(no subject)"}</span>
            {followUp ? (
              <span className="fu-wait"> — {waitingText(t.lastDate)}</span>
            ) : smart ? (
              <SmartLine row={smart} stacked={false} />
            ) : otp ? (
              <span className="otp-slot">
                <OtpPill otp={otp} />
              </span>
            ) : invite ? (
              <span className="inv-slot">
                <InviteChip chip={invite} accountId={t.accountId} threadId={t.threadId} />
              </span>
            ) : (
              t.snippet && <span className="snip"> — {t.snippet}</span>
            )}
          </span>
          {meta_}
          <RowTime t={t} />
        </>
      )}
      <span className="msg-actions" onMouseDown={(e) => e.stopPropagation()}>
        {actions.map((a) => (
          <button
            key={a.key}
            className={"act" + (a.disabled ? " is-disabled" : "")}
            title={a.title}
            onClick={(e) => {
              e.stopPropagation();
              handlers.onAction(ref, a.action);
            }}
          >
            <Icon name={a.icon} size="xs" />
            {!stacked && !floe && <Kbd>{a.key}</Kbd>}
          </button>
        ))}
      </span>
    </div>
  );
});
