// The event card at the top of a thread that carries a calendar invitation
// (a text/calendar part or an .ics file): what, when (yours and the
// organizer's time), where, who, your answer with Yes / Maybe / No, a note
// to the organizer, "Propose new time", "Open in Calendar", what changed in
// an update, and a clear banner when it's cancelled. Answers go through the
// account's route (card.route: Google Calendar, Microsoft Graph or an iMIP
// email), only on an explicit click. Renders nothing for other threads.
// OWNER: calendar agent.
import { useEffect, useState } from "react";
import type { CalendarEvent, InviteAnswer, InviteCard as Card, ThreadView } from "../../lib/types";
import { api } from "../../lib/api";
import { accountTone } from "../../lib/accountColor";
import { Icon } from "../../components/Icon";
import { accountById } from "../../app/store";
import { accountName } from "../../components/Identity";
import { openSettings } from "../settings/state";
import { openEvent, showInCalendar, useCalendarVersion } from "./state";
import { isUrl, joinLabel, mapUrl, relative, timeRange, whenLabel } from "./time";
import {
  ANSWERS,
  answerWord,
  changeLines,
  defaultProposal,
  guestList,
  guestSummary,
  inviteKey,
  organizerTime,
  parseProposal,
  proposalLabel,
  responseLabel,
  routeHint,
  shownResponse,
  type ProposalFields,
} from "./invite";
import { answerInvite, emailHintSeen, useInviteOverride } from "./inviteState";
import "./calendar.css";
import "./invites.css";

/** Mirrors the backend's is_calendar_part (the card is fetched only then). */
export function hasInvitePart(thread: ThreadView): boolean {
  return thread.messages.some((m) =>
    m.attachments.some((a) => {
      const mime = a.mimeType.toLowerCase();
      return mime === "text/calendar" || mime === "application/ics" || a.filename.toLowerCase().endsWith(".ics");
    }),
  );
}

const LOCAL_ZONE = Intl.DateTimeFormat().resolvedOptions().timeZone;

type Panel = null | "note" | "propose";

