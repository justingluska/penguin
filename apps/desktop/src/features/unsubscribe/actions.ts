// Running an unsubscribe: the one-click check, the backend call, the toast
// and its follow-ups (archive this conversation, auto-archive the sender).
// Every path starts from a user click or key and the confirm dialog
// (UnsubscribeDialog); nothing here runs on its own.
import { api, asCommandError } from "../../lib/api";
import type { MessageView, ThreadRef, UnsubscribeOffer, UnsubscribeOutcome } from "../../lib/types";
import { getUi } from "../../lib/ui";
import { currentSettings } from "../../lib/settings";
import { toast, type ToastAction } from "../../components/Toast";
import { archive } from "../../app/actions";
import { accountById, cachedThread, replaceCachedMessage } from "../../app/store";
import { openComposeFilled } from "../compose";
import { senderCondition, senderLabel, unsubscribeCopy, type UnsubscribeCopy } from "./model";
import { askUnsubscribe } from "./UnsubscribeDialog";

const refOf = (m: MessageView): ThreadRef => ({ accountId: m.accountId, threadId: m.threadId });

/** The offer to confirm: first asks the backend whether one-click is available when that's unknown. */
export async function checkedOffer(m: MessageView, offer: UnsubscribeOffer): Promise<UnsubscribeOffer> {
  if (!offer.needsCheck) return offer;
  try {
    const fresh = await api.unsubscribeCheck(m.accountId, m.id);
    if (!fresh) return offer;
    patchThread(m, fresh);
    return fresh;
  } catch (e) {
    // Offline: confirm what's known (the page or the email) instead.
    toast({ kind: "info", message: "Couldn't check for one-click unsubscribe", detail: asCommandError(e).message });
    return { ...offer, needsCheck: false };
  }
}

/** Every cached message from this sender in the thread gets the new offer state. */
function patchThread(m: MessageView, offer: UnsubscribeOffer) {
  const ref = refOf(m);
  const t = cachedThread(ref);
  const sender = m.from.email.toLowerCase();
  for (const x of t?.messages ?? [m]) {
    if (x.id === m.id) replaceCachedMessage(ref, { ...x, unsubscribe: offer });
    else if (x.unsubscribe && x.from.email.toLowerCase() === sender)
      replaceCachedMessage(ref, { ...x, unsubscribe: { ...x.unsubscribe, unsubscribed: offer.unsubscribed } });
  }
}

/** Open the mailto in the composer ("Edit first", or an unverified sender). */
export function editFirst(m: MessageView, offer: UnsubscribeOffer) {
  if (!offer.mailto) return;
  void openComposeFilled({
    accountId: m.accountId,
    to: { name: null, email: offer.mailto.to },
    subject: offer.mailto.subject,
    body: offer.mailto.body,
  });
}

/** Run the confirmed action. Resolves true when it went through. */
export async function runUnsubscribe(m: MessageView, offer: UnsubscribeOffer, action: UnsubscribeCopy["action"]): Promise<boolean> {
  if (action === "compose") {
    editFirst(m, offer);
    return false;
  }
  const sender = senderLabel(m.from);
  const pending = action === "oneClick" ? toast({ kind: "progress", message: `Unsubscribing from ${sender}…`, key: `unsub:${m.id}` }) : null;
  let out: UnsubscribeOutcome;
  try {
    out = await api.unsubscribe(m.accountId, m.id, action);
  } catch (e) {
    const err = asCommandError(e);
    const actions: ToastAction[] =
      action === "oneClick" ? [{ label: "Open page instead", run: () => void runUnsubscribe(m, offer, "link") }] : [];
    toast({ kind: "error", key: pending !== null ? `unsub:${m.id}` : undefined, message: `Couldn't unsubscribe from ${sender}`, detail: err.message, actions });
    return false;
  }
  patchThread(m, { ...offer, needsCheck: false, unsubscribed: out.record });
  const followUps = followUpActions(m);
  const message =
    out.method === "oneClick"
      ? `Unsubscribed from ${sender}`
      : out.method === "mailto"
        ? `Unsubscribe email sent to ${out.sentTo ?? out.domain}`
        : `Opened ${out.domain} to finish unsubscribing`;
  toast({
    kind: followUps.length ? "action" : "success",
    key: pending !== null ? `unsub:${m.id}` : undefined,
    message,
    detail: out.method === "link" ? undefined : "It can take the sender a few days to stop.",
    actions: followUps,
    duration: followUps.length ? 8000 : undefined,
  });
  return true;
}

/** "Archive" this conversation (while it's in the inbox) and "Auto-archive" the sender (a rule, also run on what's there now). */
function followUpActions(m: MessageView): ToastAction[] {
  const ref = refOf(m);
  const inInbox = (cachedThread(ref)?.labelIds ?? m.labelIds).includes("INBOX");
  const out: ToastAction[] = [];
  if (inInbox) out.push({ label: "Archive", run: () => archive([ref]) });
  out.push({ label: "Auto-archive sender", run: () => void autoArchive(m) });
  return out;
}

async function autoArchive(m: MessageView) {
  const email = m.from.email.toLowerCase();
  try {
    const rule = await api.saveRule({
      id: null,
      name: `Archive mail from ${email}`,
      enabled: true,
      dryRun: false,
      accountIds: [m.accountId],
      profileId: null,
      trigger: { kind: "newMessage" },
      condition: senderCondition(email),
      actions: [{ kind: "archive" }],
      stopProcessing: false,
      includeBody: false,
    });
    const report = await api.runRule(rule.id, false);
    toast({
      kind: "success",
      message: report.acted > 0 ? `Archived ${report.acted} from ${email}` : `Mail from ${email} will skip the inbox`,
      detail: "A rule in Settings → Rules keeps doing this.",
    });
  } catch (e) {
    toast({ kind: "error", message: "Couldn't create the rule", detail: asCommandError(e).message });
  }
}

/**
 * The message the ⌘U key, ⌘K and the message menu act on in the open
 * conversation: the newest one with an offer.
 */
export function currentUnsubscribeTarget(): MessageView | null {
  const ui = getUi();
  if (!currentSettings().unsubscribeButton || (ui.surface !== "mail" && !ui.threadOpen)) return null;
  const ref = ui.selected;
  const t = ref ? cachedThread(ref) : undefined;
  if (!t) return null;
  for (let i = t.messages.length - 1; i >= 0; i--) if (t.messages[i].unsubscribe && !t.messages[i].bodyPending) return t.messages[i];
  return null;
}

/** Show the confirm dialog for `offer`, then run what was picked. Resolves when that's done. */
export async function confirmUnsubscribe(m: MessageView, offer: UnsubscribeOffer, onRun?: () => void): Promise<void> {
  const copy = unsubscribeCopy(offer, senderLabel(m.from), accountById(m.accountId)?.email);
  const choice = await askUnsubscribe(offer, copy);
  if (choice === "confirm") {
    onRun?.();
    await runUnsubscribe(m, offer, copy.action);
  } else if (choice === "editFirst") editFirst(m, offer);
}

/** Unsubscribe from the menu item, ⌘U or ⌘K (the privacy-row button does the same with its own busy state). */
export async function startUnsubscribe(m: MessageView | null = currentUnsubscribeTarget()) {
  if (!m?.unsubscribe) return;
  await confirmUnsubscribe(m, await checkedOffer(m, m.unsubscribe));
}
