// The summary card at the top of a thread, and the Summarize button in the
// thread toolbar. OWNER: summaries. States: model.ts. Every point and
// request links to the message it came from.
import { useEffect, useState } from "react";
import type { AiSummary, ThreadRef, ThreadView } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { Keys } from "../../components/Kbd";
import { isMe } from "../../app/store";
import { useKeyTip } from "../../lib/shortcutHints";
import { useUi } from "../../lib/ui";
import { cardView, footnote, sourceLabel, type CardView } from "./model";
import { cancel, close, loadCached, prewarm, summarize, SUMMARIZE_KEYS, toggle, useCanSummarize, useCard } from "./state";
import "./summary.css";

/** At the top of the thread, above the messages. `onJump` opens a message and flashes it. */
export function SummaryCard({ thread, onJump }: { thread: ThreadView; onJump: (messageId: string) => void }) {
  const ref: ThreadRef = { accountId: thread.accountId, threadId: thread.threadId };
  const can = useCanSummarize();
  const state = useCard(ref);
  const lastId = thread.messages[thread.messages.length - 1]?.id;

  // A stored summary shows by itself; read again when the thread changes.
  useEffect(() => {
    if (can) void loadCached(ref);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [can, ref.accountId, ref.threadId, lastId, thread.messages.length]);

  const view = cardView(state);
  if (view.kind === "none") return null;
  return <Card view={view} thread={thread} refs={ref} onJump={onJump} />;
}

function Card({ view, thread, refs, onJump }: { view: Exclude<CardView, { kind: "none" }>; thread: ThreadView; refs: ThreadRef; onJump: (id: string) => void }) {
  const busy = view.kind === "starting" || view.kind === "reading" || view.kind === "writing";
  const summary = view.kind === "writing" || view.kind === "summary" ? view.summary : null;
  return (
    <section
      className={"ai-card" + (busy ? " is-busy" : "") + (view.kind === "summary" && view.stale ? " is-stale" : "") + (view.kind === "error" ? " is-error" : "")}
      aria-label="Conversation summary"
      aria-busy={busy}
    >
      <header className="ai-card-head">
        <Icon name="sparkles" size="xs" className="ai-card-mark" />
        <span className="ai-card-title">{busy ? "Summarizing…" : "Summary"}</span>
        <span className="ai-card-status" aria-live="polite">
          {view.kind === "reading" && view.steps > 1 ? `Reading part ${view.step} of ${view.steps - 1}` : null}
          {view.kind === "starting" ? "Apple Intelligence, on this Mac" : null}
        </span>
        <span className="grow" />
        {busy ? (
          <button className="btn btn-ghost btn-sm" onClick={() => cancel(refs)}>
            Stop
          </button>
        ) : (
          <>
            {view.kind !== "error" && (
              <button className="btn btn-ghost btn-sm" title="Summarize again" onClick={() => void summarize(refs)}>
                <Icon name="refresh" size="xs" />
                Regenerate
              </button>
            )}
            <button className="btn btn-ghost btn-sm ai-card-close" title="Hide the summary" aria-label="Hide the summary" onClick={() => close(refs)}>
              <Icon name="x" size="xs" />
            </button>
          </>
        )}
      </header>

      {view.kind === "summary" && view.stale && (
        <div className="ai-card-stale">
          <span>New messages since this summary.</span>
          <button className="btn btn-secondary btn-sm" onClick={() => void summarize(refs)}>
            Update
          </button>
        </div>
      )}

      {view.kind === "error" ? (
        <div className="ai-card-error" role="alert">
          <span>{view.message}</span>
          <button className="btn btn-secondary btn-sm" onClick={() => void summarize(refs)}>
            Try again
          </button>
        </div>
      ) : summary ? (
        <Body summary={summary} thread={thread} onJump={onJump} streaming={view.kind === "writing"} />
      ) : (
        <div className="ai-card-skeleton" aria-hidden="true">
          <span className="sk" style={{ width: "92%" }} />
          <span className="sk" style={{ width: "76%" }} />
          <span className="sk" style={{ width: "58%" }} />
        </div>
      )}

      {view.kind === "summary" && <Footnote summary={view.summary} />}
    </section>
  );
}

function Body({ summary, thread, onJump, streaming }: { summary: AiSummary; thread: ThreadView; onJump: (id: string) => void; streaming: boolean }) {
  const link = (id: string | null) => {
    const label = sourceLabel(id, thread.messages, isMe);
    if (!id || !label) return null;
    return (
      <button type="button" className="ai-src" title="Show this message" onClick={() => onJump(id)}>
        {label}
      </button>
    );
  };
  return (
    <div className="ai-card-body">
      {summary.gist ? <p className="ai-gist">{summary.gist}</p> : streaming ? <span className="sk ai-gist-sk" aria-hidden="true" /> : null}
      {summary.points.length > 0 && (
        <ul className="ai-points">
          {summary.points.map((p, i) => (
            <li key={i}>
              <span className="ai-text">{p.text}</span> {link(p.messageId)}
            </li>
          ))}
        </ul>
      )}
      {summary.asks.length > 0 && (
        <div className="ai-asks">
          <div className="ai-asks-title">Asked of you</div>
          <ul>
            {summary.asks.map((a, i) => (
              <li key={i}>
                <Icon name="reply" size="xs" className="ai-ask-mark" />
                <span className="ai-text">{a.text}</span>
                {a.due && <span className="ai-due">due {a.due}</span>} {link(a.messageId)}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

function Footnote({ summary }: { summary: AiSummary }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(t);
  }, []);
  return <p className="ai-foot">{footnote(summary, now)}. Summaries can be wrong: check the linked messages.</p>;
}

/**
 * The toolbar's Summarize button: shown only when summaries can run. No text
 * label, so the toolbar keeps room for its other actions: the sparkle and ⇧S
 * in the opened thread, the sparkle alone in the narrow preview pane. The
 * tooltip and accessible name say what a click does.
 */
export function SummarizeButton({ compact }: { compact: boolean }) {
  const tip = useKeyTip();
  const can = useCanSummarize();
  const ref = useSelectedRef();
  const state = useCard(ref ?? NO_REF);
  if (!can || !ref) return null;
  const open = state.open && !!state.summary && !state.run;
  const label = state.run ? "Summarizing…" : open ? "Hide summary" : "Summarize";
  return (
    <button
      className={"btn btn-ghost btn-sm ai-btn" + (state.run ? " is-busy" : "")}
      disabled={!!state.run}
      title={tip(label, "⇧S")}
      aria-label={label}
      aria-pressed={open}
      onPointerEnter={prewarm}
      onFocus={prewarm}
      onClick={() => toggle(ref)}
    >
      <Icon name="sparkles" size="xs" />
      {!compact && <Keys keys={SUMMARIZE_KEYS} />}
    </button>
  );
}

const NO_REF: ThreadRef = { accountId: "", threadId: "" };

function useSelectedRef(): ThreadRef | null {
  return useUi((s) => s.selected);
}
