// The privacy row's summary ("3 trackers removed · 2 links cleaned") → this
// dialog: what each tracker was, what it would have told the sender, and
// what Penguin did with it; the blocked remote images and what "Load images"
// does; and the tracking removed from links. Opened from the message
// privacy row (MessageBody) and from Message details.
import { useRef } from "react";
import type { MessageView, TrackerStatus } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { Modal, openModal } from "../../components/Modal";
import { useSetting } from "../../lib/settings";
import { openSettings } from "../settings/state";
import { QUERY_NOTE, allowedTrackers, linkCopy, privacySummary, trackerCopy, unlistedTrackers } from "./privacy";

/** Open the dialog for `message`; `onLoadImages` when images can be loaded now. */
export function openPrivacyDetails(message: MessageView, onLoadImages?: () => void) {
  openModal((close) => <PrivacyDialog message={message} onClose={close} onLoadImages={onLoadImages} />);
}

const STATUS_TEXT: Record<TrackerStatus, string> = { removed: "Removed", held: "Waiting with images", loaded: "Loaded" };

export function PrivacyDialog({
  message,
  onClose,
  onLoadImages,
}: {
  message: MessageView;
  onClose: () => void;
  /** Present when images can be loaded for this message (policy allows it). */
  onLoadImages?: () => void;
}) {
  const doneRef = useRef<HTMLButtonElement>(null);
  const stripping = useSetting("stripLinkTracking");
  const removed = message.trackersRemoved;
  const allowed = allowedTrackers(message);
  const found = removed + allowed.count;
  const blocked = message.blockedRemoteImages;
  const list = message.trackers ?? [];
  const more = unlistedTrackers(found, list);
  const summary = privacySummary(message);
  const title = summary ? summary.label.replace(/^./, (c) => c.toUpperCase()) : "Privacy protection";
  const links = linkCopy(message, stripping);

  return (
    <Modal label={title} onClose={onClose} onEnter={onClose} initialFocus={doneRef} className="pv-dialog">
      <h3 className="pv-title">
        <Icon name={summary?.tone === "warn" ? "eye" : "shield"} size="sm" className={summary?.tone === "warn" ? "is-warn" : undefined} />
        {title}
      </h3>
      <div className="modal-scroll">
        {found > 0 ? (
          <>
            <p className="st-muted">
              Senders put tiny invisible images in email. When your mail app loads one, the sender's server learns{" "}
              <b>that you opened the email</b>, <b>when</b> (and how often), <b>roughly where you are</b> (from your IP address), and{" "}
              <b>what device and app</b> you used.
            </p>
            {allowed.count === 0 ? (
              <p className="st-muted">
                Penguin removed {removed === 1 ? "it" : "them"} before this email was shown, so none of that was sent. They stay removed even
                if you load images.
              </p>
            ) : allowed.loaded ? (
              <p className="st-muted">
                <b>Blocking tracking pixels is off</b>, so {allowed.count === 1 ? "this one" : "these"} loaded with the images: the sender can
                tell you opened this email.
              </p>
            ) : (
              <p className="st-muted">
                <b>Blocking tracking pixels is off</b>, so {allowed.count === 1 ? "this one is" : "these are"} treated like the other
                pictures: held back now, and loaded if you choose Load images.
              </p>
            )}
            <ul className="pv-list">
              {list.map((t, i) => {
                const c = trackerCopy(t);
                return (
                  <li key={i} className="pv-item">
                    <div className="pv-item-head">
                      <span className="pv-item-title">{c.title}</span>
                      <span className={"pv-tag" + (t.kind === "hiddenImage" ? " is-hidden" : "")}>{c.tag}</span>
                      {t.count > 1 ? <span className="pv-count">×{t.count}</span> : null}
                      {t.status && t.status !== "removed" ? (
                        <span className={"pv-status" + (t.status === "loaded" ? " is-loaded" : "")}>{STATUS_TEXT[t.status]}</span>
                      ) : null}
                    </div>
                    <div className="pv-addr" title={c.address}>
                      {c.address}
                      {t.hadQuery ? <span className="pv-query">?…</span> : null}
                    </div>
                    {c.lines.map((l, j) => (
                      <p key={j} className="pv-line">
                        {l}
                      </p>
                    ))}
                  </li>
                );
              })}
            </ul>
            {more > 0 ? <p className="st-muted pv-more">And {more} more, not listed.</p> : null}
            {list.some((t) => t.hadQuery) ? <p className="st-muted pv-more">{QUERY_NOTE}</p> : null}
          </>
        ) : (
          <p className="st-muted">No trackers were found in this email.</p>
        )}

        <div className="pv-section">
          <Icon name="image" size="sm" />
          <span>{blocked > 0 ? `Remote images blocked (${blocked})` : "Remote images"}</span>
        </div>
        {blocked > 0 ? (
          <>
            <p className="st-muted">
              Ordinary pictures (logos, photos) also load from the sender's servers, so loading them tells the sender the same things: that
              you opened the email, when, and roughly where. Penguin holds them back until you ask.
            </p>
            <p className="st-muted">
              <b>Load images</b> shows them for this message only. <b>Always from this sender</b> loads them automatically for mail that
              Gmail verified came from that address. {allowed.count === 0 ? "Trackers stay removed either way." : ""}
            </p>
          </>
        ) : (
          <p className="st-muted">None are blocked in this message: it has none, or they were loaded.</p>
        )}

        {links ? (
          <>
            <div className="pv-section">
              <Icon name="link" size="sm" />
              <span>Links</span>
            </div>
            {links.map((l, i) => (
              <p key={i} className="st-muted">
                {l}
              </p>
            ))}
          </>
        ) : null}
      </div>
      <div className="st-confirm-actions">
        <button
          type="button"
          className="btn btn-ghost modal-actions-spacer"
          onClick={() => {
            onClose();
            openSettings("privacy");
          }}
        >
          Privacy settings
        </button>
        {blocked > 0 && onLoadImages ? (
          <button
            type="button"
            className="btn btn-secondary"
            onClick={() => {
              onLoadImages();
              onClose();
            }}
          >
            Load images
          </button>
        ) : null}
        <button ref={doneRef} type="button" className="btn btn-primary" onClick={onClose}>
          Done
        </button>
      </div>
    </Modal>
  );
}
