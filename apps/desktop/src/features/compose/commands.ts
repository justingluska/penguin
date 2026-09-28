// The composer's keys in the shortcut registry (so the "?" sheet lists them)
// and its commands in the ⌘K palette.
//
// While composing, the keys route to the open composer through `composerBus`
// (index.tsx sets it). ⌘K is the link editor inside the composer, so the
// palette's compose commands are the ones that start one: reply with an
// instant reply, reply with AI, manage snippets.
import { isMac, registerShortcuts, type Shortcut } from "../../lib/keyboard";
import { getUi } from "../../lib/ui";
import { currentSettings } from "../../lib/settings";
import { openCompose } from "../../app/actions";
import { openSettings } from "../settings/state";
import type { IconName } from "../../components/Icon";
import { instantChoices, setComposeIntent } from "./instant";

/** What the open composer can be asked to do from a registered key. */
export interface ComposerBus {
  snippets(): void;
  writeWithAi(): void;
  /** Whether Write with AI can run now (on, and Apple Intelligence available). */
  writerReady(): boolean;
  instant(n: number): void;
  send(): void;
  sendLater(): void;
}

export let composerBus: ComposerBus | null = null;

export function setComposerBus(bus: ComposerBus | null) {
  composerBus = bus;
}

const composing = () => getUi().overlay === "compose" && composerBus !== null;

const INSTANT_KEYS = [1, 2, 3, 4, 5, 6, 7, 8, 9];

let registered = false;

/** Once, from the Compose root (mounted for the app's lifetime). */
export function registerComposeShortcuts(): () => void {
  if (registered) return () => {};
  registered = true;
  const list: Shortcut[] = [
    { id: "compose.send", keys: "mod+enter", label: "Send", group: "Compose", allowInInput: true, when: composing, run: () => composerBus?.send() },
    { id: "compose.sendLater", keys: "mod+shift+enter", label: "Send later…", group: "Compose", allowInInput: true, when: composing, run: () => composerBus?.sendLater() },
    { id: "compose.snippets", keys: "mod+;", label: "Insert snippet", group: "Compose", allowInInput: true, when: composing, run: () => composerBus?.snippets() },
    {
      id: "compose.writeAi",
      keys: "mod+shift+j",
      label: "Write with AI",
      group: "Compose",
      allowInInput: true,
      when: () => composing() && !!composerBus?.writerReady(),
      run: () => composerBus?.writeWithAi(),
    },
    ...INSTANT_KEYS.map(
      (n): Shortcut => ({
        id: `compose.instant.${n}`,
        // ⌃ on the Mac (⌘1… are the mailboxes, ⌥1… the From account). The
        // same keys switch profiles, but only outside the composer.
        keys: `${isMac ? "ctrl" : "mod"}+${n}`,
        label: n === 1 ? "Use an instant reply (⌃1–⌃9)" : `Use instant reply ${n}`,
        group: "Compose",
        // One row in the sheet stands for all nine.
        hidden: n > 1,
        allowInInput: true,
        when: composing,
        run: () => composerBus?.instant(n),
      }),
    ),
  ];
  const off = registerShortcuts(list);
  return () => {
    registered = false;
    off();
  };
}

/** A palette entry (features/command). */
export interface ComposeCommand {
  id: string;
  group: string;
  label: string;
  meta?: string;
  icon?: IconName;
  alias?: string;
  run: () => void;
}

/** A conversation is under the cursor (list or open thread), so there's something to reply to. */
function canReply(): boolean {
  const ui = getUi();
  return ui.selected !== null && (ui.surface === "mail" || ui.threadOpen);
}

/**
 * ⌘K entries: an instant reply to the conversation, a reply drafted with AI
 * (`writerReady`: on, and this Mac can run it), and where to manage both.
 */
export function composeCommands(writerReady: boolean): ComposeCommand[] {
  const out: ComposeCommand[] = [];
  const s = currentSettings();
  if (canReply()) {
    if (writerReady) {
      out.push({
        id: "cmp:replyAi",
        group: "Compose",
        label: "Reply with AI…",
        meta: "Apple Intelligence, on this Mac",
        icon: "sparkles",
        alias: "write draft generate compose ai assistant help me write",
        run: () => {
          setComposeIntent({ ai: true });
          openCompose("reply");
        },
      });
    }
    for (const c of instantChoices({ ...s.instantReplies, aiSuggestions: false })) {
      out.push({
        id: `cmp:instant:${c.n}`,
        group: "Compose",
        label: `Reply “${c.text}”`,
        meta: "Instant reply",
        icon: "reply",
        alias: "instant quick reply canned response one-liner",
        run: () => {
          setComposeIntent({ text: c.text });
          openCompose("reply");
        },
      });
    }
  }
  out.push({
    id: "cmp:snippets",
    group: "Compose",
    label: "Manage snippets…",
    meta: "Reusable text, typed with ; or ⌘;",
    icon: "zap",
    alias: "templates canned responses text expansion settings",
    run: () => openSettings("compose"),
  });
  out.push({
    id: "cmp:instantSettings",
    group: "Compose",
    label: "Manage instant replies…",
    meta: "One-liners offered when you reply",
    icon: "settings",
    alias: "quick replies canned one-liners settings",
    run: () => openSettings("compose"),
  });
  return out;
}
