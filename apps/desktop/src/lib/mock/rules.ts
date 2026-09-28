// OWNER: rules agent.
//
// Mock rules & automations: a few example rules over the fictional mock
// world, previews through the mock search (so a condition counts what the
// search overlay would find), and a plausible history. Nothing runs by
// itself here; "Run now" writes history rows and fires penguin://rule-fired.
import type {
  Rule,
  RuleInput,
  RuleLogEntry,
  RuleOutcome,
  RulePreview,
  RuleRunReport,
  RulesOverview,
  RuleStats,
  SearchResponse,
} from "../types";
import type { MockHandler } from "./index";

const HOUR = 3_600_000;
const now = Date.now();
const NW = "acc-northwind";
const HL = "acc-harbor";
const PE = "acc-personal";

function rule(r: Partial<Rule> & Pick<Rule, "id" | "name" | "condition" | "actions" | "trigger">): Rule {
  return {
    enabled: true,
    dryRun: false,
    accountIds: null,
    profileId: null,
    stopProcessing: false,
    includeBody: false,
    createdAt: now - 30 * 24 * HOUR,
    updatedAt: now - 3 * 24 * HOUR,
    pausedReason: null,
    ...r,
  };
}

let allowHooks = false;
let rules: Rule[] = [
  rule({
    id: "r_receipts",
    name: "File receipts",
    trigger: { kind: "newMessage" },
    condition: "from:ledgerly.example OR from:northlineair.example has:pdf",
    actions: [{ kind: "addLabel", label: "Receipts" }, { kind: "archive" }],
    stopProcessing: true,
  }),
  rule({
    id: "r_landlord",
    name: "Landlord pings me",
    trigger: { kind: "newMessage" },
    condition: "from:cedarpine.example",
    profileId: "p-home",
    actions: [{ kind: "star" }, { kind: "notify" }],
  }),
  rule({
    id: "r_digest",
    name: "Monday unread digest",
    trigger: { kind: "schedule", every: "weekly", at: "08:00", weekday: 1 },
    condition: "is:unread in:inbox older_than:3d",
    accountIds: [NW, HL],
    actions: [{ kind: "notify" }],
    dryRun: true,
  }),
  rule({
    id: "r_books",
    name: "Invoices to bookkeeping",
    trigger: { kind: "newMessage" },
    condition: "subject:invoice has:pdf",
    accountIds: [PE],
    actions: [{ kind: "forward", to: "books@ledger.example", confirmed: true }],
    dryRun: true,
  }),
  rule({
    id: "r_hook",
    name: "Pipe CI failures to my script",
    enabled: false,
    trigger: { kind: "newMessage" },
    condition: 'from:ci@harborlabs.example subject:"failed"',
    actions: [
      { kind: "hook", program: "~/bin/on-ci-failure", args: ["--quiet"], timeoutSecs: 10 },
      { kind: "webhook", url: "https://hooks.example/penguin", secret: "whsec_example" },
    ],
  }),
];

const outcome = (action: RuleOutcome["action"], ok = true, detail = "", undo?: RuleOutcome["undo"]): RuleOutcome => ({ action, ok, detail, ...(undo ? { undo } : {}) });

let nextLog = 100;
let log: RuleLogEntry[] = [
  {
    id: nextLog++, ts: now - 2 * HOUR, ruleId: "r_receipts", trigger: "newMessage", dryRun: false, ok: true,
    accountId: HL, threadId: "t-mock-ledgerly", messageId: "m-mock-ledgerly", subject: "Your Ledgerly invoice #4821",
    fromEmail: "billing@ledgerly.example", matched: 1,
    outcomes: [outcome("addLabel", true, "", { kind: "removeLabel", labelId: "Label_receipts" }), outcome("archive", true, "", { kind: "moveToInbox" })],
    undone: false,
  },
  {
    id: nextLog++, ts: now - 26 * HOUR, ruleId: "r_receipts", trigger: "newMessage", dryRun: false, ok: true,
    accountId: PE, threadId: "t-mock-northline", messageId: "m-mock-northline", subject: "Your Northline Air e-ticket receipt",
    fromEmail: "trips@northlineair.example", matched: 1,
    outcomes: [outcome("addLabel", true, "", { kind: "removeLabel", labelId: "Label_receipts" }), outcome("archive", true, "", { kind: "moveToInbox" })],
    undone: false,
  },
  {
    id: nextLog++, ts: now - 5 * HOUR, ruleId: "r_landlord", trigger: "newMessage", dryRun: false, ok: true,
    accountId: PE, threadId: "t-mock-cedar", messageId: "m-mock-cedar", subject: "Water shutoff Thursday 9–11am",
    fromEmail: "office@cedarpine.example", matched: 1,
    outcomes: [outcome("star", true, "", { kind: "unstar" }), outcome("notify")],
    undone: false,
  },
  {
    id: nextLog++, ts: now - 50 * HOUR, ruleId: "r_books", trigger: "newMessage", dryRun: true, ok: true,
    accountId: PE, threadId: "t-mock-plumber", messageId: "m-mock-plumber", subject: "Invoice 2291 — kitchen sink repair",
    fromEmail: "service@alderplumbing.example", matched: 1,
    outcomes: [outcome("forward", true, "would forward to books@ledger.example")],
    undone: false,
  },
  {
    id: nextLog++, ts: now - 72 * HOUR, ruleId: "r_hook", trigger: "newMessage", dryRun: false, ok: false,
    accountId: HL, threadId: "t-mock-ci", messageId: "m-mock-ci", subject: "[harbor/api] Build failed on main",
    fromEmail: "ci@harborlabs.example", matched: 1,
    outcomes: [outcome("hook", false, "skipped: hooks are off (Settings → Developer)"), outcome("webhook", false, "skipped: hooks are off (Settings → Developer)")],
    undone: false,
  },
];

