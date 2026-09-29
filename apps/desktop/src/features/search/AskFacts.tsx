// OWNER: ask agent. The Ask card's rich answers: one card per extracted
// fact (flight, stay, order, parcel, bill, booking, contact details), quoted
// passages, the math behind a sum, and a person card. Every card opens the
// email it came from; keyboard selection moves over the answer's cited
// items (index.tsx), and the card for the selected item is highlighted.
import type { ReactNode } from "react";
import type { AskCard as Card, AskCite, AskFact, AskPassage, AskPerson, AskSum, Extracted } from "../../lib/types";
import { displayName, shortDate } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { Avatar } from "../../components/Identity";
import { toast } from "../../components/Toast";
import { api } from "../../lib/api";
import { daysUntil, emailRows, fmtDay, fmtTime, fmtWhen, markSegments, money, SHIP_STEPS, shipStep, statusTone } from "./askFormat";

type Open = (cite: AskCite, keep: boolean) => void;

const sameCite = (a: AskCite | null | undefined, b: AskCite) => !!a && a.accountId === b.accountId && a.messageId === b.messageId;
const keep = (e: { metaKey: boolean; ctrlKey: boolean }) => e.metaKey || e.ctrlKey;

async function copy(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast({ kind: "success", message: `Copied ${what}`, key: "ask-copied" });
  } catch (e) {
    toast({ kind: "error", message: `Couldn't copy the ${what}`, detail: String((e as Error)?.message ?? e) });
  }
}

/** A value with a copy button (codes, tracking numbers, phone numbers). */
function Copyable({ value, what, mono = true }: { value: string; what: string; mono?: boolean }) {
  return (
    <span className="ask-copy">
      <span className={mono ? "mono" : undefined}>{value}</span>
      <button
        className="ask-copy-btn"
        title={`Copy ${what}`}
        aria-label={`Copy ${what}`}
        onMouseDown={(e) => e.preventDefault()}
        onClick={(e) => {
          e.stopPropagation();
          void copy(value, what);
        }}
      >
        <Icon name="copy" size="2xs" />
      </button>
    </span>
  );
}

function Status({ text }: { text: string | null }) {
  if (!text) return null;
  return <span className={`ask-status t-${statusTone(text)}`}>{text}</span>;
}

/** The card frame: title row, body, and where it came from. */
function Frame({ card, active, onOpen, icon, title, sub, children }: { card: Card; active: boolean; onOpen: Open; icon: Parameters<typeof Icon>[0]["name"]; title: ReactNode; sub?: ReactNode; children?: ReactNode }) {
  const source = card.source === "pattern" ? "Read from the email's text" : "From the email's structured data (schema.org)";
  return (
    <article
      className={`ask-fcard ask-fcard-${card.fact.kind}${active ? " is-active" : ""}`}
      onMouseDown={(e) => e.preventDefault()}
      onClick={(e) => onOpen(card.cite, keep(e))}
      title="Open the email"
    >
      <header className="ask-fcard-head">
        <Icon name={icon} size="xs" />
        <span className="ask-fcard-title truncate">{title}</span>
        {sub && <span className="ask-fcard-sub truncate">{sub}</span>}
        <span className="grow" />
        <Status text={card.status} />
      </header>
      {children && <div className="ask-fcard-body">{children}</div>}
      <footer className="ask-fcard-foot faint">
        <span className="truncate">
          {displayName(card.from)} · {shortDate(card.date)}
          {card.related.length > 0 && ` · ${card.related.length + 1} emails`}
        </span>
        <span className="grow" />
        <span title={source}>{card.source === "pattern" ? "text" : "schema.org"}</span>
      </footer>
    </article>
  );
}

function Field({ label, children, wide = false }: { label: string; children: ReactNode; wide?: boolean }) {
  if (children === null || children === undefined || children === "") return null;
  return (
    <div className={`ask-field${wide ? " is-wide" : ""}`}>
      <span className="ask-field-label">{label}</span>
      <span className="ask-field-value">{children}</span>
    </div>
  );
}

