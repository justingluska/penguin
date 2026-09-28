// Settings → Rules: the rule list (toggle, summary, stats), per-rule
// history with Undo, "Run now", and the editor. Settings → Developer gets
// the "Allow hooks" switch (RuleHooksPanel). OWNER: rules agent.
import { useCallback, useEffect, useState } from "react";
import { api, asCommandError, onRulesChanged } from "../../lib/api";
import type { Rule, RuleLogEntry, RulesOverview, RuleStats } from "../../lib/types";
import { meta } from "../../app/store";
import { useProfiles } from "../../app/profiles";
import { Icon } from "../../components/Icon";
import { openMenu } from "../../components/ContextMenu";
import { toast } from "../../components/Toast";
import { ago, num } from "../../lib/format";
import { ConfirmDialog, Section, Switch } from "../settings/parts";
import { actionText, scopeText, triggerText, TRIGGER_TEXT } from "./describe";
import { RuleEditor } from "./RuleEditor";
import { openRuleEditor, toInput, useRuleEditor } from "./state";
import { AUTO_LABELS } from "./autoLabels";
import "./rules.css";

function useRules(): [RulesOverview | null, string | null, () => void] {
  const [data, setData] = useState<RulesOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => {
    api
      .listRules()
      .then((d) => (setData(d), setError(null)))
      .catch((e) => setError(asCommandError(e).message));
  }, []);
  useEffect(() => {
    load();
    let off: (() => void) | null = null;
    let dead = false;
    void onRulesChanged(setData).then((u) => (dead ? u() : (off = u)));
    return () => {
      dead = true;
      off?.();
    };
  }, [load]);
  return [data, error, load];
}

