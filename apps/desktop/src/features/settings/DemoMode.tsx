// Settings → Developer → Demo mode: the app on fictional data, for screenshots.
// Shown in demo mode too (it runs on the mock's settings), so it is also the way out.
import { useState } from "react";
import { isDemo, setDemoMode } from "../../app/demoMode";
import { Switch } from "./parts";

export function DemoModeRow() {
  const [busy, setBusy] = useState(false);
  const flip = async (on: boolean) => {
    setBusy(true);
    await setDemoMode(on);
    // Still here: the switch was refused (a send is pending) or failed.
    setBusy(false);
  };
  return (
    <div className="setting-row setting-tall">
      <div>
        <span className="setting-label">Demo mode</span>
        <p className="st-muted">
          {isDemo
            ? "Showing fictional accounts and mail. Nothing here is real, and nothing you do is sent or saved. Turn off to go back to your mail."
            : "Show fictional accounts and mail everywhere, for screenshots. Your accounts and mail are untouched, and nothing you do in demo mode is sent. Penguin reloads to switch."}
        </p>
      </div>
      <Switch label="Demo mode" on={isDemo} disabled={busy} onChange={(on) => void flip(on)} />
    </div>
  );
}
