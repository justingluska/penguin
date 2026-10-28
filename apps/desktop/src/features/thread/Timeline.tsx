// A conversation's timeline: every message, newest first, the one on screen
// marked; a click shows it. Beside the message in the full view, inside the
// context panel when that's open (the full view hands it over through
// `publishTimeline`), and in a dialog from the reading pane's message bar.
import { useSyncExternalStore } from "react";
import type { MessageView } from "../../lib/types";
import { displayName, shortDate } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { Avatar } from "../../components/Identity";
import { isMe } from "../../app/store";

export function Timeline({
  msgs,
  shownId,
  all = false,
  onPick,
  onAll,
  where = "rail",
}: {
  msgs: MessageView[];
  /** The message on screen; null when every message is (Show all, the reading pane). */
  shownId: string | null;
  all?: boolean;
  onPick: (id: string) => void;
  /** Show all / One at a time; no button without it. */
  onAll?: () => void;
  where?: "rail" | "panel" | "dialog";
}) {
  const last = msgs.length - 1;
  return (
    <div className={`tl-rail tl-${where}`} aria-label="Messages in this conversation">
      <div className="tl-head">
        <span>Timeline · newest first</span>
        {onAll && (
          <button className="btn btn-ghost btn-sm" onClick={onAll} title={all ? "Show one message at a time (O)" : "Show every message, oldest first (O)"}>
            {all ? "One at a time" : "Show all"}
          </button>
        )}
      </div>
      <ol className="tl-list">
        {msgs
          .map((m, i) => ({ m, i }))
          .reverse()
          .map(({ m, i }) => {
            const files = m.attachments.filter((a) => !a.inline).length;
            const on = m.id === shownId;
            return (
              <li key={m.id}>
                <button
                  className={"tl-item" + (on ? " is-on" : "") + (i === last ? " is-latest" : "") + (m.unread ? " is-unread" : "")}
                  aria-current={on ? "true" : undefined}
                  onClick={() => onPick(m.id)}
                >
                  <span className="tl-l1">
                    <Avatar person={m.from} size="xs" tone={isMe(m.from.email) ? "gray" : undefined} photo={!isMe(m.from.email)} />
                    <span className="tl-who truncate">{isMe(m.from.email) ? "You" : displayName(m.from)}</span>
                    {m.labelIds.includes("DRAFT") && <span className="badge t-amber">Draft</span>}
                    {i === last && msgs.length > 1 && <span className="badge t-blue">Latest</span>}
                    <span className="tl-when tnum">{shortDate(m.date)}</span>
                  </span>
                  <span className="tl-l2 truncate">
                    {files > 0 && (
                      <span className="tl-files">
                        <Icon name="clip" size="2xs" />
                        {files}
                      </span>
                    )}
                    {m.snippet}
                  </span>
                </button>
              </li>
            );
          })}
      </ol>
    </div>
  );
}

/** The full view's timeline as the context panel shows it (null: not handed over). */
export interface SharedTimeline {
  /** `accountId/threadId` of the open thread. */
  threadKey: string;
  shownId: string | null;
  all: boolean;
  pick: (id: string) => void;
  toggleAll: () => void;
}

let shared: SharedTimeline | null = null;
const subs = new Set<() => void>();

export function publishTimeline(t: SharedTimeline | null) {
  shared = t;
  subs.forEach((f) => f());
}

export function useSharedTimeline(): SharedTimeline | null {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => shared,
  );
}
