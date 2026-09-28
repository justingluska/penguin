// The event chip in a list row, in place of the snippet (like the
// verification-code chip): [ 📅 Tue Sep 29 · 10–11am | Yes Maybe No ].
// Answered invites show "✓ Going"; updates and cancellations get a quiet
// tag; a clash with your calendar is named. Every button is a real button
// (Tab / Enter / Space) that never selects or opens the row. OWNER: calendar agent.
import type { InviteAnswer, InviteChip as Chip } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { useNow } from "../otp/otp";
import { ANSWERS, answerWord, chipView, inviteKey } from "./invite";
import { answerInvite, useInviteOverride } from "./inviteState";
import "./invites.css";

const stop = (e: React.SyntheticEvent) => e.stopPropagation();

export function InviteChip({ chip, accountId, threadId }: { chip: Chip; accountId: string; threadId: string }) {
  const now = useNow();
  const key = inviteKey(accountId, { uid: chip.uid, messageId: chip.messageId });
  const o = useInviteOverride(key);
  const v = chipView(chip, o, now || Date.now());
  const answer = (r: InviteAnswer) =>
    void answerInvite({ accountId, threadId, messageId: chip.messageId, uid: chip.uid, current: chip.response, response: r });
  const title = `${chip.summary || "(no title)"} · ${v.when}${chip.recurring ? " · repeats" : ""}${v.conflict ? ` · ${v.conflict}` : ""}`;
  return (
    <span
      className={"inv-chip" + (v.muted ? " is-muted" : "") + (v.pending ? " is-pending" : "") + (v.tag === "Canceled" ? " is-cancelled" : "") + (v.conflict ? " has-conflict" : "")}
      title={title}
      onMouseDown={stop}
      onDoubleClick={stop}
    >
      <span className="inv-chip-when">
        <Icon name="calendar" size="xs" />
        <span className="inv-chip-text">{v.when}</span>
      </span>
      {v.tag && (
        <span className={"inv-chip-tag" + (v.tag === "Canceled" ? " is-cancel" : "")} title={v.tag} aria-label={v.tag}>
          <Icon name={v.tag === "Canceled" ? "x" : "refresh"} size="xs" />
          <span className="inv-chip-tag-text">{v.tag}</span>
        </span>
      )}
      {v.reply && <span className="inv-chip-reply">{v.reply}</span>}
      {v.buttons ? (
        <span className="inv-chip-rsvp" role="group" aria-label={`Answer the invitation: ${chip.summary || "event"}${v.conflict ? `. ${v.conflict}` : ""}`}>
          {ANSWERS.map((r) => (
            <button
              key={r}
              type="button"
              className={"inv-chip-btn is-" + r}
              disabled={v.pending}
              aria-pressed={v.response === r}
              onKeyDown={(e) => {
                // Enter/Space answer (native click) instead of opening the row.
                if (e.key === "Enter" || e.key === " ") e.stopPropagation();
              }}
              onClick={(e) => {
                e.stopPropagation();
                answer(r);
              }}
            >
              {answerWord(r)}
            </button>
          ))}
        </span>
      ) : (
        v.state && (
          <span className={"inv-chip-state is-" + v.response} aria-live="polite">
            {v.response === "accepted" && <Icon name="check" size="xs" />}
            {v.state}
          </span>
        )
      )}
      {v.conflict && (
        // Short in the row ("⚠ Standup"); the full sentence for tooltips and screen readers.
        <span className="inv-chip-conflict" title={v.conflict} aria-label={v.conflict}>
          <Icon name="info" size="xs" />
          <span className="inv-chip-conflict-name">
            {chip.conflict}
            {chip.conflicts > 1 ? ` +${chip.conflicts - 1}` : ""}
          </span>
        </span>
      )}
    </span>
  );
}