function FactBody({ card, onOpen, active, now }: { card: Card; onOpen: Open; active: boolean; now: number }) {
  const f: Extracted = card.fact;
  switch (f.kind) {
    case "flight":
      return (
        <Frame card={card} active={active} onOpen={onOpen} icon="send" title={f.flightNumber ?? "Flight"} sub={f.airline}>
          <div className="ask-route">
            <div className="ask-end">
              <span className="ask-iata">{f.departAirport ?? "—"}</span>
              <span className="ask-city truncate">{f.departName}</span>
              <span className="ask-time">{fmtTime(f.departTime)}</span>
            </div>
            <div className="ask-route-line" aria-hidden>
              <span />
              <Icon name="right" size="xs" />
            </div>
            <div className="ask-end ask-end-arr">
              <span className="ask-iata">{f.arriveAirport ?? "—"}</span>
              <span className="ask-city truncate">{f.arriveName}</span>
              <span className="ask-time">{fmtTime(f.arriveTime)}</span>
            </div>
          </div>
          <div className="ask-fields">
            <Field label="Departs">{fmtDay(f.departTime, now)}</Field>
            <Field label="Confirmation">{f.confirmation && <Copyable value={f.confirmation} what="confirmation code" />}</Field>
            <Field label="Passenger">{f.passenger}</Field>
            <Field label="Total">{f.total && money(f.total.value, f.total.currency)}</Field>
          </div>
        </Frame>
      );
    case "lodging":
      return (
        <Frame card={card} active={active} onOpen={onOpen} icon="home" title={f.name ?? "Stay"}>
          <div className="ask-fields">
            <Field label="Check in">{fmtWhen(f.checkin, now)}</Field>
            <Field label="Check out">{fmtWhen(f.checkout, now)}</Field>
            <Field label="Address" wide>
              {f.address}
            </Field>
            <Field label="Confirmation">{f.confirmation && <Copyable value={f.confirmation} what="confirmation number" />}</Field>
            <Field label="Phone">{f.phone && <Copyable value={f.phone} what="phone number" mono={false} />}</Field>
            <Field label="Total">{f.total && money(f.total.value, f.total.currency)}</Field>
          </div>
        </Frame>
      );
    case "order":
      return (
        <Frame card={card} active={active} onOpen={onOpen} icon="briefcase" title={f.merchant ?? "Order"} sub={f.orderNumber ? `#${f.orderNumber}` : null}>
          {f.items.length > 0 && <div className="ask-lines truncate">{f.items.join(" · ")}</div>}
          <div className="ask-fields">
            <Field label="Total">{f.total && <span className={f.total.value < 0 ? "ask-amt neg" : "ask-amt"}>{money(f.total.value, f.total.currency)}</span>}</Field>
            <Field label="From">{f.totalSource}</Field>
          </div>
        </Frame>
      );
    case "shipment": {
      const step = shipStep(f.status);
      return (
        <Frame card={card} active={active} onOpen={onOpen} icon="download" title={f.merchant ?? f.carrier ?? "Package"} sub={f.merchant ? f.carrier : null}>
          <ol className={`ask-steps-bar${step < 0 ? " is-problem" : ""}`} aria-label="Delivery progress">
            {SHIP_STEPS.map((s, i) => (
              <li key={s} className={i <= step ? "done" : undefined}>
                {s}
              </li>
            ))}
          </ol>
          <div className="ask-fields">
            <Field label="Expected">{f.expected && fmtDay(f.expected, now)}</Field>
            <Field label="Tracking" wide>
              {f.trackingNumber && <Copyable value={f.trackingNumber} what="tracking number" />}
            </Field>
            <Field label="Order">{f.orderNumber}</Field>
          </div>
          {f.trackingUrl && (
            <button
              className="btn btn-ghost btn-sm ask-track"
              onMouseDown={(e) => e.preventDefault()}
              onClick={(e) => {
                e.stopPropagation();
                void api.openExternal(f.trackingUrl!);
              }}
              title={f.trackingUrl}
            >
              <Icon name="external" size="xs" />
              Track on {f.carrier ?? "the carrier's site"}
            </button>
          )}
        </Frame>
      );
    }
    case "bill": {
      const days = daysUntil(f.dueDate, now);
      return (
        <Frame card={card} active={active} onOpen={onOpen} icon="file" title={f.biller ?? "Bill"} sub={f.invoiceNumber ? `#${f.invoiceNumber}` : null}>
          <div className="ask-bill">
            <span className="ask-bill-amt">{f.amountDue ? money(f.amountDue.value, f.amountDue.currency) : "—"}</span>
            <span className="ask-bill-due">
              {f.dueDate ? `${days !== null && days < 0 ? "was due" : "due"} ${fmtDay(f.dueDate, now)}` : "no due date in the email"}
            </span>
          </div>
          {f.amountSource && <div className="ask-lines faint truncate">{f.amountSource}</div>}
        </Frame>
      );
    }
    case "reservation":
      return (
        <Frame card={card} active={active} onOpen={onOpen} icon={f.category === "restaurant" ? "users" : "calendar"} title={f.name ?? "Reservation"} sub={f.category === "restaurant" ? "Table" : f.category === "event" ? "Tickets" : null}>
          <div className="ask-fields">
            <Field label="When">{fmtWhen(f.start, now)}</Field>
            <Field label="Party">{f.partySize ? `${f.partySize} ${f.partySize === 1 ? "person" : "people"}` : null}</Field>
            <Field label="Where">{[f.venue !== f.name ? f.venue : null, f.address].filter(Boolean).join(" · ")}</Field>
            <Field label="Confirmation">{f.confirmation && <Copyable value={f.confirmation} what="confirmation" />}</Field>
            <Field label="Total">{f.total && money(f.total.value, f.total.currency)}</Field>
          </div>
        </Frame>
      );
    case "contact":
      return (
        <Frame card={card} active={active} onOpen={onOpen} icon="user" title={displayName(card.from)} sub="From their signature">
          <div className="ask-fields">
            {f.phones.map((p) => (
              <Field key={p} label="Phone">
                <Copyable value={p} what="phone number" mono={false} />
              </Field>
            ))}
            {f.addresses.map((a) => (
              <Field key={a} label="Address">
                <Copyable value={a} what="address" mono={false} />
              </Field>
            ))}
          </div>
        </Frame>
      );
  }
}

