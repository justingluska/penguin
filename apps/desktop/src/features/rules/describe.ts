// Plain-language summaries of rules for the list and history. OWNER: rules agent.
import type { Account, Label, Profile, RuleAction, RuleInput, RuleLogEntry, RuleTrigger } from "../../lib/types";

export const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

export function triggerText(t: RuleTrigger): string {
  switch (t.kind) {
    case "newMessage":
      return "When new mail arrives";
    case "labelAdded":
      return `When a message gets ${t.label}`;
    case "schedule":
      return t.every === "daily" ? `Every day at ${t.at}` : `Every ${WEEKDAYS[t.weekday ?? 1]} at ${t.at}`;
    case "manual":
      return "Only when you run it";
  }
}

export function labelName(label: string, labels: Label[]): string {
  const l = labels.find((x) => x.id === label || x.name.toLowerCase() === label.toLowerCase());
  return l?.name ?? label;
}

export function actionText(a: RuleAction, labels: Label[] = []): string {
  switch (a.kind) {
    case "addLabel":
      return `Label ${labelName(a.label, labels)}`;
    case "removeLabel":
      return `Remove ${labelName(a.label, labels)}`;
    case "archive":
      return "Archive";
    case "markRead":
      return "Mark read";
    case "star":
      return "Star";
    case "trash":
      return "Move to Trash";
    case "forward":
      return `Forward to ${a.to || "…"}`;
    case "notify":
      return "Notify me";
    case "hook":
      return `Run ${a.program.split("/").pop() || "a hook"}`;
    case "webhook": {
      try {
        return `Webhook to ${new URL(a.url).host}`;
      } catch {
        return "Webhook";
      }
    }
  }
}

export function scopeText(r: Pick<RuleInput, "accountIds" | "profileId">, accounts: Account[], profiles: Profile[]): string {
  if (r.profileId) return profiles.find((p) => p.id === r.profileId)?.name ?? "A removed profile";
  if (!r.accountIds) return "All accounts";
  const names = r.accountIds.map((id) => accounts.find((a) => a.id === id)?.email ?? id);
  return names.length === 1 ? names[0] : `${names.length} accounts`;
}

export const TRIGGER_TEXT: Record<RuleLogEntry["trigger"], string> = {
  newMessage: "new mail",
  labelAdded: "label added",
  schedule: "schedule",
  manual: "run by you",
};

export const ACTION_KINDS: Array<{ kind: RuleAction["kind"]; label: string; risky?: boolean; hook?: boolean }> = [
  { kind: "addLabel", label: "Add label" },
  { kind: "removeLabel", label: "Remove label" },
  { kind: "archive", label: "Archive" },
  { kind: "markRead", label: "Mark read" },
  { kind: "star", label: "Star" },
  { kind: "notify", label: "Notify me" },
  { kind: "trash", label: "Move to Trash", risky: true },
  { kind: "forward", label: "Forward", risky: true },
  { kind: "hook", label: "Run a program", hook: true },
  { kind: "webhook", label: "Call a webhook", hook: true },
];

export function defaultAction(kind: RuleAction["kind"], labels: Label[]): RuleAction {
  const firstUser = labels.find((l) => l.kind === "user")?.name ?? "";
  switch (kind) {
    case "addLabel":
    case "removeLabel":
      return { kind, label: firstUser };
    case "trash":
      return { kind, confirmed: false };
    case "forward":
      return { kind, to: "", confirmed: false };
    case "hook":
      return { kind, program: "", args: [], timeoutSecs: 10 };
    case "webhook":
      return { kind, url: "", secret: null };
    default:
      return { kind } as RuleAction;
  }
}