export function InviteCard({ thread }: { thread: ThreadView }) {
  const v = useCalendarVersion();
  const invite = hasInvitePart(thread);
  const [card, setCard] = useState<Card | null>(null);
  const [panel, setPanel] = useState<Panel>(null);
  useEffect(() => {
    if (!invite) {
      setCard(null);
      return;
    }
    let live = true;
    api.eventInvite(thread.accountId, thread.threadId).then(
      (c) => live && setCard(c),
      // Offline with the .ics not cached yet, or unparsable: no card.
      () => live && setCard(null),
    );
    return () => {
      live = false;
    };
    // The newest message changing (an update arrived) reloads it too.
  }, [invite, thread.accountId, thread.threadId, thread.messages.length, v]);
  useEffect(() => setPanel(null), [thread.threadId]);

  const key = card ? inviteKey(card.accountId, { uid: card.uid, messageId: card.messageId }) : null;
  const o = useInviteOverride(key);
  if (!card) return null;
  const e = card.event;
  const now = Date.now();
  const cancelled = card.method === "cancel";
  const over = e.end < now && !card.recurring;
  const mine = shownResponse(card.response, o);
  const pending = !!o?.pending;
  const acct = accountById(card.accountId);
  const organizer = e.organizer ? (e.organizer.name ?? e.organizer.email) : null;
  const theirTime = organizerTime(e.start, e.allDay, card.timeZone, LOCAL_ZONE);
  const changes = card.updated ? changeLines(card) : [];
  const hint = card.canRespond && !emailHintSeen() ? routeHint(card, organizer) : null;

  async function send(response: InviteAnswer, extra: { comment?: string | null; proposal?: { start: number; end: number } | null } = {}) {
    if (!card) return false;
    const fresh = await answerInvite({
      accountId: card.accountId,
      threadId: card.threadId,
      messageId: card.messageId,
      uid: card.uid,
      current: card.response,
      response,
      comment: extra.comment,
      proposal: extra.proposal,
      organizer: e.organizer?.name?.split(/\s+/)[0] ?? e.organizer?.email ?? null,
    });
    if (fresh) setCard(fresh);
    return !!fresh;
  }

  return (
    <div
      className={"invite card card-sm" + (cancelled ? " is-cancelled" : "") + (acct ? ` t-${accountTone(acct.color)}` : "")}
      role="region"
      aria-label={`Invitation: ${e.summary || "(no title)"}`}
    >
      <div className="inv-date" aria-hidden="true">
        <span>{new Intl.DateTimeFormat(undefined, { month: "short" }).format(e.start)}</span>
        <b className="tnum">{new Date(e.start).getDate()}</b>
      </div>
      <div className="grow min0">
        {cancelled && (
          <div className="inv-banner is-cancel" role="status">
            <Icon name="x" size="xs" />
            <span>
              Canceled{organizer ? ` by ${organizer}` : ""}. {card.inCalendar ? "It's still on your calendar until it syncs." : "It's no longer happening."}
            </span>
          </div>
        )}
        {!cancelled && card.updated && (
          <div className="inv-banner is-update" role="status">
            <Icon name="refresh" size="xs" />
            <span>{changes.length ? "Updated invitation. What changed:" : "Updated invitation"}</span>
          </div>
        )}
        {changes.length > 0 && (
          <dl className="inv-changes">
            {changes.map((c) => (
              <div key={c.field} style={{ display: "contents" }}>
                <dt>{c.label}</dt>
                <dd>
                  {c.field === "guests" ? (
                    c.after
                  ) : (
                    <>
                      <s>{c.before}</s> → {c.after}
                    </>
                  )}
                </dd>
              </div>
            ))}
          </dl>
        )}
        <div className="inv-kicker faint small">
          {cancelled
            ? "Cancelled event"
            : card.method === "reply"
              ? "Response to your event"
              : card.method === "counter"
                ? "New time proposed for your event"
                : card.recurring
                  ? "Recurring invitation · next occurrence"
                  : "Invitation"}
          {!card.inCalendar && !cancelled && card.calendarConnected && <span> · Not on your calendar yet</span>}
        </div>
        <button
          className={"inv-title" + (cancelled ? " is-struck" : "")}
          onClick={(ev) => (card.inCalendar ? openEvent(e, ev.currentTarget) : undefined)}
          disabled={!card.inCalendar}
          title={card.inCalendar ? "Event details" : undefined}
        >
          {e.summary || "(no title)"}
        </button>
        <div className="inv-line">
          <Icon name="clock" size="xs" />
          <span>
            {whenLabel(e, now)}
            {!e.allDay && !over && <span className="faint"> · {relative(e.start, now)}</span>}
            {theirTime && <span className="faint"> · {theirTime}</span>}
          </span>
        </div>
        {e.location && !isUrl(e.location) && (
          <div className="inv-line">
            <Icon name="mapPin" size="xs" />
            <span className="truncate">{e.location}</span>
            <button className="pc-inline-link small" onClick={() => void api.openExternal(mapUrl(e.location))} title="Open in Maps">
              Map
            </button>
          </div>
        )}
        {e.organizer && (
          <div className="inv-line faint">
            <Icon name="user" size="xs" />
            <span className="truncate">Organized by {organizer}</span>
          </div>
        )}
        <Guests event={e} />
        {card.conflicts.length > 0 && !cancelled && mine !== "declined" && (
          <div className="inv-conflict">
            <Icon name="info" size="xs" />
            <span>
              Conflicts with{" "}
              {card.conflicts.slice(0, 2).map((c, i) => (
                <span key={c.accountId + c.id}>
                  {i > 0 && ", "}
                  <button className="pc-inline-link" onClick={(ev) => openEvent(c, ev.currentTarget)}>
                    {c.summary || "(no title)"}
                  </button>{" "}
                  <span className="faint">
                    {timeRange(c)}
                    {c.accountId !== card.accountId && accountById(c.accountId) ? ` · ${accountShort(c)}` : ""}
                  </span>
                </span>
              ))}
              {card.conflicts.length > 2 && <span className="faint"> +{card.conflicts.length - 2} more</span>}
            </span>
          </div>
        )}
        <div className="inv-actions">
          {card.canRespond ? (
            <>
              <span className="small">Going?</span>
              <div className="setting-choice inv-rsvp" role="radiogroup" aria-label="Your answer">
                {ANSWERS.map((r) => (
                  <button
                    key={r}
                    role="radio"
                    aria-checked={mine === r}
                    className={(mine === r ? "selected" : "") + (pending && mine === r ? " is-pending" : "")}
                    disabled={pending}
                    onClick={() => void send(r)}
                  >
                    {answerWord(r)}
                  </button>
                ))}
              </div>
            </>
          ) : (
            mine &&
            !cancelled &&
            responseLabel(mine) && (
              <span className={"badge " + (mine === "accepted" ? "t-green" : mine === "declined" ? "t-red" : "t-amber")}>{answerLabel(mine)}</span>
            )
          )}
          {e.conferenceUrl && !over && !cancelled && (
            <button className="btn btn-secondary btn-sm" onClick={() => void api.openExternal(e.conferenceUrl!)}>
              {joinLabel(e.conferenceKind)}
            </button>
          )}
          <span className="grow" />
          {!card.calendarConnected && acct?.capabilities?.calendar ? (
            <button className="pc-inline-link small" onClick={() => openSettings("calendar")}>
              Connect calendar
            </button>
          ) : (
            card.inCalendar && (
              <button className="pc-inline-link small" onClick={() => showInCalendar(e)}>
                Open in Calendar
              </button>
            )
          )}
        </div>
        {card.canRespond && (
          <div className="inv-more">
            <button className="pc-inline-link small" aria-expanded={panel === "note"} onClick={() => setPanel(panel === "note" ? null : "note")}>
              <Icon name="pencil" size="xs" /> {card.sent?.comment ? "Edit note" : "Add a note"}
            </button>
            {card.canPropose && (
              <button className="pc-inline-link small" aria-expanded={panel === "propose"} onClick={() => setPanel(panel === "propose" ? null : "propose")}>
                <Icon name="calendar" size="xs" /> Propose new time
              </button>
            )}
          </div>
        )}
        {panel === "note" && (
          <NotePanel
            initial={card.sent?.comment ?? ""}
            current={mine}
            to={organizer}
            onCancel={() => setPanel(null)}
            onSend={async (r, comment) => {
              if (await send(r, { comment })) setPanel(null);
            }}
          />
        )}
        {panel === "propose" && (
          <ProposePanel
            event={e}
            route={card.route}
            onCancel={() => setPanel(null)}
            onSend={async (r, proposal, comment) => {
              if (await send(r, { proposal, comment })) setPanel(null);
            }}
          />
        )}
        {card.sent && !cancelled && <SentLine card={card} />}
        {hint && (
          <div className="inv-hint">
            {hint}{" "}
            {card.rsvpAvailable && (
              <button className="pc-inline-link" onClick={() => openSettings("calendar")}>
                Turn on RSVP
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function Guests({ event }: { event: CalendarEvent }) {
  const people = guestList(event.attendees);
  if (people.length < 2) return null;
  return (
    <details className="inv-guests">
      <summary className="faint">
        <Icon name="users" size="xs" />
        <span>
          {people.length} guests · {guestSummary(people)}
        </span>
      </summary>
      <ul className="inv-guest-list">
        {people.map((a) => (
          <li key={a.email}>
            <span className={"inv-guest-dot is-" + a.response} aria-hidden="true" />
            <span className="truncate">
              {a.name ?? a.email}
              {a.self && " (you)"}
              {a.organizer && <span className="faint"> · organizer</span>}
              {a.optional && <span className="faint"> · optional</span>}
            </span>
            <span className="faint small">{a.response === "needsAction" ? "awaiting" : (responseLabel(a.response) ?? "").toLowerCase()}</span>
          </li>
        ))}
      </ul>
    </details>
  );
}

function NotePanel({
  initial,
  current,
  to,
  onCancel,
  onSend,
}: {
  initial: string;
  current: Card["response"];
  to: string | null;
  onCancel: () => void;
  onSend: (r: InviteAnswer, comment: string) => Promise<void>;
}) {
  const [text, setText] = useState(initial);
  const [answer, setAnswer] = useState<InviteAnswer>(current === "tentative" || current === "declined" ? current : "accepted");
  const [busy, setBusy] = useState(false);
  return (
    <form
      className="inv-compose"
      onSubmit={async (ev) => {
        ev.preventDefault();
        setBusy(true);
        await onSend(answer, text);
        setBusy(false);
      }}
      onKeyDown={(ev) => {
        if (ev.key === "Escape") {
          ev.stopPropagation();
          onCancel();
        }
      }}
    >
      <textarea
        autoFocus
        aria-label={`Note to ${to ?? "the organizer"}`}
        placeholder={`A note to ${to ?? "the organizer"} (everyone on the invite may see it)`}
        value={text}
        maxLength={2000}
        onChange={(ev) => setText(ev.target.value)}
      />
      <div className="inv-compose-row">
        <span>Send with</span>
        <div className="setting-choice inv-rsvp" role="radiogroup" aria-label="Answer to send with the note">
          {ANSWERS.map((r) => (
            <button key={r} type="button" role="radio" aria-checked={answer === r} className={answer === r ? "selected" : ""} onClick={() => setAnswer(r)}>
              {answerWord(r)}
            </button>
          ))}
        </div>
        <span className="grow" />
        <button type="button" className="btn btn-ghost btn-sm" onClick={onCancel}>
          Cancel
        </button>
        <button type="submit" className="btn btn-primary btn-sm" disabled={busy || !text.trim()}>
          Send
        </button>
      </div>
    </form>
  );
}

function ProposePanel({
  event,
  route,
  onCancel,
  onSend,
}: {
  event: CalendarEvent;
  route: Card["route"];
  onCancel: () => void;
  onSend: (r: InviteAnswer, proposal: { start: number; end: number }, comment: string | null) => Promise<void>;
}) {
  const [f, setF] = useState<ProposalFields>(() => defaultProposal(event));
  const [answer, setAnswer] = useState<InviteAnswer>("tentative");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const parsed = parseProposal(f, Date.now());
  const error = "error" in parsed ? parsed.error : null;
  return (
    <form
      className="inv-compose"
      aria-label="Propose a new time"
      onSubmit={async (ev) => {
        ev.preventDefault();
        if ("error" in parsed) return;
        setBusy(true);
        await onSend(answer, parsed, note.trim() || null);
        setBusy(false);
      }}
      onKeyDown={(ev) => {
        if (ev.key === "Escape") {
          ev.stopPropagation();
          onCancel();
        }
      }}
    >
      <div className="inv-compose-row">
        <input autoFocus aria-label="Day" type="date" value={f.date} onChange={(ev) => setF({ ...f, date: ev.target.value })} />
        <input aria-label="Start" type="time" value={f.start} step={300} onChange={(ev) => setF({ ...f, start: ev.target.value })} />
        <span>to</span>
        <input aria-label="End" type="time" value={f.end} step={300} onChange={(ev) => setF({ ...f, end: ev.target.value })} />
        {!error && "start" in parsed && <span className="faint small">{proposalLabel(parsed.start, parsed.end)}</span>}
      </div>
      <input
        className="input"
        type="text"
        placeholder="Why this time? (optional)"
        aria-label="Note with the proposal"
        value={note}
        maxLength={2000}
        onChange={(ev) => setNote(ev.target.value)}
      />
      <div className="inv-compose-row">
        <span>Your answer meanwhile</span>
        <div className="setting-choice inv-rsvp" role="radiogroup" aria-label="Your answer with the proposal">
          {(["tentative", "declined"] as const).map((r) => (
            <button key={r} type="button" role="radio" aria-checked={answer === r} className={answer === r ? "selected" : ""} onClick={() => setAnswer(r)}>
              {answerWord(r)}
            </button>
          ))}
        </div>
        <span className="grow" />
        <button type="button" className="btn btn-ghost btn-sm" onClick={onCancel}>
          Cancel
        </button>
        <button type="submit" className="btn btn-primary btn-sm" disabled={busy || !!error}>
          Send proposal
        </button>
      </div>
      {error && <div className="inv-compose-err">{error}</div>}
      <div className="inv-hint">
        {route === "calendar"
          ? "Google Calendar can't send proposals from other apps, so Penguin emails it to the organizer as a standard proposal (Outlook and Google Calendar show it as one) and updates your answer in Google Calendar."
          : route === "graph"
            ? "Sent through Outlook as a proposed new time."
            : "Emailed to the organizer as a standard proposal; Outlook and Google Calendar show it as one."}
      </div>
    </form>
  );
}

function SentLine({ card }: { card: Card }) {
  const s = card.sent!;
  const how = s.via === "calendar" ? "through Google Calendar" : s.via === "graph" ? "through Outlook" : "by email";
  const proposed = s.proposedStart != null && s.proposedEnd != null;
  return (
    <div className="inv-sent">
      {proposed
        ? // Google Calendar can't carry a proposal: it went by email, the answer through the calendar.
          `You proposed ${proposalLabel(s.proposedStart!, s.proposedEnd!)} ${s.via === "graph" ? "through Outlook" : "by email"} and answered ${answerWord(s.response)}${s.via === "calendar" ? " in Google Calendar" : ""}`
        : `You answered ${answerWord(s.response)} ${how}`}
      {s.comment ? ` · Note: “${s.comment}”` : ""}
    </div>
  );
}

function answerLabel(r: CalendarEvent["myResponse"]): string {
  return r === "accepted" ? "You're going" : r === "declined" ? "You declined" : r === "tentative" ? "You said maybe" : "Not answered";
}

function accountShort(e: CalendarEvent): string {
  const a = accountById(e.accountId);
  return a ? accountName(a) : "";
}
