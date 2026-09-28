// Right-hand context panel in the full thread view (design/02-thread.html):
// who the sender is, recent threads and files shared with them, and who's in
// the thread. All from the local search index, so it's instant and offline.
import { memo, useEffect, useMemo, useState } from "react";
import type { Address, SearchResponse, ThreadView } from "../../lib/types";
import { openThread, setUi } from "../../lib/ui";
import { api } from "../../lib/api";
import { displayName, fileExt, fileTone, firstName, monthYear, shortDate } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { Avatar } from "../../components/Identity";
import { isMe } from "../../app/store";
import { PersonMeetings } from "../calendar/PersonMeetings";

const cache = new Map<string, SearchResponse>();

function useSenderHistory(email: string | null): SearchResponse | null {
  const [res, setRes] = useState<SearchResponse | null>(email ? cache.get(email) ?? null : null);
  useEffect(() => {
    if (!email) return;
    setRes(cache.get(email) ?? null);
    let live = true;
    api.search({ query: `from:${email}`, accountId: null, limit: 50 }).then(
      (r) => {
        cache.set(email, r);
        if (live) setRes(r);
      },
      () => {
        // Search not ready (index still building): the panel shows just the person.
      },
    );
    return () => {
      live = false;
    };
  }, [email]);
  return res;
}

export const ContextPanel = memo(function ContextPanel({ thread }: { thread: ThreadView }) {
  // The person: latest sender who isn't me, else the first other recipient.
  const person: Address | null = useMemo(() => {
    for (let i = thread.messages.length - 1; i >= 0; i--) {
      const m = thread.messages[i];
      if (!isMe(m.from.email)) return m.from;
    }
    for (const m of thread.messages) for (const a of [...m.to, ...m.cc]) if (!isMe(a.email)) return a;
    return null;
  }, [thread]);

  const people = useMemo(() => {
    const seen = new Map<string, Address>();
    for (const m of thread.messages) for (const a of [m.from, ...m.to, ...m.cc]) if (!seen.has(a.email.toLowerCase())) seen.set(a.email.toLowerCase(), a);
    return [...seen.values()];
  }, [thread]);

  const history = useSenderHistory(person?.email ?? null);

  const recent = useMemo(() => {
    if (!history) return [];
    const seen = new Set<string>();
    return history.hits
      .filter((h) => {
        const k = h.accountId + h.threadId;
        if (seen.has(k) || h.threadId === thread.threadId) return false;
        seen.add(k);
        return true;
      })
      .sort((a, b) => b.date - a.date);
  }, [history, thread.threadId]);

  const files = useMemo(() => (history ? [...history.attachments].sort((a, b) => b.date - a.date) : []), [history]);
  const first = history && history.hits.length ? Math.min(...history.hits.map((h) => h.date)) : null;

  if (!person) return <aside className="context" />;
  const name = displayName(person);

  return (
    <aside className="context v-scroll">
      <div className="ctx-person">
        <Avatar person={person} size="lg" />
        <div className="grow" style={{ minWidth: 0 }}>
          <div className="ctx-name truncate">{name}</div>
          <div className="faint truncate small">{person.email.split("@")[1]}</div>
        </div>
      </div>
      <div className="ctx-facts">
        <div className="fact">
          <Icon name="mail" size="xs" />
          <span className="truncate">{person.email}</span>
        </div>
        {first !== null && (
          <div className="fact">
            <Icon name="history" size="xs" />
            <span>First emailed {monthYear(first)}</span>
            <span className="faint">· {recent.length + 1} {recent.length === 0 ? "thread" : "threads"}</span>
          </div>
        )}
      </div>
      <div className="row-flex" style={{ gap: 6, padding: "0 16px 16px" }}>
        <button
          className="btn btn-secondary btn-sm grow"
          onClick={() => setUi({ overlay: "search", searchPrefill: `from:${person.email}` })}
        >
          <Icon name="search" size="xs" />
          All mail with {firstName(person)}
        </button>
        <button
          className="btn btn-secondary btn-sm btn-icon"
          title={`Email ${firstName(person)}`}
          onClick={() => setUi({ overlay: "compose", composeContext: { mode: "new" } })}
        >
          <Icon name="compose" size="xs" />
        </button>
      </div>

      <PersonMeetings email={person.email} variant="panel" />

      {recent.length > 0 && (
        <div className="ctx-section">
          <div className="ctx-title">
            <span>Recent threads</span>
            <span className="faint tnum">{recent.length}</span>
          </div>
          {recent.slice(0, 5).map((h) => (
            <button
              key={h.accountId + h.threadId}
              className="ctx-row"
              onClick={() => openThread({ accountId: h.accountId, threadId: h.threadId }, h.messageId)}
            >
              <span className="truncate grow">{h.subject || "(no subject)"}</span>
              <span className="faint tnum">{shortDate(h.date)}</span>
            </button>
          ))}
        </div>
      )}

      {files.length > 0 && (
        <div className="ctx-section">
          <div className="ctx-title">
            <span>Shared files</span>
            <span className="faint tnum">{files.length}</span>
          </div>
          {files.slice(0, 5).map((f) => (
            <button
              key={f.attachment.id + f.messageId}
              className={"ctx-file " + fileTone(f.attachment.filename, f.attachment.mimeType)}
              onClick={() => openThread({ accountId: f.accountId, threadId: f.threadId }, f.messageId)}
            >
              <span className="mini-ico">{fileExt(f.attachment.filename)}</span>
              <span className="truncate grow">{f.attachment.filename}</span>
              <span className="faint">{shortDate(f.date)}</span>
            </button>
          ))}
        </div>
      )}

      <div className="ctx-section">
        <div className="ctx-title">
          <span>In this thread</span>
        </div>
        <div className="ctx-people">
          {people.slice(0, 6).map((p) => (
            <Avatar key={p.email} person={p} size="sm" tone={isMe(p.email) ? "gray" : undefined} photo={!isMe(p.email)} />
          ))}
          <span className="faint small" style={{ marginLeft: 6 }}>
            {people
              .slice(0, 4)
              .map((p) => (isMe(p.email) ? "you" : firstName(p)))
              .join(", ")}
            {people.length > 4 ? ` +${people.length - 4}` : ""}
          </span>
        </div>
      </div>
    </aside>
  );
});
