// The Unsubscribe button at the right end of a message's privacy row
// ("3 trackers removed · Unsubscribe"). Click → (one-click check when
// needed) → confirm dialog naming the sender and exactly what happens
// (UnsubscribeDialog) → run. Hidden by Settings → Privacy → "Unsubscribe
// button".
import { useState, type ReactNode } from "react";
import type { MessageView } from "../../lib/types";
import { useSetting } from "../../lib/settings";
import { shortDate } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { checkedOffer, confirmUnsubscribe } from "./actions";
import { senderLabel, unsubscribeCopy } from "./model";
import "./unsubscribe.css";

type Phase = "idle" | "checking" | "confirm" | "running";

/** MessageBody's `trailing` for `m`: the button, or null (no offer, or turned off in Settings). */
export function useUnsubscribeSlot(m: MessageView): ReactNode {
  const enabled = useSetting("unsubscribeButton");
  return enabled && m.unsubscribe ? <UnsubscribeButton m={m} /> : null;
}

export function UnsubscribeButton({ m }: { m: MessageView }) {
  const enabled = useSetting("unsubscribeButton");
  const [phase, setPhase] = useState<Phase>("idle");
  const offer = m.unsubscribe;

  if (!enabled || !offer) return null;
  const sender = senderLabel(m.from);
  const copy = unsubscribeCopy(offer, sender);

  const open = async () => {
    setPhase("checking");
    try {
      const checked = await checkedOffer(m, offer);
      setPhase("confirm");
      await confirmUnsubscribe(m, checked, () => setPhase("running"));
    } finally {
      setPhase("idle");
    }
  };

  // Stays enabled while the dialog is up, so focus can return to it.
  const busy = phase === "checking" || phase === "running";
  const doneTitle = offer.unsubscribed ? `You unsubscribed on ${shortDate(offer.unsubscribed.at)}. ${copy.tooltip}` : copy.tooltip;
  return (
    <button
      type="button"
      className={"unsub-btn unsub-open" + (copy.done ? " is-done" : "")}
      data-shortcut={copy.done ? undefined : "triage.unsubscribe"}
      onClick={() => void open()}
      disabled={busy}
      title={copy.done ? doneTitle : copy.tooltip}
      aria-label={copy.done ? `Unsubscribed from ${sender}. Unsubscribe again` : `Unsubscribe from ${sender}`}
      aria-haspopup="dialog"
    >
      <Icon name={copy.done ? "check" : "belloff"} size="xs" />
      {phase === "checking" ? "Checking…" : phase === "running" ? "Unsubscribing…" : copy.label}
    </button>
  );
}
