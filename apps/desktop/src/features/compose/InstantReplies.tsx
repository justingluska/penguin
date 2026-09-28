// The instant-reply row: in the reply composer while nothing is written yet
// (⌃1–⌃9 or a click puts one in the message; ⌘↵ sends it), and in the
// thread's reply box (a click opens the reply with it). Choices come from
// instant.ts; Settings → Compose edits them.
import { useSetting } from "../../lib/settings";
import { Icon } from "../../components/Icon";
import { isMac } from "../../lib/keyboard";
import { instantChoices, setComposeIntent, type InstantChoice } from "./instant";

const CTRL = isMac ? "⌃" : "Ctrl+";

export function InstantRow({
  choices,
  suggesting,
  onPick,
}: {
  choices: InstantChoice[];
  /** The on-device model is still thinking of suggestions. */
  suggesting: boolean;
  onPick: (c: InstantChoice) => void;
}) {
  if (choices.length === 0 && !suggesting) return null;
  return (
    <div className="cmp-instant" role="toolbar" aria-label="Instant replies" onMouseDown={(e) => e.preventDefault()}>
      <span className="cmp-instant-label faint">Quick reply</span>
      {choices.map((c) => (
        <button
          key={`${c.ai ? "ai" : "me"}:${c.text}`}
          type="button"
          className={`cmp-instant-chip${c.ai ? " is-ai" : ""}`}
          onClick={() => onPick(c)}
          title={c.ai ? "Suggested by Apple Intelligence on this Mac" : undefined}
        >
          {c.ai && <Icon name="sparkles" size="2xs" className="cmp-instant-ai" />}
          <span className="truncate">{c.text}</span>
          <span className="kbd">
            {CTRL}
            {c.n}
          </span>
        </button>
      ))}
      {suggesting && (
        <span className="cmp-instant-chip is-ai is-loading" aria-live="polite">
          <Icon name="sparkles" size="2xs" className="cmp-instant-ai" />
          Suggesting…
        </span>
      )}
    </div>
  );
}

/** The thread's reply box: your one-liners; a click opens the reply with it (still yours to send). */
export function InstantDockRow({ onOpen }: { onOpen: () => void }) {
  const settings = useSetting("instantReplies");
  const choices = instantChoices({ ...settings, aiSuggestions: false });
  if (choices.length === 0) return null;
  return (
    <div className="reply-instant" role="toolbar" aria-label="Instant replies">
      {choices.map((c) => (
        <button
          key={c.text}
          type="button"
          className="cmp-instant-chip"
          onClick={() => {
            setComposeIntent({ text: c.text });
            onOpen();
          }}
          title="Opens the reply with this text. ⌘↵ sends it."
        >
          <span className="truncate">{c.text}</span>
        </button>
      ))}
    </div>
  );
}
