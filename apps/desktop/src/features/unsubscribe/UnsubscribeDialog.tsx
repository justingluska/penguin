// The unsubscribe confirm: "Unsubscribe from <sender>?" plus exactly what
// happens for this offer (one-click request, the email that gets sent, or
// the full URL of the page that opens). Used by the privacy-row button, the
// message menu, ⌘U and ⌘K. Enter confirms, Esc cancels.
import { useRef } from "react";
import type { UnsubscribeOffer } from "../../lib/types";
import { shortDate } from "../../lib/format";
import { copyText } from "../../lib/clipboard";
import { Icon } from "../../components/Icon";
import { Modal, UrlLine, openModal } from "../../components/Modal";
import type { UnsubscribeCopy } from "./model";
import "./unsubscribe.css";

export type UnsubscribeChoice = "confirm" | "editFirst" | "cancel";

/** Show the confirm; resolves with what the user picked. */
export function askUnsubscribe(offer: UnsubscribeOffer, copy: UnsubscribeCopy): Promise<UnsubscribeChoice> {
  return new Promise((resolve) => {
    let settled = false;
    const settle = (c: UnsubscribeChoice) => {
      if (settled) return false;
      settled = true;
      resolve(c);
      return true;
    };
    openModal(
      (close) => (
        <UnsubscribeDialog
          offer={offer}
          copy={copy}
          onPick={(c) => {
            if (settle(c)) close();
          }}
        />
      ),
      () => settle("cancel"),
    );
  });
}

function UnsubscribeDialog({ offer, copy, onPick }: { offer: UnsubscribeOffer; copy: UnsubscribeCopy; onPick: (c: UnsubscribeChoice) => void }) {
  const confirmRef = useRef<HTMLButtonElement>(null);
  const confirm = () => onPick("confirm");
  const cancel = () => onPick("cancel");
  const previously = offer.unsubscribed && copy.done ? `You unsubscribed from this sender on ${shortDate(offer.unsubscribed.at)}.` : null;
  const icon = copy.action === "link" ? "external" : copy.action === "oneClick" ? "belloff" : "mail";

  return (
    <Modal label={copy.question} role="alertdialog" onClose={cancel} onEnter={confirm} initialFocus={confirmRef} className="unsub-dialog">
      <h3>{copy.question}</h3>
      <div className="modal-scroll">
        {previously ? <p className="st-muted">{previously} Doing it again is harmless.</p> : null}
        {copy.explain.map((line, i) => (
          <p key={i} className="st-muted">
            {line}
          </p>
        ))}
        {copy.mail ? (
          <dl className="unsub-mail">
            <dt>From</dt>
            <dd>{copy.mail.from}</dd>
            <dt>To</dt>
            <dd className="mono">{copy.mail.to}</dd>
            <dt>Subject</dt>
            <dd>{copy.mail.subject}</dd>
          </dl>
        ) : null}
        {copy.action === "link" && copy.url ? <UrlLine url={copy.url} copy={() => void copyText(copy.url!, "Link copied")} /> : null}
      </div>
      <div className="st-confirm-actions">
        {copy.editFirst ? (
          <button type="button" className="btn btn-ghost modal-actions-spacer" onClick={() => onPick("editFirst")}>
            Edit first
          </button>
        ) : null}
        <button type="button" className="btn btn-ghost" onClick={cancel}>
          Cancel
        </button>
        <button ref={confirmRef} type="button" className="btn btn-primary" onClick={confirm}>
          <Icon name={icon} size="xs" />
          {copy.verb}
        </button>
      </div>
    </Modal>
  );
}
