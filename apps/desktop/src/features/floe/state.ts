// Floe mode: the single-surface layout. One calm, full-width
// list; Enter opens a focused reading column; Esc returns with the cursor
// kept, and Esc again leaves Floe. The sidebar, reading pane and status bar
// slide away rather than being swapped out (morph.ts): views and labels stay
// one ⌘K away and every shortcut still works.
// OWNER: floe.
//
// The mode is the `floeMode` setting itself, so the app reopens in whichever
// mode was last used ("Start in Floe mode" in Settings → Inbox is the same
// switch). updateSettings is optimistic, so toggling is instant.
import { registerShortcuts } from "../../lib/keyboard";
import { currentSettings, subscribeSettings, updateSettings, useSetting } from "../../lib/settings";
import { getUi, setUi } from "../../lib/ui";
import { toast } from "../../components/Toast";
import { asCommandError } from "../../lib/api";

export function isFloe(): boolean {
  return currentSettings().floeMode;
}

export function useFloe(): boolean {
  return useSetting("floeMode");
}

export function toggleFloe() {
  updateSettings({ floeMode: !isFloe() }).catch((e) =>
    toast({ tone: "error", message: `Couldn't switch mode: ${asCommandError(e).message}` }),
  );
}

/**
 * The context panel is on demand in Floe (i summons it), persistent in the
 * full layout: entering Floe hides it and leaving restores what it was.
 */
function trackContextPanel(): () => void {
  let was = isFloe();
  let savedPanel = getUi().contextPanel;
  if (was) setUi({ contextPanel: false });
  return subscribeSettings(() => {
    const now = isFloe();
    if (now === was) return;
    was = now;
    if (now) {
      savedPanel = getUi().contextPanel;
      setUi({ contextPanel: false });
    } else {
      setUi({ contextPanel: savedPanel });
    }
  });
}

const noOverlay = () => {
  const o = getUi().overlay;
  return o === null || o === "command";
};

/**
 * ⌘⇧F (and \) toggle Floe mode; Esc leaves it. Esc is a fallback: popovers,
 * a multi-selection and an open thread each take Esc first, so it leaves
 * Floe only from the plain list. Registered once by App.
 */
export function installFloe(): () => void {
  const offPanel = trackContextPanel();
  const offKeys = registerShortcuts([
    { id: "floe.toggle", keys: "mod+shift+f", label: "Floe mode (single column)", group: "App", when: noOverlay, run: toggleFloe },
    { id: "floe.toggle.backslash", keys: "\\", label: "Floe mode (single column)", group: "App", hidden: true, when: noOverlay, run: toggleFloe },
    { id: "floe.leave", keys: "escape", label: "Leave Floe mode", group: "App", fallback: true, when: () => isFloe() && noOverlay(), run: toggleFloe },
  ]);
  return () => {
    offPanel();
    offKeys();
  };
}
