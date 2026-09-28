// "Message details": click a message's time. Everything about one message in
// one place — full address lists, the date in both timezones, who it was
// mailed-by / signed-by (Gmail's own SPF/DKIM/DMARC verdicts), TLS in
// transit, ids, labels, size, attachments — with Copy all, Show original
// (raw source, shown strictly as escaped text) and Open in Gmail.
import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import type { Address, MessageDetails as Details, MessageView } from "../../lib/types";
import { getUi, setUi, useUi } from "../../lib/ui";
import { api, asCommandError } from "../../lib/api";
import { registerShortcuts } from "../../lib/keyboard";
import { bytes, fileExt, fileTone } from "../../lib/format";
import { accountTone } from "../../lib/accountColor";
import { Icon } from "../../components/Icon";
import { Kbd } from "../../components/Kbd";
import { toast } from "../../components/Toast";
import { accountById, labelById, meta } from "../../app/store";
import { accountName } from "../../components/Identity";
import { openPrivacyDetails } from "../message-body/PrivacyDialog";
import { privacyFacts } from "../message-body/privacy";

let target: MessageView | null = null;
/** Open straight into "Show original" (dev deep link for screenshots). */
let startWithSource = false;
const subs = new Set<() => void>();

export function openMessageDetails(m: MessageView, showSource = false) {
  target = m;
  startWithSource = showSource;
  subs.forEach((f) => f());
  setUi({ overlay: "details" });
}

function close() {
  if (getUi().overlay === "details") setUi({ overlay: null });
}

export function MessageDetailsHost() {
  const open = useUi((s) => s.overlay === "details");
  const m = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => target,
  );
  if (!open || !m) return null;
  return <Modal key={m.accountId + m.id} m={m} />;
}

// ---------------------------------------------------------------------------

const SYSTEM_LABELS: Record<string, string> = {
  INBOX: "Inbox",
  UNREAD: "Unread",
  STARRED: "Starred",
  IMPORTANT: "Important",
  SENT: "Sent",
  DRAFT: "Draft",
  TRASH: "Trash",
  SPAM: "Spam",
  CATEGORY_PERSONAL: "Primary",
  CATEGORY_SOCIAL: "Social",
  CATEGORY_PROMOTIONS: "Promotions",
  CATEGORY_UPDATES: "Updates",
  CATEGORY_FORUMS: "Forums",
};

function addr(a: Address): string {
  return a.name ? `${a.name} <${a.email}>` : a.email;
}

function fullDate(ms: number): string {
  return new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    second: "2-digit",
    timeZoneName: "short",
  }).format(ms);
}

function gmailUrl(email: string, messageId: string): string {
  return `https://mail.google.com/mail/u/${encodeURIComponent(email)}/#all/${encodeURIComponent(messageId)}`;
}

async function copy(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast({ message: `${what} copied` });
  } catch (e) {
    toast({ tone: "error", message: `Couldn't copy: ${asCommandError(e).message}` });
  }
}

function labelName(accountId: string, id: string): string {
  const l = labelById(accountId, id);
  if (l && l.kind === "user") return l.name;
  return SYSTEM_LABELS[id] ?? l?.name ?? id;
}

type Verdict = "pass" | "fail" | "unknown";
function verdictOf(result: string | null): Verdict {
  if (!result) return "unknown";
  return result === "pass" ? "pass" : "fail";
}

