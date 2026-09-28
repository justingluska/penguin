// The rule editor: a dialog over Settings → Rules. OWNER: rules agent.
//
// Condition = the search box itself (same highlighting and autocomplete),
// with a live "matches N existing messages" count and a preview of the
// newest matches. Risky actions (Trash, Forward) ask once when added.
// After saving a new-mail rule, it offers to handle existing matches: a
// dry run first, then the real thing.
import { useEffect, useMemo, useRef, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import type { Rule, RuleAction, RuleInput, RulePreview, RuleRunReport, RuleTrigger } from "../../lib/types";
import { meta } from "../../app/store";
import { useProfiles } from "../../app/profiles";
import { Icon } from "../../components/Icon";
import { Select, type SelectOption } from "../../components/Select";
import { toast } from "../../components/Toast";
import { num, displayName, shortDate } from "../../lib/format";
import { QueryInput } from "../search/QueryInput";
import "../search/search.css";
import { ConfirmDialog, Switch } from "../settings/parts";
import { ACTION_KINDS, WEEKDAYS, actionText, defaultAction } from "./describe";
import { closeRuleEditor } from "./state";

const PREVIEW_DEBOUNCE_MS = 200;
const PREVIEW_ROWS = 6;

type Risky = Extract<RuleAction, { kind: "trash" | "forward" }>;
type Confirming = { index: number; action: Risky } | null;

export function RuleEditor({ initial, allowHooks }: { initial: RuleInput; allowHooks: boolean }) {
  const [draft, setDraft] = useState<RuleInput>(initial);
  const [preview, setPreview] = useState<RulePreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<Confirming>(null);
  const [saved, setSaved] = useState<Rule | null>(null);
  const conditionRef = useRef<HTMLInputElement>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  const accounts = meta.use((m) => m.accounts);
  const allLabels = meta.use((m) => m.labels);
  const profiles = useProfiles();
  const isNew = initial.id === null;

  const set = (patch: Partial<RuleInput>) => setDraft((d) => ({ ...d, ...patch }));

  useEffect(() => {
    (initial.condition ? nameRef : conditionRef).current?.focus();
    // Esc closes the editor, not Settings underneath (.st-confirm handling
    // is skipped while a nested confirm is up).
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || document.querySelector(".st-scrim .st-confirm")) return;
      if (document.querySelector(".qi-ac")) return; // autocomplete takes Esc first
      e.preventDefault();
      e.stopPropagation();
      closeRuleEditor();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [initial.condition]);

  // Live match count + newest matches, debounced.
  useEffect(() => {
    const condition = draft.condition.trim();
    if (!condition) {
      setPreview(null);
      setPreviewError(null);
      return;
    }
    let live = true;
    const t = setTimeout(() => {
      api
        .previewRule({ condition, accountIds: draft.accountIds, profileId: draft.profileId, ruleId: draft.id, limit: PREVIEW_ROWS })
        .then((p) => live && (setPreview(p), setPreviewError(null)))
        .catch((e) => live && setPreviewError(asCommandError(e).message));
    }, PREVIEW_DEBOUNCE_MS);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [draft.condition, draft.accountIds, draft.profileId, draft.id]);

  // Labels the scope's accounts have (by name; rules resolve names per account).
  const scopeIds = useMemo(() => {
    if (draft.profileId) return profiles.find((p) => p.id === draft.profileId)?.accountIds ?? [];
    return draft.accountIds ?? accounts.map((a) => a.id);
  }, [draft.profileId, draft.accountIds, profiles, accounts]);
  const labels = useMemo(() => allLabels.filter((l) => scopeIds.includes(l.accountId)), [allLabels, scopeIds]);
  const labelOptions = useMemo(() => {
    const seen = new Set<string>();
    const out: SelectOption<string>[] = [];
    for (const l of labels) {
      if (l.kind !== "user" || seen.has(l.name.toLowerCase())) continue;
      seen.add(l.name.toLowerCase());
      out.push({ value: l.name, label: l.name });
    }
    return out.sort((a, b) => a.label.localeCompare(b.label));
  }, [labels]);
  const triggerLabelOptions: SelectOption<string>[] = [
    { value: "STARRED", label: "Starred" },
    { value: "IMPORTANT", label: "Important" },
    ...labelOptions,
  ];

  const scopeValue = draft.profileId ? `p:${draft.profileId}` : draft.accountIds?.length === 1 ? `a:${draft.accountIds[0]}` : "all";
  const scopeOptions: SelectOption<string>[] = [
    { value: "all", label: "All accounts" },
    ...profiles.map((p) => ({ value: `p:${p.id}`, label: `${p.emoji ? p.emoji + " " : ""}${p.name}`, hint: "Profile" })),
    ...accounts.map((a) => ({ value: `a:${a.id}`, label: a.email })),
  ];
  const setScope = (v: string) =>
    v === "all"
      ? set({ accountIds: null, profileId: null })
      : v.startsWith("p:")
        ? set({ profileId: v.slice(2), accountIds: null })
        : set({ accountIds: [v.slice(2)], profileId: null });

  const triggerKind = draft.trigger.kind;
  const setTrigger = (t: RuleTrigger) => set({ trigger: t });

  const setAction = (i: number, a: RuleAction) => set({ actions: draft.actions.map((x, j) => (j === i ? a : x)) });
  const removeAction = (i: number) => set({ actions: draft.actions.filter((_, j) => j !== i) });
  const addAction = (kind: RuleAction["kind"]) => {
    const a = defaultAction(kind, labels);
    // Asked once: Trash when it's added, Forward on save (once the address is typed).
    if (a.kind === "trash") setConfirming({ index: draft.actions.length, action: a });
    set({ actions: [...draft.actions, a] });
  };

  const hasHooks = draft.actions.some((a) => a.kind === "hook" || a.kind === "webhook");
  const unconfirmedForward = draft.actions.findIndex((a) => a.kind === "forward" && !a.confirmed);

  async function save() {
    const pending = draft.actions[unconfirmedForward];
    if (pending?.kind === "forward") {
      setConfirming({ index: unconfirmedForward, action: pending });
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const rule = await api.saveRule(draft);
      if (isNew && rule.trigger.kind === "newMessage" && (preview?.count ?? 0) > 0) {
        setSaved(rule);
      } else {
        toast({ message: isNew ? `Rule “${rule.name}” created${rule.dryRun ? " in test mode" : ""}` : "Rule saved" });
        closeRuleEditor();
      }
    } catch (e) {
      setError(asCommandError(e).message);
    } finally {
      setSaving(false);
    }
  }

  if (saved) return <ApplyExisting rule={saved} count={preview?.count ?? 0} />;

  return (
    <div className="st-scrim rl-scrim" onMouseDown={(e) => e.target === e.currentTarget && closeRuleEditor()}>
      <div className="rl-editor" role="dialog" aria-modal="true" aria-label={isNew ? "New rule" : "Edit rule"}>
        <header className="rl-ed-head">
          <h3>{isNew ? "New rule" : "Edit rule"}</h3>
          <button className="btn btn-ghost btn-sm btn-icon" aria-label="Close" onClick={closeRuleEditor}>
            <Icon name="x" size="sm" />
          </button>
        </header>

        <div className="rl-ed-body">
          <label className="rl-field">
            <span className="rl-label">Name</span>
            <input
              ref={nameRef}
              className="input rl-input"
              value={draft.name}
              maxLength={80}
              placeholder="e.g. File Uber receipts"
              onChange={(e) => set({ name: e.target.value })}
            />
          </label>

          <div className="rl-grid">
            <div className="rl-field">
              <span className="rl-label">When</span>
              <Select
                label="Trigger"
                value={triggerKind}
                onChange={(k) =>
                  setTrigger(
                    k === "labelAdded"
                      ? { kind: k, label: triggerLabelOptions[0]?.value ?? "STARRED" }
                      : k === "schedule"
                        ? { kind: k, every: "daily", at: "08:00", weekday: null }
                        : { kind: k },
                  )
                }
                options={[
                  { value: "newMessage", label: "New mail arrives", icon: "inbox" },
                  { value: "labelAdded", label: "A label is added", icon: "tag" },
                  { value: "schedule", label: "On a schedule", icon: "clock" },
                  { value: "manual", label: "Only when I run it", icon: "zap" },
                ]}
              />
            </div>
            <div className="rl-field">
              <span className="rl-label">In</span>
              <Select label="Accounts" value={scopeValue} onChange={setScope} options={scopeOptions} />
            </div>
          </div>

          {draft.trigger.kind === "labelAdded" && (
            <div className="rl-sub-row">
              <span className="st-muted">Label</span>
              <Select
                label="Trigger label"
                value={draft.trigger.label}
                options={triggerLabelOptions}
                onChange={(label) => setTrigger({ kind: "labelAdded", label })}
              />
            </div>
          )}
          {draft.trigger.kind === "schedule" && <ScheduleFields trigger={draft.trigger} onChange={setTrigger} />}
          {draft.trigger.kind === "newMessage" && (
            <p className="st-muted rl-hint">Runs on mail that arrives while Penguin syncs, never on older mail it downloads.</p>
          )}

          <div className="rl-field">
            <span className="rl-label">Matching</span>
            <div className="rl-query">
              <Icon name="search" size="sm" className="rl-query-ico" />
              <QueryInput
                value={draft.condition}
                onChange={(condition) => set({ condition })}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void save();
                }}
                inputRef={conditionRef}
                accounts={accounts.filter((a) => scopeIds.includes(a.id))}
                labels={labels}
                placeholder="A search, e.g. from:@uber.com has:pdf"
              />
            </div>
            <MatchSummary condition={draft.condition} preview={preview} error={previewError} />
          </div>

          <div className="rl-field">
            <span className="rl-label">Then</span>
            <ol className="rl-actions">
              {draft.actions.map((a, i) => (
                <ActionRow
                  key={i}
                  action={a}
                  labelOptions={labelOptions}
                  allowHooks={allowHooks}
                  onChange={(next) => setAction(i, next)}
                  onRemove={draft.actions.length > 1 ? () => removeAction(i) : undefined}
                />
              ))}
            </ol>
            <Select<string>
              label="Add an action"
              value=""
              placeholder="Add action"
              className="rl-add"
              onChange={(k) => addAction(k as RuleAction["kind"])}
              options={ACTION_KINDS.map((k) => ({
                value: k.kind,
                label: k.label,
                hint: k.hook ? (allowHooks ? "Developer" : "Hooks are off") : k.risky ? "Asks first" : undefined,
              }))}
            />
            {hasHooks && !allowHooks && (
              <p className="rl-warn">
                <Icon name="info" size="xs" />
                Programs and webhooks are off. Turn on “Allow hooks” in Settings → Developer, or these actions are skipped.
              </p>
            )}
          </div>

          <div className="rl-options">
            <div className="setting-row">
              <div>
                <span className="setting-label">Test mode</span>
                <p className="st-muted">Log what the rule would do without doing it. New rules start here.</p>
              </div>
              <Switch label="Test mode" on={draft.dryRun} onChange={(dryRun) => set({ dryRun })} />
            </div>
            <div className="setting-row">
              <div>
                <span className="setting-label">Stop here</span>
                <p className="st-muted">Later rules skip mail this one matched.</p>
              </div>
              <Switch label="Stop processing more rules" on={draft.stopProcessing} onChange={(stopProcessing) => set({ stopProcessing })} />
            </div>
            {hasHooks && (
              <div className="setting-row">
                <div>
                  <span className="setting-label">Send message text to hooks</span>
                  <p className="st-muted">Off: programs and webhooks get headers only (from, subject, labels, attachments).</p>
                </div>
                <Switch label="Include message text" on={draft.includeBody} onChange={(includeBody) => set({ includeBody })} />
              </div>
            )}
          </div>
          {error && <p className="st-error">{error}</p>}
        </div>

        <footer className="rl-ed-foot">
          <span className="st-muted">Evaluated on this Mac. Actions go through Gmail like your own.</span>
          <span className="grow" />
          <button className="btn btn-ghost" onClick={closeRuleEditor}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={saving} onClick={() => void save()}>
            {isNew ? "Create rule" : "Save"}
            <span className="kbd">⌘↵</span>
          </button>
        </footer>
      </div>

      {confirming && (
        <ConfirmDialog
          title={confirming.action.kind === "trash" ? "Let this rule move mail to Trash?" : "Let this rule forward mail?"}
          body={
            confirming.action.kind === "trash"
              ? "Matching conversations go to Trash automatically (Gmail empties Trash after 30 days). You can undo from the rule's history."
              : `Matching messages are sent to ${confirming.action.kind === "forward" ? confirming.action.to : ""} from your account, at most 20 an hour. Forwards can't be undone.`
          }
          confirm={confirming.action.kind === "trash" ? "Allow Trash" : "Allow forwarding"}
          onCancel={() => {
            // Declined: drop the action rather than keep an unconfirmed one.
            if (confirming.action.kind === "trash") removeAction(confirming.index);
            setConfirming(null);
          }}
          onConfirm={() => {
            setAction(confirming.index, { ...confirming.action, confirmed: true });
            setConfirming(null);
          }}
        />
      )}
    </div>
  );
}

