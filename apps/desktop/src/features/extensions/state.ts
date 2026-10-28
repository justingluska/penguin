// Extensions (src-tauri/src/extensions.rs, docs/EXTENSIONS.md): what's
// installed and allowed, and running an action on a message. The list is
// read when something first asks for it and again whenever the window comes
// back to the front, so a folder dropped in Finder shows up without a
// restart. OWNER: extensions.
import { useEffect, useSyncExternalStore } from "react";
import type { ExtensionAction, ExtensionInfo, ExtensionResult, ExtensionsOverview, MessageView } from "../../lib/types";
import { api, asCommandError } from "../../lib/api";
import { dismissToastKey, toast } from "../../components/Toast";
import { openSettings } from "../settings/state";
import { openExtensionSheet } from "./ExtensionForm";

let overview: ExtensionsOverview | null = null;
let loading: Promise<void> | null = null;
let watching = false;
const subs = new Set<() => void>();

function set(next: ExtensionsOverview) {
  overview = next;
  subs.forEach((f) => f());
}

export function loadExtensions(): Promise<void> {
  if (!watching && typeof window !== "undefined") {
    watching = true;
    window.addEventListener("focus", () => void loadExtensions());
  }
  loading ??= api
    .listExtensions()
    .then(set)
    .catch(() => {})
    .finally(() => (loading = null));
  return loading;
}

/** Settings → Extensions changed something: take its answer as the list. */
export function setExtensions(next: ExtensionsOverview) {
  set(next);
}

export function useExtensions(): ExtensionsOverview | null {
  const v = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => overview,
  );
  useEffect(() => {
    if (!overview) void loadExtensions();
  }, []);
  return v;
}

export interface MessageAction {
  ext: ExtensionInfo;
  action: ExtensionAction;
}

/** Every action of the allowed extensions, in folder order. */
export function allowedActions(o: ExtensionsOverview | null = overview): MessageAction[] {
  return (o?.extensions ?? []).filter((e) => e.state === "on").flatMap((ext) => ext.actions.map((action) => ({ ext, action })));
}

/** A form default with `{{subject}}`, `{{from.name}}`, `{{from.email}}`, `{{snippet}}` filled in. */
export function fillTemplate(text: string, m: MessageView): string {
  const values: Record<string, string> = {
    subject: m.subject,
    "from.name": m.from.name ?? m.from.email,
    "from.email": m.from.email,
    snippet: m.snippet,
  };
  return text.replace(/\{\{\s*([a-z.]+)\s*\}\}/g, (all, key: string) => values[key] ?? all);
}

/** A label for the action in menus: "Create Forge task", or "Tasks: Save" when the title alone is vague. */
export function actionLabel({ ext, action }: MessageAction): string {
  return action.title.toLowerCase().includes(ext.name.toLowerCase()) ? action.title : `${ext.name}: ${action.title}`;
}

/** The program's answer as a toast, with Open for its link. */
function showAnswer(a: MessageAction, r: ExtensionResult, key?: string) {
  const url = r.url;
  toast({
    kind: url ? "action" : "success",
    message: r.message ?? `${actionLabel(a)}: done`,
    key,
    duration: url ? 10_000 : undefined,
    actions: url ? [{ label: "Open", run: () => void api.openExternal(url) }] : [],
  });
}

/**
 * Run an action on a message. With a form, the form's sheet runs it (and
 * shows any confirm the program asks); without one it runs straight away
 * behind a progress toast, and a confirm opens the sheet.
 */
export async function runExtensionAction(a: MessageAction, m: MessageView): Promise<void> {
  const { ext, action } = a;
  if (action.form.length) {
    openExtensionSheet(a, m, null, (r) => showAnswer(a, r));
    return;
  }
  const key = `ext-${ext.id}-${action.id}`;
  toast({ kind: "progress", message: `${actionLabel(a)}…`, key, duration: null });
  try {
    const r = await api.runExtensionAction({ extensionId: ext.id, actionId: action.id, accountId: m.accountId, messageId: m.id, form: {} });
    if (r.confirm) {
      dismissToastKey(key);
      openExtensionSheet(a, m, { confirm: r.confirm, form: {} }, (done) => showAnswer(a, done));
      return;
    }
    showAnswer(a, r, key);
  } catch (e) {
    const err = asCommandError(e);
    toast({
      kind: "error",
      message: err.message,
      key,
      actions: err.code === "permissionDenied" ? [{ label: "Settings", run: () => openSettings("extensions") }] : [],
    });
  }
}