function Modal({ m }: { m: MessageView }) {
  const [details, setDetails] = useState<Details | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [source, setSource] = useState<{ text?: string; error?: string; loading: boolean } | null>(null);
  meta.use((s) => s.labels);
  const account = accountById(m.accountId);
  const accounts = meta.use((s) => s.accounts);

  useEffect(() => {
    if (startWithSource) {
      startWithSource = false;
      showOriginal();
    }
  }, []);

  useEffect(() => {
    let live = true;
    api.getMessageDetails(m.accountId, m.id).then(
      (d) => live && setDetails(d),
      (e) => live && setError(asCommandError(e).message),
    );
    return () => {
      live = false;
    };
  }, [m.accountId, m.id]);

  const sourceOpen = source !== null;
  const showOriginal = () => {
    setSource({ loading: true });
    api.getMessageSource(m.accountId, m.id).then(
      (text) => setSource({ text, loading: false }),
      (e) => setSource({ error: asCommandError(e).message, loading: false }),
    );
  };

  useEffect(
    () =>
      registerShortcuts([
        {
          id: "details.close",
          keys: "escape",
          label: "Close message details",
          group: "Message details",
          hidden: true,
          when: () => getUi().overlay === "details",
          // Esc steps back from the source view first.
          run: () => (sourceOpen ? setSource(null) : close()),
        },
      ]),
    [sourceOpen],
  );

  const d = details;
  const allText = d ? detailsText(d, m, account ? `${accountName(account, accounts)} <${account.email}>` : m.accountId) : "";

  return (
    <>
      <div className="scrim" onMouseDown={close} />
      <div className="overlay-host center" onMouseDown={(e) => e.target === e.currentTarget && close()}>
        <section className="panel md-panel" role="dialog" aria-label="Message details">
          <header className="md-head">
            {sourceOpen ? (
              <button className="btn btn-ghost btn-sm" onClick={() => setSource(null)}>
                <Icon name="left" size="xs" />
                Details
              </button>
            ) : null}
            <div className="grow" style={{ minWidth: 0 }}>
              <div className="md-title">{sourceOpen ? "Original message" : "Message details"}</div>
              <div className="faint small md-subtitle">{m.subject || "(no subject)"}</div>
            </div>
            {sourceOpen ? (
              <button className="btn btn-ghost btn-sm" disabled={!source?.text} onClick={() => source?.text && void copy(source.text, "Original")}>
                <Icon name="draft" size="xs" />
                Copy
              </button>
            ) : (
              <>
                <button className="btn btn-ghost btn-sm" disabled={!d} onClick={() => void copy(allText, "Details")}>
                  <Icon name="draft" size="xs" />
                  Copy all
                </button>
                <button className="btn btn-ghost btn-sm" onClick={showOriginal}>
                  <Icon name="file" size="xs" />
                  Show original
                </button>
                {account?.provider === "gmail" && (
                  <button className="btn btn-ghost btn-sm" onClick={() => void api.openExternal(gmailUrl(account.email, m.id))}>
                    <Icon name="external" size="xs" />
                    Open in Gmail
                  </button>
                )}
              </>
            )}
            <button className="btn btn-ghost btn-sm btn-icon" aria-label="Close" title="Close" onClick={close}>
              <Icon name="x" size="xs" />
            </button>
          </header>

          {sourceOpen ? (
            <div className="md-source v-scroll">
              {source?.loading ? (
                <div className="md-loading faint small">
                  <span className="spinner" aria-hidden="true" />
                  Fetching the original from Gmail…
                </div>
              ) : source?.error ? (
                <div className="md-error">Couldn't load the original: {source.error}</div>
              ) : (
                // React escapes this: the source is shown as text, never parsed as HTML.
                <pre>{source?.text}</pre>
              )}
            </div>
          ) : (
            <div className="md-body v-scroll">
              {error ? (
                <div className="md-error">Couldn't load the details: {error}</div>
              ) : !d ? (
                <div className="md-loading faint small">
                  <span className="spinner" aria-hidden="true" />
                  Loading details…
                </div>
              ) : (
                <DetailsBody d={d} m={m} accountLabel={account ? accountName(account, accounts) : null} />
              )}
            </div>
          )}
          <footer className="md-foot faint small">
            <span className="hint">
              <Kbd>Esc</Kbd>
              {sourceOpen ? "Back" : "Close"}
            </span>
          </footer>
        </section>
      </div>
    </>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </>
  );
}

function Addrs({ list }: { list: Address[] }) {
  return (
    <div className="md-addrs">
      {list.map((a) => (
        <span key={a.email} className="md-addr">
          {a.name ? (
            <>
              <span className="emph">{a.name}</span> <span className="faint">&lt;{a.email}&gt;</span>
            </>
          ) : (
            <span className="emph">{a.email}</span>
          )}
        </span>
      ))}
    </div>
  );
}

function Copyable({ value, what }: { value: string; what: string }) {
  return (
    <span className="md-copyable">
      <code>{value}</code>
      <button className="btn btn-ghost btn-sm btn-icon" title={`Copy ${what}`} aria-label={`Copy ${what}`} onClick={() => void copy(value, what)}>
        <Icon name="draft" size="xs" />
      </button>
    </span>
  );
}

