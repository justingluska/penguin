// The Welcome setup's host: mounted once by App while the mail UI is up. It
// opens the setup by itself on a new install (welcomeCompleted false, an
// account added) and loads the screens only when they're shown.
import { useEffect, useRef } from "react";
import { getSettings } from "../../lib/settings";
import { lazyScreen } from "../../lib/lazy";
import { meta } from "../../app/store";
import { openWelcome, useWelcomeOpen } from "./state";
import { shouldAutoOpen } from "./model";

const Welcome = lazyScreen(() => import("./Welcome").then((m) => m.Welcome), { prefetch: false });

export { openWelcome } from "./state";

export function WelcomeHost() {
  const open = useWelcomeOpen();
  const accounts = meta.use((m) => m.accounts.length);
  const checked = useRef(false);
  useEffect(() => {
    if (checked.current || accounts === 0) return;
    checked.current = true;
    void getSettings().then((s) => {
      if (shouldAutoOpen(s, accounts)) openWelcome();
    });
  }, [accounts]);
  if (!open) return null;
  return <Welcome />;
}
