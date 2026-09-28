// Settings → AI → Thread summaries: the switch, and (once, here) why
// summaries can't run on this Mac when they can't. The thread view just hides
// the action in that case. OWNER: summaries.
import { useEffect } from "react";
import { toast } from "../../components/Toast";
import { asCommandError } from "../../lib/api";
import { updateSettings, useSetting } from "../../lib/settings";
import { Switch } from "../settings/parts";
import { unavailableText } from "./model";
import { refreshAvailability, useAvailability } from "./state";

export function SummarySettings() {
  const on = useSetting("summaries");
  const availability = useAvailability();
  // Apple Intelligence may have been switched on since: ask again here.
  useEffect(() => void refreshAvailability(true), []);
  const available = availability?.available === true;
  const note =
    availability === null
      ? "Checking Apple Intelligence on this Mac…"
      : available
        ? "Summarize a conversation with Apple Intelligence, on this Mac: nothing is sent anywhere. Summarize in the thread toolbar, or ⇧S."
        : unavailableText(availability.reason);
  return (
    <div className="setting-row setting-tall" data-setting="summaries">
      <div>
        <span className="setting-label row-flex">Thread summaries</span>
        <p className={"st-muted" + (availability && !available ? " st-warn" : "")}>{note}</p>
      </div>
      <Switch
        label="Thread summaries"
        on={on && available}
        disabled={!available}
        onChange={(summaries) =>
          updateSettings({ summaries }).catch((e) => toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` }))
        }
      />
    </div>
  );
}