function stats(): RuleStats[] {
  return rules.map((r) => {
    const rows = log.filter((l) => l.ruleId === r.id);
    const failed = rows.filter((l) => !l.ok);
    return {
      ruleId: r.id,
      applied: rows.filter((l) => !l.dryRun && l.ok).reduce((n, l) => n + l.matched, 0),
      dryRunMatches: rows.filter((l) => l.dryRun).reduce((n, l) => n + l.matched, 0),
      lastRunAt: rows.length ? Math.max(...rows.map((l) => l.ts)) : null,
      lastErrorAt: failed.length ? Math.max(...failed.map((l) => l.ts)) : null,
      lastError: failed[0]?.outcomes.find((o) => !o.ok)?.detail ?? null,
    };
  });
}

function overview(): RulesOverview {
  return {
    allowHooks,
    rules: rules.map((r) => ({ ...r })),
    stats: stats(),
    auditLogPath: "~/Library/Logs/co.gluska.penguin/rules-audit.log",
  };
}

async function emit(event: string, payload: unknown) {
  const { mockBackend } = await import("./index");
  mockBackend.emit(event, payload);
}

const changed = () => emit("penguin://rules-changed", overview());

function invalid(message: string): never {
  throw { code: "invalidInput", message };
}

function validate(input: RuleInput) {
  if (!input.name.trim()) invalid("Give the rule a name.");
  if (!input.condition.trim()) invalid("Add a condition (a search, e.g. from:@uber.com has:pdf).");
  if (!input.actions.length) invalid("Add at least one action.");
  for (const a of input.actions) {
    if (a.kind === "trash" && !a.confirmed) invalid("Confirm that this rule may move mail to Trash.");
    if (a.kind === "forward" && !a.confirmed) invalid("Confirm that this rule may forward mail.");
    if (a.kind === "forward" && !/^[^\s@<>,;"]+@[^\s@<>,;"]+\.[^\s@<>,;"]+$/.test(a.to.trim())) invalid(`"${a.to}" isn't an email address.`);
    if (a.kind === "hook" && !/^(~\/|\/)/.test(a.program.trim())) invalid("Give the hook's full path (e.g. ~/bin/on-receipt.sh).");
    if (a.kind === "webhook" && !/^(https:\/\/|http:\/\/(localhost|127\.0\.0\.1)(:|\/|$))/.test(a.url.trim())) invalid("Webhooks must use https (http only to localhost).");
    if ((a.kind === "addLabel" || a.kind === "removeLabel") && !a.label.trim()) invalid("Pick a label.");
  }
}

async function search(condition: string, accountIds: string[] | null): Promise<SearchResponse> {
  const { mockBackend } = await import("./index");
  return mockBackend.invoke<SearchResponse>("search", {
    request: { query: condition, accountId: null, accountIds, limit: 200 },
  });
}

function scopeOf(accountIds: string[] | null, profileId: string | null): string[] | null {
  const profiles: Record<string, string[]> = { "p-work": [NW, HL], "p-northwind": [NW], "p-home": [PE] };
  if (profileId) return (profiles[profileId] ?? []).filter((a) => !accountIds || accountIds.includes(a));
  return accountIds;
}

export const ruleHandlers: Record<string, MockHandler> = {
  list_rules: () => overview(),
  save_rule: async ({ rule: input }) => {
    const r = input as RuleInput;
    validate(r);
    const prev = r.id ? rules.find((x) => x.id === r.id) : undefined;
    if (r.id && !prev) invalid("That rule no longer exists.");
    const saved: Rule = {
      ...r,
      id: prev?.id ?? `r_${Math.random().toString(16).slice(2, 14)}`,
      name: r.name.trim().replace(/\s+/g, " "),
      condition: r.condition.trim(),
      createdAt: prev?.createdAt ?? Date.now(),
      updatedAt: Date.now(),
      pausedReason: null,
    };
    rules = prev ? rules.map((x) => (x.id === saved.id ? saved : x)) : [...rules, saved];
    await changed();
    return saved;
  },
  delete_rule: async ({ id }) => {
    rules = rules.filter((r) => r.id !== id);
    log = log.filter((l) => l.ruleId !== id);
    await changed();
  },
  set_rule_enabled: async ({ id, enabled }) => {
    const r = rules.find((x) => x.id === id);
    if (!r) invalid("That rule no longer exists.");
    r.enabled = enabled;
    if (enabled) r.pausedReason = null;
    await changed();
    return { ...r };
  },
  reorder_rules: async ({ ids }) => {
    const order = ids as string[];
    rules = [...rules].sort((a, b) => order.indexOf(a.id) - order.indexOf(b.id));
    await changed();
  },
  set_allow_hooks: async ({ allow }) => {
    allowHooks = !!allow;
    await changed();
    return overview();
  },
  preview_rule: async ({ condition, accountIds, profileId, ruleId, limit }): Promise<RulePreview> => {
    if (!String(condition).trim()) invalid("a rule condition needs at least one search term");
    const res = await search(condition, scopeOf(accountIds, profileId));
    const applied = new Set(log.filter((l) => l.ruleId === ruleId && !l.dryRun).map((l) => l.threadId));
    return {
      count: res.hits.reduce((n, h) => n + h.matchCount, 0),
      capped: false,
      items: res.hits.slice(0, limit ?? 50).map((h) => ({
        accountId: h.accountId,
        threadId: h.threadId,
        messageId: h.messageId,
        subject: h.subject,
        from: h.from,
        date: h.date,
        alreadyApplied: applied.has(h.threadId),
      })),
    };
  },
  run_rule: async ({ id, dryRun }): Promise<RuleRunReport> => {
    const r = rules.find((x) => x.id === id);
    if (!r) throw { code: "notFound", message: "That rule no longer exists." };
    const res = await search(r.condition, scopeOf(r.accountIds, r.profileId));
    const done = new Set(log.filter((l) => l.ruleId === id && !l.dryRun).map((l) => l.threadId));
    const todo = res.hits.filter((h) => !done.has(h.threadId));
    const rows: RuleLogEntry[] = todo.map((h) => ({
      id: nextLog++,
      ts: Date.now(),
      ruleId: r.id,
      trigger: "manual",
      dryRun,
      ok: true,
      accountId: h.accountId,
      threadId: h.threadId,
      messageId: h.messageId,
      subject: h.subject,
      fromEmail: h.from.email,
      matched: 1,
      outcomes: r.actions.map((a) => outcome(a.kind, true, dryRun ? `would ${a.kind}` : "")),
      undone: false,
    }));
    log = [...rows.reverse(), ...log];
    if (rows.length) {
      await emit("penguin://rule-fired", {
        ruleId: r.id, ruleName: r.name, trigger: "manual", dryRun, count: rows.length, failed: 0, logIds: rows.map((x) => x.id),
      });
      await changed();
    }
    return {
      ruleId: r.id, dryRun, matched: res.hits.length, alreadyApplied: res.hits.length - todo.length,
      acted: rows.length, failed: 0, capped: false, logIds: rows.map((x) => x.id),
    };
  },
  rule_history: ({ ruleId, limit, beforeId }) =>
    log
      .filter((l) => (!ruleId || l.ruleId === ruleId) && (beforeId == null || l.id < beforeId))
      .sort((a, b) => b.ts - a.ts || b.id - a.id)
      .slice(0, limit ?? 100),
  undo_rule_actions: async ({ logIds }) => {
    let n = 0;
    for (const l of log) {
      if ((logIds as number[]).includes(l.id) && !l.dryRun && !l.undone && l.outcomes.some((o) => o.ok && o.undo)) {
        l.undone = true;
        n++;
      }
    }
    return n;
  },
};