export function RulesSection() {
  const [data, error, reload] = useRules();
  const editing = useRuleEditor();
  const [deleting, setDeleting] = useState<Rule | null>(null);
  const [historyFor, setHistoryFor] = useState<string | null>(null);
  const rules = data?.rules ?? [];

  const fail = (what: string) => (e: unknown) => toast({ tone: "error", message: `Couldn't ${what}: ${asCommandError(e).message}` });

  const toggle = (r: Rule, enabled: boolean) => api.setRuleEnabled(r.id, enabled).then(reload, fail("change the rule"));
  const move = (r: Rule, delta: number) => {
    const ids = rules.map((x) => x.id);
    const i = ids.indexOf(r.id);
    const j = i + delta;
    if (j < 0 || j >= ids.length) return;
    [ids[i], ids[j]] = [ids[j], ids[i]];
    api.reorderRules(ids).then(reload, fail("reorder"));
  };
  const run = async (r: Rule, dryRun: boolean) => {
    try {
      const rep = await api.runRule(r.id, dryRun);
      const n = `${num(rep.acted)} ${rep.acted === 1 ? "message" : "messages"}`;
      toast({
        message: rep.acted === 0 ? `“${r.name}”: nothing new to handle (${num(rep.matched)} matched, all handled before)` : dryRun ? `Test run of “${r.name}”: would act on ${n}` : `“${r.name}” handled ${n}`,
        detail: rep.capped ? "Only the newest 1,000 matches per run." : undefined,
      });
      setHistoryFor(r.id);
      reload();
    } catch (e) {
      fail("run the rule")(e);
    }
  };
  const remove = (r: Rule) => {
    setDeleting(null);
    api.deleteRule(r.id).then(() => toast({ message: `Deleted “${r.name}”` }), fail("delete the rule"));
  };

  const more = (r: Rule, anchor: HTMLElement) =>
    openMenu(anchor.getBoundingClientRect(), [
      { label: "Edit…", icon: "pencil", onSelect: () => openRuleEditor(toInput(r)) },
      { label: "Test run on existing mail", icon: "eye", onSelect: () => void run(r, true) },
      { label: r.dryRun ? "Run now (test mode: logs only)" : "Run now on existing mail", icon: "zap", onSelect: () => void run(r, r.dryRun) },
      { label: "History", icon: "history", onSelect: () => setHistoryFor(historyFor === r.id ? null : r.id) },
      { type: "separator" },
      { label: "Move up", icon: "arrowup", disabled: rules[0]?.id === r.id, onSelect: () => move(r, -1) },
      { label: "Move down", icon: "arrowdown", disabled: rules[rules.length - 1]?.id === r.id, onSelect: () => move(r, 1) },
      { type: "separator" },
      { label: "Delete…", icon: "trash", danger: true, onSelect: () => setDeleting(r) },
    ]);

  return (
    <Section
      id="rules"
      icon="wand"
      title="Rules"
      badge={
        <button className="btn btn-sm" data-setting="new-rule" onClick={() => openRuleEditor()}>
          <Icon name="plus" size="xs" />
          New rule
        </button>
      }
    >
      <p className="st-muted rl-intro">
        Rules run on this Mac as mail arrives, on a schedule, or when you run them. Conditions are searches, so what you
        can find you can automate. Every new rule starts in test mode.
      </p>
      {error && <p className="st-error">Couldn't load rules: {error}</p>}
      {data && rules.length === 0 && <EmptyRules />}
      <ul className="rl-list">
        {rules.map((r) => (
          <RuleRow
            key={r.id}
            rule={r}
            stats={data?.stats.find((s) => s.ruleId === r.id)}
            onToggle={(v) => toggle(r, v)}
            onEdit={() => openRuleEditor(toInput(r))}
            onMore={(el) => more(r, el)}
            historyOpen={historyFor === r.id}
            onHistory={() => setHistoryFor(historyFor === r.id ? null : r.id)}
          />
        ))}
      </ul>
      {data && <AutoLabels have={rules.map((r) => r.name)} />}
      {editing && <RuleEditor key={editing.id ?? "new"} initial={editing} allowHooks={data?.allowHooks ?? false} />}
      {deleting && (
        <ConfirmDialog
          title={`Delete “${deleting.name}”?`}
          body="The rule and its history are removed. Mail it already handled stays as it is."
          confirm="Delete rule"
          onConfirm={() => remove(deleting)}
          onCancel={() => setDeleting(null)}
        />
      )}
    </Section>
  );
}

/**
 * Auto labels and auto archive (./autoLabels.ts): one click opens the
 * editor with the rule filled in, in test mode. Ones already added are left out.
 */
function AutoLabels({ have }: { have: string[] }) {
  const left = AUTO_LABELS.filter((a) => !have.includes(a.draft.name ?? ""));
  if (left.length === 0) return null;
  return (
    <div className="rl-auto" data-setting="auto-labels">
      <h3 className="st-sub">Auto labels</h3>
      <p className="st-muted">
        Sort mail as it arrives, and keep some of it out of the inbox. Mail from people you've written to is left alone.
      </p>
      <div className="rl-auto-list">
        {left.map((a) => (
          <button key={a.key} className="rl-example" title={a.draft.condition} onClick={() => openRuleEditor(a.draft)}>
            <strong>{a.title}</strong>
            <span className="st-muted">{a.hint}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function EmptyRules() {
  const examples: Array<[string, string, Parameters<typeof openRuleEditor>[0]]> = [
    ["File receipts", "from:uber.com has:pdf → label Receipts, archive", { name: "File receipts", condition: "has:pdf subject:receipt", actions: [{ kind: "addLabel", label: "Receipts" }, { kind: "archive" }] }],
    ["Newsletters out of the inbox", "label:newsletters is:read → archive, every morning", { name: "Tidy newsletters", trigger: { kind: "schedule", every: "daily", at: "07:00", weekday: null }, condition: "in:inbox is:read older_than:2d label:newsletters", actions: [{ kind: "archive" }] }],
    ["Ping me for my manager", "from:boss@… → star, notify", { name: "Mail from my manager", condition: "from:", actions: [{ kind: "star" }, { kind: "notify" }] }],
  ];
  return (
    <div className="rl-empty">
      <span className="st-muted">Start from an example:</span>
      {examples.map(([title, hint, draft]) => (
        <button key={title} className="rl-example" onClick={() => openRuleEditor(draft)}>
          <strong>{title}</strong>
          <span className="st-muted mono">{hint}</span>
        </button>
      ))}
    </div>
  );
}

function RuleRow({
  rule,
  stats,
  onToggle,
  onEdit,
  onMore,
  historyOpen,
  onHistory,
}: {
  rule: Rule;
  stats: RuleStats | undefined;
  onToggle: (v: boolean) => void;
  onEdit: () => void;
  onMore: (anchor: HTMLElement) => void;
  historyOpen: boolean;
  onHistory: () => void;
}) {
  const accounts = meta.use((m) => m.accounts);
  const labels = meta.use((m) => m.labels);
  const profiles = useProfiles();
  return (
    <li className={"rl-row" + (rule.enabled ? "" : " is-off")}>
      <div className="rl-row-main">
        <button className="rl-row-text" onClick={onEdit} title="Edit rule">
          <span className="rl-row-title">
            <strong className="truncate">{rule.name}</strong>
            {rule.dryRun && <span className="badge t-violet">Test mode</span>}
            {rule.pausedReason && <span className="badge t-amber" title={rule.pausedReason}>Paused</span>}
            {rule.stopProcessing && <span className="badge t-gray" title="Later rules skip mail this one matched">Stops</span>}
          </span>
          <span className="rl-row-sum truncate">
            {triggerText(rule.trigger)} · {scopeText(rule, accounts, profiles)} · <span className="mono">{rule.condition}</span>
          </span>
          <span className="rl-row-sum truncate">→ {rule.actions.map((a) => actionText(a, labels)).join(", ")}</span>
          {rule.pausedReason && <span className="rl-warn">{rule.pausedReason}</span>}
        </button>
        <div className="rl-row-side">
          <StatsLine stats={stats} dryRun={rule.dryRun} onHistory={onHistory} />
          <div className="rl-row-ctl">
            <button className="btn btn-ghost btn-sm btn-icon" aria-label={`More for ${rule.name}`} onClick={(e) => onMore(e.currentTarget)}>
              <Icon name="more" size="xs" />
            </button>
            <Switch label={`${rule.name} enabled`} on={rule.enabled} onChange={onToggle} />
          </div>
        </div>
      </div>
      {historyOpen && <RuleHistory ruleId={rule.id} />}
    </li>
  );
}

function StatsLine({ stats, dryRun, onHistory }: { stats: RuleStats | undefined; dryRun: boolean; onHistory: () => void }) {
  if (!stats?.lastRunAt) return <span className="st-muted rl-stats">Hasn't matched yet</span>;
  const n = dryRun ? stats.dryRunMatches : stats.applied;
  return (
    <button className="st-link rl-stats" onClick={onHistory}>
      {dryRun ? `Would have acted on ${num(n)}` : `Acted on ${num(n)}`} · {ago(stats.lastRunAt)}
      {stats.lastError && (
        <span className="rl-err" title={stats.lastError}>
          <Icon name="info" size="2xs" /> error
        </span>
      )}
    </button>
  );
}

function RuleHistory({ ruleId }: { ruleId: string }) {
  const [rows, setRows] = useState<RuleLogEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => {
    api
      .ruleHistory(ruleId, 50)
      .then(setRows)
      .catch((e) => setError(asCommandError(e).message));
  }, [ruleId]);
  useEffect(load, [load]);
  useEffect(() => {
    let off: (() => void) | null = null;
    let dead = false;
    void onRulesChanged(() => load()).then((u) => (dead ? u() : (off = u)));
    return () => {
      dead = true;
      off?.();
    };
  }, [load]);

  const undo = async (ids: number[]) => {
    try {
      const n = await api.undoRuleActions(ids);
      toast({ message: n ? `Undid ${n === 1 ? "1 message" : `${n} messages`}` : "Nothing to undo" });
      load();
    } catch (e) {
      toast({ tone: "error", message: `Couldn't undo: ${asCommandError(e).message}` });
    }
  };

  if (error) return <p className="st-error rl-history">{error}</p>;
  if (!rows) return <p className="st-muted rl-history">Loading history…</p>;
  if (!rows.length) return <p className="st-muted rl-history">No history yet.</p>;
  const undoable = (r: RuleLogEntry) => !r.dryRun && !r.undone && r.outcomes.some((o) => o.ok && o.undo);
  return (
    <div className="rl-history">
      <table className="rl-log">
        <tbody>
          {rows.map((r) => (
            <tr key={r.id} className={(r.ok ? "" : "is-bad ") + (r.undone ? "is-undone" : "")}>
              <td className="rl-log-when">{ago(r.ts)}</td>
              <td className="rl-log-what">
                <span className="truncate">{r.subject ?? (r.matched === 0 ? "Whole run" : "(message)")}</span>
                {r.fromEmail && <span className="st-muted truncate">{r.fromEmail}</span>}
              </td>
              <td className="rl-log-out">
                {r.dryRun && <span className="badge t-violet">test</span>}
                <span className="badge t-gray">{TRIGGER_TEXT[r.trigger]}</span>
                {r.outcomes.map((o, i) => (
                  <span key={i} className={"badge " + (o.ok ? "t-green" : "t-red")} title={o.detail || undefined}>
                    {o.action}
                  </span>
                ))}
              </td>
              <td className="rl-log-act">
                {r.undone ? <span className="st-muted">undone</span> : undoable(r) ? (
                  <button className="btn btn-ghost btn-sm" onClick={() => void undo([r.id])}>
                    Undo
                  </button>
                ) : null}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {rows.filter(undoable).length > 1 && (
        <button className="btn btn-ghost btn-sm" onClick={() => void undo(rows.filter(undoable).map((r) => r.id))}>
          <Icon name="undo" size="xs" />
          Undo all shown
        </button>
      )}
    </div>
  );
}

/** Settings → Developer: the switch that lets rules run programs and call webhooks. */
export function RuleHooksPanel() {
  const [data, error, reload] = useRules();
  const [confirming, setConfirming] = useState(false);
  const set = (allow: boolean) =>
    api.setAllowHooks(allow).then(reload, (e) => toast({ tone: "error", message: `Couldn't save: ${asCommandError(e).message}` }));
  const users = data?.rules.filter((r) => r.actions.some((a) => a.kind === "hook" || a.kind === "webhook")).length ?? 0;
  return (
    <div className="rl-hooks">
      <h3 className="st-sub">Rule hooks</h3>
      <div className="setting-row setting-tall">
        <div>
          <span className="setting-label">Allow hooks</span>
          <p className="st-muted">
            Lets rules run programs on this Mac and call webhooks, with matching mail as JSON (ruleMatch v1). Programs run
            as you, with arguments exactly as typed; message data only arrives on stdin, never in a shell.
          </p>
          {users > 0 && <p className="st-muted">{users === 1 ? "1 rule uses" : `${users} rules use`} hooks.</p>}
          {data?.auditLogPath && (
            <p className="st-muted">
              Every rule action is logged (ids and outcomes, no content) to <span className="mono">{data.auditLogPath}</span>
            </p>
          )}
        </div>
        <Switch label="Allow hooks" on={data?.allowHooks ?? false} disabled={!data} onChange={(v) => (v ? setConfirming(true) : void set(false))} />
      </div>
      {error && <p className="st-error">{error}</p>}
      {confirming && (
        <ConfirmDialog
          title="Allow rules to run programs and call webhooks?"
          body="A rule with a hook can run any program you point it at and send mail details to any https address. Only turn this on for hooks you wrote or trust."
          confirm="Allow hooks"
          onCancel={() => setConfirming(false)}
          onConfirm={() => {
            setConfirming(false);
            void set(true);
          }}
        />
      )}
    </div>
  );
}
