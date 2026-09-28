// ⌘K entries for the Split Inbox and Get to zero (features/command adds them
// to the palette): go to a split, add the selected conversation's sender or
// their company to a split, turn the Split Inbox on or off, edit the splits,
// and Get to zero.
import type { Address } from "../../lib/types";
import type { IconName } from "../../components/Icon";
import { currentSettings, updateSettings } from "../../lib/settings";
import { getUi } from "../../lib/ui";
import { isConsumerAddress } from "../../lib/format";
import { cachedThread, currentSplit, isMe, listSummary } from "../../app/store";
import { openSettings } from "../settings/state";
import { canGetToZero, openGetToZero } from "../zero/GetToZero";
import { addToSplit, setSplit, shownTabs } from "./state";

export interface PaletteCmd {
  id: string;
  group: string;
  label: string;
  meta?: string;
  keys?: string;
  icon?: IconName;
  alias?: string;
  run: () => void;
}

/** Who the selected conversation is from: its latest message not from you. */
function selectedSender(): Address | null {
  const sel = getUi().selected;
  if (!sel) return null;
  const t = cachedThread(sel);
  const fromThread = t && [...t.messages].reverse().find((m) => !isMe(m.from.email))?.from;
  if (fromThread) return fromThread;
  return listSummary(sel)?.participants.find((p) => !isMe(p.email)) ?? null;
}

export function splitPaletteCommands(): PaletteCmd[] {
  const s = currentSettings();
  const out: PaletteCmd[] = [];
  const G = "Split Inbox";
  if (s.inboxTabs) {
    const here = currentSplit();
    shownTabs().forEach((t, i) => {
      if (t.id === here) return;
      out.push({
        id: `split:go:${t.id}`,
        group: G,
        label: `Go to ${t.name}`,
        meta: t.query ?? "Everything no split claims",
        keys: i < 9 ? String(i + 1) : undefined,
        icon: "inbox",
        alias: "split tab",
        run: () => setSplit(t.id),
      });
    });
  }
  const who = selectedSender();
  if (who && s.inboxSplits.length) {
    const name = who.name?.trim() || who.email;
    const domain = who.email.split("@")[1]?.toLowerCase();
    for (const sp of s.inboxSplits) {
      out.push({
        id: `split:add:${sp.id}`,
        group: G,
        label: `Add ${name} to ${sp.name}`,
        meta: `from:${who.email.toLowerCase()}`,
        icon: "plus",
        alias: "add sender to a split vip",
        run: () => addToSplit(sp.id, `from:${who.email.toLowerCase()}`),
      });
      if (domain && !isConsumerAddress(who.email)) {
        out.push({
          id: `split:adddomain:${sp.id}`,
          group: G,
          label: `Add everyone at ${domain} to ${sp.name}`,
          meta: `from:@${domain}`,
          icon: "users",
          alias: "add domain company to a split team",
          run: () => addToSplit(sp.id, `from:@${domain}`),
        });
      }
    }
  }
  out.push({
    id: "split:toggle",
    group: G,
    label: s.inboxTabs ? "Turn off Split Inbox" : "Turn on Split Inbox",
    meta: s.inboxTabs ? "One inbox list" : s.inboxSplits.map((x) => x.name).concat("Other").join(" · "),
    icon: "columns",
    alias: "split inbox tabs important other",
    run: () => void updateSettings({ inboxTabs: !s.inboxTabs }),
  });
  out.push({
    id: "split:edit",
    group: G,
    label: "Edit splits…",
    meta: "Add VIP, Team, News, or any search",
    icon: "settings",
    alias: "split inbox settings vip team add a split",
    run: () => openSettings("inbox"),
  });
  if (canGetToZero()) {
    out.push({
      id: "zero:open",
      group: "Triage",
      label: "Get to zero…",
      meta: here() ? `Archive older conversations in ${here()}` : "Archive older conversations in the inbox",
      icon: "done",
      alias: "inbox zero get me to zero archive old bulk archive mark all done older than clean up",
      run: openGetToZero,
    });
  }
  return out;
}

function here(): string | null {
  const id = currentSplit();
  return id === null ? null : shownTabs().find((t) => t.id === id)?.name ?? null;
}
