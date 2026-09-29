// The floating bar over the thread list while conversations are selected
// (app/selection.ts): the count, the batch actions, and "select all N
// matching" once every loaded row is selected but the view has more.
import { useEffect, useState } from "react";
import { setUi, useUi } from "../../lib/ui";
import { num } from "../../lib/format";
import { Icon, type IconName } from "../../components/Icon";
import { archive, markRead, markUnread, moveToInbox, notSpam, onTargets, reportSpam, targetAbilities, targetsInSpam, toggleStar, trash } from "../../app/actions";
import { list, listAllInView, meta } from "../../app/store";
import { clearSelection, selectAllMatching, useSelection } from "../../app/selection";
import { toast } from "../../components/Toast";
import { viewTitle } from "./ThreadList";
import { openSnooze } from "../snooze/SnoozePicker";
import { dismissFollowUps, leaveReplyLater, toggleReplyLater } from "../triage/actions";

interface BarAction {
  icon: IconName;
  label: string;
  keys?: string;
  run: () => void;
}

export function SelectionBar() {
  const count = useSelection((s) => s.keys.size);
  const allMatching = useSelection((s) => s.allMatching);
  const loaded = list.use((l) => l.items.length);
  const hasMore = list.use((l) => l.hasMore);
  const view = useUi((s) => s.view);
  const total = useViewTotal(count > 0 && count === loaded && hasMore && !allMatching);
  // Which accounts are selected decides Label vs Move to (below).
  useSelection((s) => s.keys);
  meta.use((m) => m.accounts);
  if (count === 0 && !allMatching) return null;

  const inInbox = view.kind === "inbox";
  // Label and Move to follow what every selected conversation's account can do.
  const can = targetAbilities();
  const spam = targetsInSpam();
  const actions: (BarAction | false)[] = [
    spam
      ? { icon: "inbox", label: "Not spam", keys: "!", run: () => onTargets(notSpam) }
      : view.kind === "followUp"
      ? { icon: "x", label: "Dismiss", keys: "e", run: () => onTargets(dismissFollowUps) }
      : view.kind === "replyLater"
        ? { icon: "done", label: "Done: out of Reply Later", keys: "e", run: () => onTargets(leaveReplyLater) }
        : inInbox
          ? { icon: "done", label: "Archive", keys: "e", run: () => onTargets(archive) }
          : { icon: "inbox", label: "Move to inbox", run: () => onTargets(moveToInbox) },
    view.kind === "replyLater"
      ? { icon: "inbox", label: "Back to the inbox", keys: "y", run: () => onTargets(toggleReplyLater) }
      : { icon: "replyLater", label: "Reply later", keys: "y", run: () => onTargets(toggleReplyLater) },
    { icon: "trash", label: view.kind === "trash" ? "Restore" : "Trash", keys: "#", run: () => onTargets(trash) },
    !spam && view.kind !== "trash" && view.kind !== "drafts" && { icon: "shield", label: "Report spam", keys: "!", run: () => onTargets(reportSpam) },
    { icon: "eye", label: "Mark read", run: () => onTargets((r) => (markRead(r), toast({ message: `Marked ${num(r.length)} as read` }))) },
    { icon: "unread", label: "Mark unread", keys: "shift+u", run: () => onTargets((r) => (markUnread(r), toast({ message: `Marked ${num(r.length)} as unread` }))) },
    { icon: "star", label: "Star / unstar", keys: "s", run: () => onTargets(toggleStar) },
    { icon: "snooze", label: "Snooze", keys: "h", run: () => openSnooze() },
    can.label && { icon: "tag", label: "Label", keys: "l", run: () => setUi({ overlay: "label" }) },
    can.move && { icon: "folder", label: "Move to", keys: "v", run: () => setUi({ overlay: "move" }) },
  ];

  const where = viewTitle(view);
  return (
    <div className="sel-bar-wrap">
      {total !== null && (
        <div className="sel-banner panel">
          <span className="grow">All {num(count)} loaded selected.</span>
          <button className="btn btn-ghost btn-sm sel-link" onClick={selectAllMatching}>
            Select all {total > 0 ? num(total) : ""} in {where}
          </button>
        </div>
      )}
      <div className="sel-bar panel" role="toolbar" aria-label="Selected conversations">
        <span className="sel-count tnum">{allMatching ? `All in ${where}` : `${num(count)} selected`}</span>
        <span className="sel-sep" />
        {actions.filter((a): a is BarAction => !!a).map((a) => (
          <button
            key={a.label}
            className="btn btn-ghost btn-icon btn-sm sel-act"
            title={a.keys ? `${a.label} (${a.keys.replace("shift+", "⇧").toUpperCase()})` : a.label}
            aria-label={a.label}
            onMouseDown={(e) => e.preventDefault()}
            onClick={a.run}
          >
            <Icon name={a.icon} size="sm" />
          </button>
        ))}
        <span className="sel-sep" />
        <button
          className="btn btn-ghost btn-icon btn-sm sel-clear"
          onMouseDown={(e) => e.preventDefault()}
          onClick={clearSelection}
          title="Clear selection (Esc)"
          aria-label="Clear selection"
        >
          <Icon name="x" size="sm" />
        </button>
      </div>
    </div>
  );
}

/** How many threads the whole view has, counted once the banner needs it (-1 while counting). */
function useViewTotal(wanted: boolean): number | null {
  const view = useUi((s) => JSON.stringify([s.view, s.split, s.accountFilter, s.profileId]));
  const [total, setTotal] = useState<{ view: string; n: number } | null>(null);
  useEffect(() => {
    // Count afresh each time the banner appears (mail may have come and gone).
    if (!wanted) {
      if (total) setTotal(null);
      return;
    }
    if (total?.view === view) return;
    let live = true;
    listAllInView().then(
      (all) => live && setTotal({ view, n: all.length }),
      () => live && setTotal({ view, n: 0 }),
    );
    return () => {
      live = false;
    };
  }, [wanted, view, total]);
  if (!wanted) return null;
  return total?.view === view ? total.n : -1;
}
