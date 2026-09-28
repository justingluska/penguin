// Entering and leaving demo mode (Settings → Developer, ⌘K). The switch saves
// open drafts, persists the per-device flag and reloads the page onto the
// other backend; src/lib/demo.ts takes it from there. See docs/DEMO.md.
import { isDemo } from "../lib/api";
import { realStorage } from "../lib/demo";
import { switchDemoMode } from "../lib/demoFlag";
import { toast } from "../components/Toast";
import { sendPending } from "../features/compose/send";
import { flushAllSavers } from "../features/compose/autosave";

export { isDemo };

export async function setDemoMode(on: boolean): Promise<void> {
  try {
    const r = await switchDemoMode(on, isDemo, {
      storage: realStorage,
      sendPending,
      flushDrafts: flushAllSavers,
      reload: () => location.reload(),
    });
    if (r === "send-pending") {
      toast({ message: "A message is still sending", detail: "Try again once it has gone, or undo it first." });
    }
  } catch (e) {
    toast({ tone: "error", message: `Couldn't switch demo mode: ${e instanceof Error ? e.message : String(e)}` });
  }
}
