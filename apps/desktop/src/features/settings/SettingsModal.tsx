// Mounted once by App; renders only while Settings is open. The screen itself
// (features/settings/index.tsx and every page it imports) is its own chunk:
// see lib/lazy.ts.
import { lazyScreen } from "../../lib/lazy";
import { useSettingsOpen } from "./state";

const SettingsDialog = lazyScreen(() => import("./index").then((m) => m.SettingsDialog));

export function SettingsModal() {
  const open = useSettingsOpen();
  return open ? <SettingsDialog /> : null;
}
