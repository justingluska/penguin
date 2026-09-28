// Send later + "remind me if no reply": the composer's pickers, and the toasts
// for what the app's local scheduler does (it only runs while Penguin does,
// which the UI says wherever you schedule something).
import { useEffect, useMemo, useRef, useState } from "react";
import { api, asCommandError, onReminderDue, onScheduledSent } from "../../lib/api";
import { openThread } from "../../lib/ui";
import { useSetting } from "../../lib/settings";
import { parseWhen, sendLaterPresets } from "./when";
import type { ScheduledSend } from "../../lib/types";
import { toast } from "../../components/Toast";
import { Icon } from "../../components/Icon";

export const RUNNING_NOTE = "Sends when Penguin is running";

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

/** "Today, 5:00 PM" · "Tomorrow, 8:00 AM" · "Mon, Sep 28, 8:00 AM". */
export function fmtWhen(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  const days = Math.floor((ms - start.getTime()) / DAY);
  if (days === 0) return `Today, ${time}`;
  if (days === 1) return `Tomorrow, ${time}`;
  return `${d.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" })}, ${time}`;
}

export { sendLaterPresets, type SendLaterPreset } from "./when";

export const REMIND_CHOICES: Array<{ ms: number | null; label: string }> = [
  { ms: null, label: "Off" },
  { ms: DAY, label: "1 day" },
  { ms: 2 * DAY, label: "2 days" },
  { ms: 3 * DAY, label: "3 days" },
  { ms: 7 * DAY, label: "1 week" },
];

export function remindLabel(ms: number | null): string {
  return REMIND_CHOICES.find((c) => c.ms === ms)?.label ?? `${Math.round((ms ?? 0) / DAY)} days`;
}

/** `<input type=datetime-local>` value for a unix ms time (local clock). */
function localInput(ms: number): string {
  const d = new Date(ms - new Date(ms).getTimezoneOffset() * 60_000);
  return d.toISOString().slice(0, 16);
}

export function SendLaterMenu({
  onPick,
  onClose,
  scheduled,
}: {
  onPick: (at: number) => void;
  onClose: () => void;
  scheduled: ScheduledSend | null;
}) {
  const morningHour = useSetting("sendLaterHour");
  const presets = useMemo(() => sendLaterPresets(new Date(), morningHour), [morningHour]);
  const [idx, setIdx] = useState(0);
  const [custom, setCustom] = useState(() => localInput(presets[0]?.at ?? Date.now() + DAY));
  const [customError, setCustomError] = useState<string | null>(null);
  const [words, setWords] = useState("");
  const wordsAt = useMemo(() => (words.trim() ? parseWhen(words, new Date(), morningHour) : null), [words, morningHour]);
  const ref = useRef<HTMLDivElement>(null);
  const wordsRef = useRef<HTMLInputElement>(null);
  useEffect(() => ref.current?.focus(), []);

  function check(ms: number): boolean {
    if (Number.isNaN(ms) || ms <= Date.now()) {
      setCustomError("Pick a time in the future.");
      return false;
    }
    if (ms > Date.now() + 365 * DAY) {
      setCustomError("Pick a time within a year.");
      return false;
    }
    return true;
  }

  function pickCustom() {
    const ms = new Date(custom).getTime();
    if (check(ms)) onPick(ms);
  }

  function pickWords() {
    if (wordsAt === null) return setCustomError("Couldn't read that time. Try “tomorrow 9am”, “fri 3pm” or “in 2 hours”.");
    if (check(wordsAt)) onPick(wordsAt);
  }

  return (
    <div
      ref={ref}
      tabIndex={-1}
      className="panel menu cmp-later"
      role="menu"
      onKeyDown={(e) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          setIdx((i) => (i + (e.key === "ArrowDown" ? 1 : presets.length - 1)) % presets.length);
        } else if (e.key === "Enter" && (e.target as HTMLElement).tagName !== "INPUT") {
          e.preventDefault();
          onPick(presets[idx].at);
        } else if (/^[1-9]$/.test(e.key) && (e.target as HTMLElement).tagName !== "INPUT" && presets[Number(e.key) - 1]) {
          // 1–5 pick a preset (like the snooze picker); Tab reaches the custom time.
          e.preventDefault();
          onPick(presets[Number(e.key) - 1].at);
        } else if (/^[a-z]$/i.test(e.key) && !e.metaKey && !e.ctrlKey && !e.altKey && (e.target as HTMLElement).tagName !== "INPUT") {
          // Typing a word starts the "type a time" field.
          e.preventDefault();
          setWords(e.key);
          setCustomError(null);
          wordsRef.current?.focus();
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="cmp-later-head">
        <span>Send later</span>
        <span className="faint">{RUNNING_NOTE}</span>
      </div>
      {presets.map((p, i) => (
        <div
          key={p.id}
          role="menuitem"
          className={`menu-item${i === idx ? " active" : ""}`}
          onMouseMove={() => setIdx(i)}
          onClick={() => onPick(p.at)}
        >
          <Icon name="clock" size="xs" />
          <span>{p.label}</span>
          <span className="faint cmp-later-when">{fmtWhen(p.at)}</span>
          <span className="kbd">{i + 1}</span>
        </div>
      ))}
      <div className="cmp-later-custom cmp-later-words">
        <input
          ref={wordsRef}
          className="input"
          value={words}
          onChange={(e) => {
            setWords(e.target.value);
            setCustomError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              pickWords();
            }
          }}
          placeholder="Type a time: tomorrow 9am, fri 3pm, in 2 hours"
          aria-label="Send time in words"
          spellCheck={false}
        />
        <span className={`cmp-later-parsed${wordsAt === null ? " faint" : ""}`} aria-live="polite">
          {words.trim() ? (wordsAt !== null ? fmtWhen(wordsAt) : "…") : ""}
        </span>
      </div>
      <div className="cmp-later-custom">
        <input
          type="datetime-local"
          className="input"
          value={custom}
          min={localInput(Date.now())}
          onChange={(e) => {
            setCustom(e.target.value);
            setCustomError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              pickCustom();
            }
          }}
          aria-label="Custom send time"
        />
        <button className="btn btn-secondary btn-sm" onClick={pickCustom}>
          Schedule
        </button>
      </div>
      {customError && <div className="cmp-later-error">{customError}</div>}
      {scheduled && <div className="cmp-later-foot faint">Currently scheduled for {fmtWhen(scheduled.sendAt)}. Picking a time replaces it.</div>}
    </div>
  );
}

