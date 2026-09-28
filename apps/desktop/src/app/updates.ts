// Self-update status as toasts (src-tauri/src/updater.rs does the work).
// Penguin → Check for Updates… reports every step; the automatic checks
// only speak up once a new version is installed and needs a restart.
import { api, asCommandError, onUpdate } from "../lib/api";
import type { UpdateEvent } from "../lib/types";
import { dismissToastKey, toast } from "../components/Toast";

const KEY = "app:update";

function restart() {
  api.restartToUpdate().catch((e) => toast({ tone: "error", message: `Couldn't restart: ${asCommandError(e).message}` }));
}

function show(e: UpdateEvent) {
  switch (e.state) {
    case "checking":
      return toast({ key: KEY, kind: "progress", message: "Checking for updates…", duration: null });
    case "downloading":
      return toast({ key: KEY, kind: "progress", message: `Downloading Penguin ${e.version ?? ""}…`, duration: null });
    case "upToDate":
      return toast({ key: KEY, message: `Penguin is up to date (${e.current})` });
    case "error":
      return toast({ key: KEY, tone: "error", message: "Couldn't check for updates", detail: e.message ?? undefined });
    case "unconfigured":
      return toast({
        key: KEY,
        message: "Updates aren't set up for this build",
        detail: "It was built from source without an update channel. Pull and rebuild to update.",
      });
    case "ready":
      return toast({
        key: KEY,
        title: `Penguin ${e.version ?? ""} is ready`,
        message: summary(e.notes) ?? "Restart to finish updating. Drafts are saved.",
        action: { label: "Restart", run: restart },
        duration: null,
      });
  }
}

/** The release notes ("• one\n• two…") as one line of up to three; the rest is in Settings → What's new. */
function summary(notes: string | null): string | null {
  const lines = (notes ?? "")
    .split("\n")
    .map((l) => l.replace(/^•\s*/, "").trim())
    .filter(Boolean);
  if (!lines.length) return null;
  return lines.length > 3 ? [...lines.slice(0, 3), `and ${lines.length - 3} more`].join(" · ") : lines.join(" · ");
}

/** Listen for penguin://update; returns the unlisten. */
export function startUpdateToasts(): () => void {
  const off = onUpdate(show);
  return () => {
    dismissToastKey(KEY);
    void off.then((f) => f());
  };
}
