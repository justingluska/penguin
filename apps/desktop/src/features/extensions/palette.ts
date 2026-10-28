// ⌘K entries for the allowed extensions' actions, run on the selected
// conversation's latest message (the one Reply answers).
import { getUi } from "../../lib/ui";
import { cachedThread } from "../../app/store";
import { isIconName, type IconName } from "../../components/Icon";
import { pickMessage } from "../compose/quote";
import { actionLabel, allowedActions, runExtensionAction } from "./state";

export interface ExtensionCommand {
  id: string;
  group: string;
  label: string;
  meta?: string;
  icon: IconName;
  alias: string;
  run: () => void;
}

export function extensionCommands(): ExtensionCommand[] {
  const ui = getUi();
  if (!ui.selected || !(ui.surface === "mail" || ui.threadOpen)) return [];
  const thread = cachedThread(ui.selected);
  const m = thread && pickMessage(thread.messages, undefined);
  if (!m) return [];
  return allowedActions().map((a) => ({
    id: `ext:${a.ext.id}/${a.action.id}`,
    group: "Extensions",
    label: actionLabel(a),
    meta: a.ext.name,
    icon: isIconName(a.action.icon) ? a.action.icon : "zap",
    alias: `extension ${a.ext.name} ${a.ext.description}`,
    run: () => void runExtensionAction(a, m),
  }));
}