export function FactCards({ cards, activeCite, onOpen, now = Date.now() }: { cards: Card[]; activeCite: AskCite | null; onOpen: Open; now?: number }) {
  if (cards.length === 0) return null;
  return (
    <div className={`ask-fcards${cards.length === 1 ? " is-one" : ""}`}>
      {cards.map((c, i) => (
        <FactBody key={`${c.cite.accountId}/${c.cite.messageId}/${i}`} card={c} onOpen={onOpen} active={sameCite(activeCite, c.cite)} now={now} />
      ))}
    </div>
  );
}

/** Quoted sentences, the question's words highlighted, with who wrote them. */
export function Passages({ passages, activeCite, onOpen }: { passages: AskPassage[]; activeCite: AskCite | null; onOpen: Open }) {
  if (passages.length === 0) return null;
  return (
    <div className="ask-passages">
      {passages.map((p, i) => (
        <figure
          key={`${p.cite.messageId}/${i}`}
          className={`ask-passage${sameCite(activeCite, p.cite) ? " is-active" : ""}`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={(e) => onOpen(p.cite, keep(e))}
          title="Open the email"
        >
          <blockquote>
            {markSegments(p.text, p.marks).map((s, j) => (s.mark ? <mark key={j}>{s.text}</mark> : <span key={j}>{s.text}</span>))}
          </blockquote>
          <figcaption className="faint">
            <span className="ask-passage-who">{p.sent ? "You" : displayName(p.from)}</span> · {shortDate(p.date)} · <span className="truncate">{p.subject}</span>
          </figcaption>
        </figure>
      ))}
    </div>
  );
}

/** "$412.18 across 9 receipts": the total per currency and what was left out. */
export function SumBlock({ sum, expanded, onToggle, count }: { sum: AskSum; expanded: boolean; onToggle: () => void; count: number }) {
  const left: string[] = [];
  if (sum.duplicates) left.push(`${sum.duplicates} repeating an order already counted`);
  if (sum.skipped) left.push(`${sum.skipped} with no amount`);
  if (sum.unpaid) left.push(`${sum.unpaid} ${sum.unpaid === 1 ? "invoice" : "invoices"} still due`);
  return (
    <div className="ask-sum">
      <div className="ask-sum-totals">
        {sum.totals.map((t, i) => (
          <span key={t.currency} className="ask-sum-total">
            {i > 0 && <span className="faint"> + </span>}
            <b>{money(t.value, t.currency)}</b>
            <span className="faint">
              {" "}
              · {t.count} {t.count === 1 ? "email" : "emails"}
            </span>
          </span>
        ))}
      </div>
      <div className="ask-sum-basis faint">
        {sum.basis}
        {left.length > 0 && ` · left out: ${left.join(", ")}`}
      </div>
      {count > 0 && (
        <button className="btn btn-ghost btn-sm" onMouseDown={(e) => e.preventDefault()} onClick={onToggle} aria-expanded={expanded}>
          <Icon name={expanded ? "up" : "down"} size="xs" />
          {expanded ? "Hide the list" : `Show all ${count} added`}
        </button>
      )}
    </div>
  );
}

/** Who the answer is about: name, addresses, and the phone/address facts. */
/**
 * A person card: name, addresses, phone and postal address. With `emails`
 * (a contact-details answer), each address you've exchanged mail with is a
 * row with its counts and a copy button, and the phone copies too.
 */
export function PeopleCard({ person, facts, onOpen, emails = false }: { person: AskPerson; facts: AskFact[]; onOpen: Open; emails?: boolean }) {
  const detail = facts.filter((f) => f.label === "Phone" || f.label === "Address");
  // "priya@linden.example · 41 from them · 27 from you · last Sep 22, 2026"
  const rows = emails ? emailRows(facts) : [];
  return (
    <div className={`ask-person${rows.length ? " has-emails" : ""}`}>
      <Avatar person={{ name: person.name, email: person.emails[0] ?? "" }} size="lg" />
      <div className="ask-person-main">
        <div className="ask-person-name">{person.name ?? person.label}</div>
        {rows.length === 0 && <div className="ask-person-sub faint truncate">{person.company ? person.domain : person.emails.join(", ")}</div>}
        {rows.length > 0 && (
          <ul className="ask-person-emails" aria-label="Email addresses">
            {rows.map(({ address, detail: counts, fact: f }) => (
              <li key={address}>
                <Copyable value={address} what="address" mono={false} />
                {counts && (
                  <span className="faint truncate">
                    {f.cite ? (
                      <a onMouseDown={(e) => e.preventDefault()} onClick={(e) => onOpen(f.cite!, keep(e))} title="Open the latest email from this address">
                        {counts}
                      </a>
                    ) : (
                      counts
                    )}
                  </span>
                )}
              </li>
            ))}
          </ul>
        )}
        {detail.length > 0 && (
          <div className="ask-person-facts">
            {detail.map((f) => {
              const [value, ...rest] = f.value.split(" · ");
              return (
                <span key={f.label} className="ask-person-fact">
                  <span className="faint">{f.label}</span>{" "}
                  {f.label === "Phone" && emails ? (
                    <>
                      <Copyable value={value} what="phone number" mono={false} />
                      {rest.length > 0 && (
                        <a className="faint" onMouseDown={(e) => e.preventDefault()} onClick={(e) => f.cite && onOpen(f.cite, keep(e))}>
                          {rest.join(" · ")}
                        </a>
                      )}
                    </>
                  ) : f.cite ? (
                    <a onMouseDown={(e) => e.preventDefault()} onClick={(e) => onOpen(f.cite!, keep(e))}>
                      {f.value}
                    </a>
                  ) : (
                    f.value
                  )}
                </span>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
