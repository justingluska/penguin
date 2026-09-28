// Dev-only URL params so overlays can be opened directly for screenshots:
//   ?overlay=search&q=from:mike%20lease   ?overlay=command&q=sn   ?overlay=compose&mode=reply
//   &theme=light|dark   &profile=<profile id>   &switcher=1 (account switcher open)
//   &settings=<section> (Settings open at that section, e.g. profiles)
// Ignored in production builds.
import { setUi, type Overlay } from "../../lib/ui";
import { openSettings, type SettingsSection } from "../settings/state";

let applied = false;

export function devParam(name: string): string | null {
  if (!import.meta.env.DEV) return null;
  try {
    return new URLSearchParams(window.location.search).get(name);
  } catch {
    return null;
  }
}

export function applyDevParams() {
  if (applied || !import.meta.env.DEV) return;
  applied = true;
  const overlay = devParam("overlay");
  const theme = devParam("theme");
  const patch: Parameters<typeof setUi>[0] = {};
  if (theme === "light" || theme === "dark") patch.theme = theme;
  const profile = devParam("profile");
  if (profile) patch.profileId = profile;
  const settings = devParam("settings");
  if (settings) openSettings(settings as SettingsSection);
  if (overlay === "search" || overlay === "command" || overlay === "compose") {
    patch.overlay = overlay as Overlay;
    if (overlay === "compose") {
      const mode = devParam("mode");
      patch.composeContext =
        mode === "reply" || mode === "replyAll" || mode === "forward"
          ? { mode, thread: { accountId: devParam("account") ?? "acc-personal", threadId: devParam("thread") ?? "t-lease-renewal" } }
          : { mode: "new" };
    }
  }
  if (Object.keys(patch).length) setUi(patch);
}