function ScheduleFields({ trigger, onChange }: { trigger: Extract<RuleTrigger, { kind: "schedule" }>; onChange: (t: RuleTrigger) => void }) {
  return (
    <div className="rl-sub-row">
      <Select
        label="How often"
        value={trigger.every}
        options={[
          { value: "daily", label: "Every day" },
          { value: "weekly", label: "Every week" },
        ]}
        onChange={(every) => onChange({ ...trigger, every, weekday: every === "weekly" ? trigger.weekday ?? 1 : null })}
      />
      {trigger.every === "weekly" && (
        <Select
          label="Day"
          value={String(trigger.weekday ?? 1)}
          options={WEEKDAYS.map((d, i) => ({ value: String(i), label: d }))}
          onChange={(d) => onChange({ ...trigger, weekday: Number(d) })}
        />
      )}
      <span className="st-muted">at</span>
      <input
        type="time"
        className="input rl-time"
        value={trigger.at}
        aria-label="Time"
        onChange={(e) => onChange({ ...trigger, at: e.target.value })}
      />
      <span className="st-muted">Handles matches it hasn't handled before.</span>
    </div>
  );
}

function MatchSummary({ condition, preview, error }: { condition: string; preview: RulePreview | null; error: string | null }) {
  if (!condition.trim()) return <p className="st-muted rl-hint">Uses the search language: from:, to:, subject:, has:pdf, label:, is:unread, older_than:… Relative dates count from when the rule runs.</p>;
  if (error) return <p className="st-error">{error}</p>;
  if (!preview) return <p className="st-muted rl-hint">Counting…</p>;
  const accounts = meta.get().accounts;
  return (
    <div className="rl-preview">
      <p className="rl-count">
        <Icon name="search" size="xs" />
        Matches <b>{num(preview.count)}{preview.capped ? "+" : ""}</b> existing {preview.count === 1 ? "message" : "messages"}
      </p>
      {preview.items.length > 0 && (
        <ul className="rl-preview-list" aria-label="Newest matches">
          {preview.items.map((m) => (
            <li key={`${m.accountId}/${m.messageId}`} className={m.alreadyApplied ? "is-done" : undefined}>
              <span className="rl-pv-from truncate">{displayName(m.from)}</span>
              <span className="rl-pv-subj truncate">{m.subject || "(no subject)"}</span>
              {accounts.length > 1 && <span className="rl-pv-acct truncate">{accounts.find((a) => a.id === m.accountId)?.email}</span>}
              <span className="rl-pv-date">{m.alreadyApplied ? "handled" : shortDate(m.date)}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function ActionRow({
  action,
  labelOptions,
  allowHooks,
  onChange,
  onRemove,
}: {
  action: RuleAction;
  labelOptions: SelectOption<string>[];
  allowHooks: boolean;
  onChange: (a: RuleAction) => void;
  onRemove?: () => void;
}) {
  const kind = ACTION_KINDS.find((k) => k.kind === action.kind);
  return (
    <li className="rl-action">
      <span className={"rl-action-kind" + (kind?.risky ? " is-risky" : "")}>{kind?.label ?? action.kind}</span>
      <span className="rl-action-args">
        {(action.kind === "addLabel" || action.kind === "removeLabel") && (
          <Select
            label="Label"
            value={action.label}
            placeholder="Pick a label"
            options={labelOptions.length ? labelOptions : [{ value: action.label, label: action.label || "No labels in these accounts", disabled: true }]}
            onChange={(label) => onChange({ ...action, label })}
          />
        )}
        {action.kind === "forward" && (
          <input
            className="input rl-input"
            type="email"
            value={action.to}
            placeholder="name@example.com"
            aria-label="Forward to"
            // A new address needs a fresh confirmation.
            onChange={(e) => onChange({ ...action, to: e.target.value, confirmed: false })}
          />
        )}
        {action.kind === "hook" && (
          <>
            <input
              className="input rl-input mono"
              value={action.program}
              placeholder="~/bin/on-mail.sh"
              aria-label="Program (full path)"
              onChange={(e) => onChange({ ...action, program: e.target.value })}
            />
            <input
              className="input rl-input mono"
              value={action.args.join(" ")}
              placeholder="arguments (optional)"
              aria-label="Arguments, separated by spaces"
              title="Passed as-is; message data arrives as JSON on stdin, never in arguments"
              onChange={(e) => onChange({ ...action, args: e.target.value.split(" ").filter(Boolean) })}
            />
          </>
        )}
        {action.kind === "webhook" && (
          <>
            <input
              className="input rl-input mono"
              value={action.url}
              placeholder="https://…"
              aria-label="Webhook URL"
              onChange={(e) => onChange({ ...action, url: e.target.value })}
            />
            <input
              className="input rl-input mono"
              value={action.secret ?? ""}
              placeholder="signing secret (optional)"
              aria-label="Signing secret"
              title="Signs each request: X-Penguin-Signature: t=<secs>,v1=<HMAC-SHA256 hex of “t.body”>"
              onChange={(e) => onChange({ ...action, secret: e.target.value || null })}
            />
          </>
        )}
        {action.kind === "trash" && !action.confirmed && <span className="rl-warn">Not confirmed</span>}
        {(action.kind === "hook" || action.kind === "webhook") && !allowHooks && <span className="badge t-amber">Hooks off</span>}
        {action.kind === "forward" && <span className="st-muted">Max 20/hour</span>}
        {action.kind === "notify" && <span className="st-muted">macOS notification</span>}
      </span>
      {onRemove && (
        <button className="btn btn-ghost btn-sm btn-icon" aria-label={`Remove ${actionText(action)}`} onClick={onRemove}>
          <Icon name="x" size="xs" />
        </button>
      )}
    </li>
  );
}

/** After creating a new-mail rule: offer to run it on existing matches, dry run first. */
function ApplyExisting({ rule, count }: { rule: Rule; count: number }) {
  const [report, setReport] = useState<RuleRunReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (dryRun: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const r = await api.runRule(rule.id, dryRun);
      if (dryRun) setReport(r);
      else {
        toast({ message: `“${rule.name}” handled ${num(r.acted)} existing ${r.acted === 1 ? "message" : "messages"}` });
        closeRuleEditor();
      }
    } catch (e) {
      setError(asCommandError(e).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="st-scrim rl-scrim">
      <div className="st-confirm rl-apply" role="alertdialog" aria-modal="true" aria-label="Apply to existing mail">
        <h3>Rule “{rule.name}” created</h3>
        <p className="st-muted">
          It runs on new mail from now on{rule.dryRun ? ", in test mode" : ""}. {num(count)} existing{" "}
          {count === 1 ? "message matches" : "messages match"} too.
        </p>
        {report && (
          <p className="rl-apply-report">
            Test run: it would act on <b>{num(report.acted)}</b> {report.acted === 1 ? "message" : "messages"}
            {report.alreadyApplied ? ` (${num(report.alreadyApplied)} already handled)` : ""}
            {report.capped ? " — the newest 1,000; run it again for more" : ""}. See the rule's history for the list.
          </p>
        )}
        {error && <p className="st-error">{error}</p>}
        <div className="st-confirm-actions">
          <button className="btn btn-ghost" onClick={closeRuleEditor}>
            Only new mail
          </button>
          {report ? (
            <button className="btn btn-primary" disabled={busy || rule.dryRun || report.acted === 0} title={rule.dryRun ? "Turn off test mode to apply" : undefined} onClick={() => void run(false)}>
              Apply to {num(report.acted)}
            </button>
          ) : (
            <button className="btn btn-primary" disabled={busy} onClick={() => void run(true)}>
              Test on existing mail
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