function DetailsBody({ d, m, accountLabel }: { d: Details; m: MessageView; accountLabel: string | null }) {
  const account = accountById(d.accountId);
  const spf = verdictOf(d.auth.spf);
  const dkim = verdictOf(d.auth.dkim);
  const dmarc = verdictOf(d.auth.dmarc);
  const authKnown = d.headersFetched && (d.auth.spf || d.auth.dkim || d.auth.dmarc);
  return (
    <>
      <dl className="md-grid">
        <Row label="From">
          <Addrs list={[d.from]} />
        </Row>
        {d.replyTo.length > 0 && (
          <Row label="Reply-To">
            <Addrs list={d.replyTo} />
          </Row>
        )}
        {d.to.length > 0 && (
          <Row label="To">
            <Addrs list={d.to} />
          </Row>
        )}
        {d.cc.length > 0 && (
          <Row label="Cc">
            <Addrs list={d.cc} />
          </Row>
        )}
        {d.bcc.length > 0 && (
          <Row label="Bcc">
            <Addrs list={d.bcc} />
          </Row>
        )}
        <Row label="Date">
          <div>{fullDate(d.date)}</div>
          {d.dateHeader && <div className="faint small">Sent as: {d.dateHeader}</div>}
        </Row>
        <Row label="Subject">{d.subject || "(no subject)"}</Row>
        {account && (
          <Row label="Account">
            <span className="md-acct">
              <i className={`dot dot-sm t-${accountTone(account.color)}`} />
              {accountLabel} <span className="faint">{account.email}</span>
            </span>
          </Row>
        )}
      </dl>

      <div className="md-section">Authenticity</div>
      <dl className="md-grid">
        <Row label="Verdict">
          {!d.headersFetched ? (
            <span className="md-verdict unknown">
              <Icon name="shield" size="xs" />
              Unknown — couldn't reach Gmail{d.headersError ? ` (${d.headersError})` : ""}
            </span>
          ) : d.senderAuthenticated ? (
            <span className="md-verdict pass">
              <Icon name="shield" size="xs" />
              Sender verified by Gmail
            </span>
          ) : authKnown ? (
            <span className="md-verdict fail">
              <Icon name="shield" size="xs" />
              Sender not verified — the From address may not be who it claims
            </span>
          ) : (
            <span className="md-verdict unknown">
              <Icon name="shield" size="xs" />
              No verdict (Gmail didn't check this message, e.g. mail you sent)
            </span>
          )}
        </Row>
        {d.headersFetched && (
          <>
            <Row label="Mailed-by">
              {d.auth.mailedBy ?? <span className="faint">—</span>}
              {d.auth.spf && <span className={`md-pill ${spf}`}>SPF {d.auth.spf}</span>}
            </Row>
            <Row label="Signed-by">
              {d.auth.signedBy.length ? d.auth.signedBy.join(", ") : <span className="faint">—</span>}
              {d.auth.dkim && <span className={`md-pill ${dkim}`}>DKIM {d.auth.dkim}</span>}
            </Row>
            <Row label="DMARC">{d.auth.dmarc ? <span className={`md-pill ${dmarc}`}>{d.auth.dmarc}</span> : <span className="faint">—</span>}</Row>
            <Row label="Security">
              {d.transport.tls === true ? (
                <span>
                  <Icon name="lock" size="xs" className="md-inline-ico" />
                  Standard encryption (TLS){d.transport.detail ? <span className="faint"> · {d.transport.detail}</span> : null}
                </span>
              ) : d.transport.tls === false ? (
                <span className="md-warn">Not encrypted in transit to Google</span>
              ) : (
                <span className="faint">Unknown</span>
              )}
            </Row>
          </>
        )}
        <Row label="Privacy">
          <button className="md-link" onClick={() => openPrivacyDetails(m)} title="What was removed and why">
            {privacyFacts(m)}
            <Icon name="info" size="xs" />
          </button>
        </Row>
        {d.unsubscribe.length > 0 && (
          <Row label="Unsubscribe">
            <div className="md-links">
              {d.unsubscribe.map((u) => (
                <button key={u} className="md-link" onClick={() => void api.openExternal(u)}>
                  {u.startsWith("mailto:") ? `Email ${u.slice(7).split("?")[0]}` : "Unsubscribe on the sender's site"}
                  <Icon name="external" size="xs" />
                </button>
              ))}
            </div>
          </Row>
        )}
      </dl>

      <div className="md-section">Message</div>
      <dl className="md-grid">
        <Row label="Labels">
          <div className="md-labels">
            {d.labelIds.map((id) => (
              <span key={id} className="md-label">
                {labelName(d.accountId, id)}
              </span>
            ))}
          </div>
        </Row>
        <Row label="Size">{d.size != null ? bytes(d.size) : <span className="faint">Unknown</span>}</Row>
        {d.attachments.length > 0 && (
          <Row label="Attachments">
            <div className="md-atts">
              {d.attachments.map((a) => (
                <span key={a.id} className="md-att">
                  <span className={"mini-ico " + fileTone(a.filename, a.mimeType)}>{fileExt(a.filename)}</span>
                  {a.filename} <span className="faint">{bytes(a.size)}</span>
                </span>
              ))}
            </div>
          </Row>
        )}
        {d.messageIdHeader && (
          <Row label="Message-ID">
            <Copyable value={d.messageIdHeader} what="Message-ID" />
          </Row>
        )}
        {(d.inReplyTo || d.references.length > 0) && (
          <Row label="In reply to">
            <details className="md-refs">
              <summary>{d.inReplyTo ?? `${d.references.length} references`}</summary>
              {d.references.map((r) => (
                <code key={r}>{r}</code>
              ))}
            </details>
          </Row>
        )}
        <Row label="Gmail ids">
          <div className="md-ids">
            <span className="faint small">Message</span>
            <Copyable value={d.messageId} what="Gmail message id" />
            <span className="faint small">Thread</span>
            <Copyable value={d.threadId} what="Gmail thread id" />
          </div>
        </Row>
      </dl>
    </>
  );
}

/** Plain-text version of everything shown, for "Copy all". */
function detailsText(d: Details, m: MessageView, account: string): string {
  const lines: [string, string][] = [
    ["From", addr(d.from)],
    ...(d.replyTo.length ? [["Reply-To", d.replyTo.map(addr).join(", ")] as [string, string]] : []),
    ["To", d.to.map(addr).join(", ")],
    ...(d.cc.length ? [["Cc", d.cc.map(addr).join(", ")] as [string, string]] : []),
    ...(d.bcc.length ? [["Bcc", d.bcc.map(addr).join(", ")] as [string, string]] : []),
    ["Date", fullDate(d.date) + (d.dateHeader ? ` (sent as ${d.dateHeader})` : "")],
    ["Subject", d.subject],
    ["Account", account],
    ["Sender verified", d.headersFetched ? (d.senderAuthenticated ? "yes" : "no") : "unknown"],
    ["Mailed-by", `${d.auth.mailedBy ?? "—"}${d.auth.spf ? ` (SPF ${d.auth.spf})` : ""}`],
    ["Signed-by", `${d.auth.signedBy.join(", ") || "—"}${d.auth.dkim ? ` (DKIM ${d.auth.dkim})` : ""}`],
    ["DMARC", d.auth.dmarc ?? "—"],
    ["Security", d.transport.tls == null ? "unknown" : d.transport.tls ? `TLS${d.transport.detail ? ` (${d.transport.detail})` : ""}` : "not encrypted"],
    ["Labels", d.labelIds.map((id) => labelName(d.accountId, id)).join(", ")],
    ["Size", d.size != null ? bytes(d.size) : "unknown"],
    ["Attachments", d.attachments.map((a) => `${a.filename} (${bytes(a.size)})`).join(", ") || "none"],
    ["Message-ID", d.messageIdHeader ?? "—"],
    ["In-Reply-To", d.inReplyTo ?? "—"],
    ["References", d.references.join(" ") || "—"],
    ["Gmail message id", d.messageId],
    ["Gmail thread id", d.threadId],
    ["Privacy", privacyFacts(m)],
    ...(d.unsubscribe.length ? [["Unsubscribe", d.unsubscribe.join(", ")] as [string, string]] : []),
  ];
  return lines.map(([k, v]) => `${k}: ${v}`).join("\n");
}