export function RemindMenu({ value, onPick, onClose }: { value: number | null; onPick: (ms: number | null) => void; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [idx, setIdx] = useState(() => Math.max(0, REMIND_CHOICES.findIndex((c) => c.ms === value)));
  useEffect(() => ref.current?.focus(), []);
  return (
    <div
      ref={ref}
      tabIndex={-1}
      className="panel menu cmp-remind-menu"
      role="menu"
      onKeyDown={(e) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          setIdx((i) => (i + (e.key === "ArrowDown" ? 1 : REMIND_CHOICES.length - 1)) % REMIND_CHOICES.length);
        } else if (e.key === "Enter") {
          e.preventDefault();
          onPick(REMIND_CHOICES[idx].ms);
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="cmp-later-head">
        <span>Remind me if no reply in…</span>
      </div>
      {REMIND_CHOICES.map((c, i) => (
        <div
          key={c.label}
          role="menuitemradio"
          aria-checked={c.ms === value}
          className={`menu-item${i === idx ? " active" : ""}`}
          onMouseMove={() => setIdx(i)}
          onClick={() => onPick(c.ms)}
        >
          <span>{c.label}</span>
          {c.ms === value && <Icon name="check" size="xs" className="accent-ico" />}
        </div>
      ))}
      <div className="cmp-later-foot faint">The thread comes back to your inbox, unread. Checked while Penguin is running.</div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Toasts for what the scheduler did
// ---------------------------------------------------------------------------

async function threadSubject(accountId: string, threadId: string): Promise<string | null> {
  try {
    return (await api.getThread(accountId, threadId))?.subject || null;
  } catch {
    return null; // the toast still makes sense without a subject
  }
}

/** Mounted once (by the Compose root). */
export function useOutboxToasts(open: { draft: (accountId: string, draftId: string) => void }) {
  const openRef = useRef(open);
  openRef.current = open;
  useEffect(() => {
    const unSent = onScheduledSent((b) => {
      const missed = Math.min(b.missed, b.sent.length);
      if (missed > 0) {
        toast({ kind: "success", message: `Sent ${missed} scheduled ${missed === 1 ? "email" : "emails"}`, detail: `${missed === 1 ? "It was" : "They were"} due while Penguin was closed` });
      }
      const onTime = b.sent.slice(missed);
      if (onTime.length === 1) {
        const s = onTime[0];
        void threadSubject(s.accountId, s.threadId).then((subject) =>
          toast({
            kind: "success",
            message: "Scheduled email sent",
            detail: subject || undefined,
            action: { label: "Open", run: () => openThread({ accountId: s.accountId, threadId: s.threadId }) },
          }),
        );
      } else if (onTime.length > 1) {
        toast({ kind: "success", message: `Sent ${onTime.length} scheduled emails` });
      }
      for (const f of b.failed) {
        toast({
          kind: "error",
          message: "A scheduled email wasn't sent",
          detail: `${f.message}. Its draft (if it still exists) is in Drafts.`,
          title: f.message,
          action: { label: "Open draft", run: () => openRef.current.draft(f.accountId, f.draftId) },
        });
      }
    });
    const unDue = onReminderDue((r) =>
      toast({
        kind: "action",
        key: `reminder:${r.accountId}:${r.threadId}`,
        message: "No reply yet",
        detail: r.subject || "A message you asked to be reminded about",
        action: { label: "Open", run: () => openThread({ accountId: r.accountId, threadId: r.threadId }) },
        duration: 12_000,
      }),
    );
    return () => {
      unSent.then((f) => f());
      unDue.then((f) => f());
    };
  }, []);
}

export function scheduleErrorText(e: unknown): string {
  return `Couldn't schedule: ${asCommandError(e).message}`;
}
