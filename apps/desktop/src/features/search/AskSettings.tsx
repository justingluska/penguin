// Settings → AI → "Read questions with Apple Intelligence": when Ask's grammar
// can't read a question exactly, Apple's on-device model turns it into a
// query (subject, dates, what to count) that Penguin checks and answers from
// local mail. The model never writes the answer. OWNER: ask agent.
import { useEffect } from "react";
import { toast } from "../../components/Toast";
import { asCommandError } from "../../lib/api";
import { updateSettings, useSetting } from "../../lib/settings";
import { Switch } from "../settings/parts";
import { unavailableText } from "../summary/model";
import { refreshAvailability, useAvailability } from "../summary/state";

export const ASK_AI_LABEL = "Read questions with Apple Intelligence";

export function AskAiSettings() {
  const on = useSetting("askWithAi");
  const availability = useAvailability();
  useEffect(() => void refreshAvailability(true), []);
  const available = availability?.available === true;
  const note =
    availability === null
      ? "Checking Apple Intelligence on this Mac…"
      : available
        ? "When Ask can't read a question itself, the on-device model turns it into a query. Penguin checks it and counts the answer from your mail; the model never writes the answer and nothing is sent anywhere."
        : unavailableText(availability.reason);
  return (
    <div className="setting-row setting-tall" data-setting="askWithAi">
      <div>
        <span className="setting-label row-flex">{ASK_AI_LABEL}</span>
        <p className={"st-muted" + (availability && !available ? " st-warn" : "")}>{note}</p>
      </div>
      <Switch
        label={ASK_AI_LABEL}
        on={on && available}
        disabled={!available}
        onChange={(askWithAi) =>
          updateSettings({ askWithAi }).catch((e) => toast({ tone: "error", message: `Couldn't save settings: ${asCommandError(e).message}` }))
        }
      />
    </div>
  );
}
