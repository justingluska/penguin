// Rules & automations, UI state and entry points. OWNER: rules agent.
//
// The editor opens over Settings → Rules from three places: "New rule" in
// the section, "Create rule" in the search overlay (the query becomes the
// condition), and "Always do this…" (⇧A / ⌘K) on the selected conversation.
// A fired rule shows a toast with Undo for the thread actions it took.
import { useSyncExternalStore } from "react";
import { api, asCommandError, onRuleFired } from "../../lib/api";
import type { Rule, RuleAction, RuleInput, ThreadRef } from "../../lib/types";
import { registerShortcuts } from "../../lib/keyboard";
import { getUi } from "../../lib/ui";
import { toast } from "../../components/Toast";
import { openSettings } from "../settings/state";
import { fetchThread } from "../../app/store";

export function blankRule(): RuleInput {
  return {
    id: null,
    name: "",
    enabled: true,
    // Every new rule starts in test mode: it logs what it would do.
    dryRun: true,
    accountIds: null,
    profileId: null,
    trigger: { kind: "newMessage" },
    condition: "",
    actions: [{ kind: "archive" }],
    stopProcessing: false,
    includeBody: false,
  };
}

export function toInput(r: Rule): RuleInput {
  const { createdAt: _c, updatedAt: _u, pausedReason: _p, ...rest } = r;
  return rest;
}

let editing: RuleInput | null = null;
const subs = new Set<() => void>();
const notify = () => subs.forEach((f) => f());

/** Open the editor (over Settings → Rules) with a new or existing rule. */
export function openRuleEditor(draft: Partial<RuleInput> = {}) {
  editing = { ...blankRule(), ...draft };
  openSettings("rules");
  notify();
}

export function closeRuleEditor() {
  editing = null;
  notify();
}

export function useRuleEditor(): RuleInput | null {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => editing,
  );
}

/** "Always do this…": a rule for mail like the conversation's latest sender. */
export async function ruleFromThread(ref: ThreadRef, actions: RuleAction[] = [{ kind: "archive" }]) {
  const t = await fetchThread(ref);
  const last = t?.messages[t.messages.length - 1];
  if (!t || !last) {
    toast({ tone: "error", message: "Couldn't read that conversation" });
    return;
  }
  const who = last.from.name?.trim() || last.from.email;
  openRuleEditor({
    name: `Mail from ${who}`,
    condition: `from:${last.from.email}`,
    accountIds: [ref.accountId],
    actions,
  });
}

/** ⇧A on a selected conversation; also listed in ⌘K. */
export function registerRuleShortcuts(): () => void {
  return registerShortcuts([
    {
      id: "rules.fromThread",
      keys: "shift+a",
      label: "Always do this… (rule for mail like this)",
      group: "Triage",
      when: () => getUi().overlay === null && getUi().selected !== null,
      run: () => {
        const sel = getUi().selected;
        if (sel) void ruleFromThread(sel);
      },
    },
  ]);
}

/** Toasts for rules that acted: "File receipts archived 3 conversations · Undo". */
export function installRuleToasts(): () => void {
  let off: (() => void) | null = null;
  let dead = false;
  void onRuleFired((e) => {
    if (e.dryRun || e.count === 0) return;
    const n = e.count === 1 ? "1 message" : `${e.count} messages`;
    const failed = e.failed ? ` (${e.failed} with errors)` : "";
    toast({
      key: `rule:${e.ruleId}`,
      message: `Rule “${e.ruleName}” handled ${n}${failed}`,
      action: {
        label: "Undo",
        run: async () => {
          try {
            const undone = await api.undoRuleActions(e.logIds);
            toast({ message: undone ? `Undid “${e.ruleName}”` : "Nothing to undo (forwards, notifications and hooks can't be taken back)" });
          } catch (err) {
            toast({ tone: "error", message: `Couldn't undo: ${asCommandError(err).message}` });
          }
        },
      },
    });
  }).then((u) => {
    if (dead) u();
    else off = u;
  });
  return () => {
    dead = true;
    off?.();
  };
}
