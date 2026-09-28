// The search reference (⌘/ in the search overlay): every operator by
// category, with examples that go into the box when clicked. The full
// reference with semantics is docs/SEARCH.md; keep the two in step.
import { Fragment, type ReactNode } from "react";
import { Icon, type IconName } from "../../components/Icon";
import { tokenize } from "./query";

export interface Tip {
  q: string;
  what: string;
}

export interface TipGroup {
  title: string;
  icon: IconName;
  tips: Tip[];
}

export const TIP_GROUPS: TipGroup[] = [
  {
    title: "Plain English",
    icon: "wand",
    tips: [
      { q: "new senders last week", what: "Read as is:new-sender date:\"last week\"" },
      { q: "emails I sent on august 27", what: "Read as in:sent date:\"august 27\"" },
      { q: "unanswered from nick this month", what: "Replies you still owe" },
      { q: "pdfs from acme.example", what: "Files from a company" },
    ],
  },
  {
    title: "People",
    icon: "users",
    tips: [
      { q: "from:mike", what: "Sender (name or address)" },
      { q: "to:ana", what: "Any recipient (to, cc, bcc)" },
      { q: "with:priya", what: "Sender or any recipient" },
      { q: "domain:acme.example", what: "Anyone at a domain" },
      { q: "from:me", what: "Mail you sent" },
      { q: "to:me", what: "Addressed to you" },
      { q: "from:new", what: "New senders: their first mail ever" },
      { q: "to:new", what: "First mail you ever sent someone" },
    ],
  },
  {
    title: "Replies",
    icon: "reply",
    tips: [
      { q: "is:unanswered", what: "Received, you haven't replied since" },
      { q: "is:awaiting", what: "You wrote last, no answer yet" },
      { q: "is:replied", what: "Received, and you replied" },
      { q: "is:reply", what: "Not the first message of its thread" },
      { q: "messages:>5", what: "Long threads (message count)" },
    ],
  },
  {
    title: "Dates",
    icon: "calendar",
    tips: [
      { q: 'date:"august 27"', what: "A day, month or phrase" },
      { q: 'date:"last week"', what: "today, yesterday, this month…" },
      { q: "date:aug1..aug15", what: "A range" },
      { q: "on:2026-08-27", what: "One day" },
      { q: 'before:"aug 27"', what: "Before (also after:, since:, until:)" },
      { q: "newer_than:3d", what: "Relative: 3d, 2w, 6m, 1y" },
      { q: "day:weekend", what: "A weekday: monday, weekday, weekend" },
    ],
  },
  {
    title: "Files and content",
    icon: "clip",
    tips: [
      { q: "has:attachment", what: "Any attached file" },
      { q: "has:pdf", what: "Also image, doc, spreadsheet, presentation" },
      { q: "has:invite", what: "A calendar invitation" },
      { q: "filename:invoice", what: "Words in a file name" },
      { q: "larger:5M", what: "Attachments over 5 MB (smaller: too)" },
      { q: "has:link", what: "A link in the text" },
      { q: "has:otp", what: "A verification code or sign-in link" },
      { q: 'subject:"q3 plan"', what: "Words in the subject" },
    ],
  },
  {
    title: "Status and place",
    icon: "inbox",
    tips: [
      { q: "is:unread", what: "Also read, starred, important" },
      { q: "is:snoozed", what: "Snoozed conversations" },
      { q: "is:newsletter", what: "Newsletters and bulk mail" },
      { q: "in:sent", what: "inbox, drafts, done, trash, spam" },
      { q: "in:anywhere", what: "Include Trash and Spam" },
      { q: "label:finance", what: "A label" },
      { q: "category:promotions", what: "A Gmail category" },
      { q: "account:work", what: "One of your accounts" },
    ],
  },
  {
    title: "Combine",
    icon: "sliders",
    tips: [
      { q: '"exact phrase"', what: "Words in this order" },
      { q: "-newsletter", what: "Leave a word (or -operator:) out" },
      { q: "from:ana OR from:bob", what: "Either one" },
      { q: "(from:ana OR from:bob) has:pdf", what: "Group with parentheses" },
      { q: "-(is:newsletter OR category:promotions)", what: "Exclude a group" },
    ],
  },
];

/** A query rendered with the same operator colors as the search box. */
export function QueryMono({ q }: { q: string }): ReactNode {
  return (
    <span className="mono q-mini sx-q">
      {tokenize(q).map((t) =>
        t.kind === "op" ? (
          <Fragment key={t.start}>
            <span className="qm-op">{t.text.slice(0, t.valueStart - t.start)}</span>
            {t.op === "date" ? <span className="qm-date">{t.text.slice(t.valueStart - t.start)}</span> : t.text.slice(t.valueStart - t.start)}
          </Fragment>
        ) : t.kind === "paren" || t.kind === "or" ? (
          <span key={t.start} className="qm-op">
            {t.text}
          </span>
        ) : (
          <Fragment key={t.start}>{t.text}</Fragment>
        ),
      )}
    </span>
  );
}

export function SearchTips({ onInsert, onClose }: { onInsert: (q: string) => void; onClose: () => void }) {
  return (
    <div className="sx-tips" role="region" aria-label="Search tips">
      <div className="sx-tips-head">
        <div>
          <div className="sx-empty-title">Search tips</div>
          <p className="sx-empty-copy">
            Everything here runs on this Mac against the local index. Click an example to add it to the box; Tab completes operator names and
            values as you type.
          </p>
        </div>
        <span className="grow" />
        <span className="gh-hint">
          <span className="kbd">⌘/</span> Toggle
        </span>
        <button className="btn btn-ghost btn-sm btn-icon" title="Close tips" aria-label="Close tips" onMouseDown={(e) => e.preventDefault()} onClick={onClose}>
          <Icon name="x" size="xs" />
        </button>
      </div>
      <div className="sx-tips-grid">
        {TIP_GROUPS.map((g) => (
          <section key={g.title} className="sx-tips-group">
            <div className="group-head">
              <span className="sx-tips-title">
                <Icon name={g.icon} size="xs" />
                {g.title}
              </span>
            </div>
            {g.tips.map((t) => (
              <a key={t.q} className="recent sx-tip-row" title={`Add ${t.q}`} onMouseDown={(e) => e.preventDefault()} onClick={() => onInsert(t.q)}>
                <QueryMono q={t.q} />
                <span className="grow" />
                <span className="faint sx-tip">{t.what}</span>
              </a>
            ))}
          </section>
        ))}
      </div>
      <p className="sx-empty-copy faint sx-tips-foot">
        Operators combine with AND by default. Words are never read as dates on their own: type date: or say it in a phrase (“emails I sent last
        week”). Full reference: docs/SEARCH.md.
      </p>
    </div>
  );
}
