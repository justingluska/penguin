// OWNER: onboarding agent. App renders this instead of the mail UI when the
// OAuth client is not configured or there are no accounts.
//
// First-run setup: a progress rail on the left, one step at a time on the
// right, Back/Next (↵ / ⇧↵). It starts with "Enter your email"; detection
// (detect_provider) picks the provider's steps: the Google Cloud guide and
// sign-in, the Microsoft Entra registration, app-password instructions,
// server settings, or "not supported yet" (flow.tsx). Progress is remembered
// in localStorage so reopening resumes where the user left off.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api, asCommandError, isMock } from "../../lib/api";
import type { Account, DetectedProvider } from "../../lib/types";
import { Icon } from "../../components/Icon";
import { PenguinMark } from "../../components/PenguinMark";
import { useAddFlow, type SavedFlow } from "./flow";
import { WhoChip } from "./ProviderSteps";
import "./onboarding.css";

const STORE_KEY = "penguin.onboarding";

function readSaved(): Partial<SavedFlow> {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    const v = raw ? (JSON.parse(raw) as Partial<SavedFlow> & { v?: number; projectId?: unknown }) : {};
    const projectId = typeof v.projectId === "string" ? v.projectId : undefined;
    // Before the email-first flow (no v): keep only the Cloud project.
    if (v.v !== 2) return { projectId };
    const d = v.detected as DetectedProvider | null | undefined;
    return {
      stepId: typeof v.stepId === "string" ? v.stepId : undefined,
      email: typeof v.email === "string" ? v.email : undefined,
      detected: d && typeof d === "object" && typeof d.email === "string" && typeof d.setup === "string" ? d : null,
      projectId,
      cloudSetup: typeof v.cloudSetup === "boolean" ? v.cloudSetup : undefined,
      googleQuick: v.googleQuick === true,
    };
  } catch {
    return {};
  }
}

function writeSaved(s: SavedFlow | null) {
  try {
    if (s) localStorage.setItem(STORE_KEY, JSON.stringify({ v: 2, ...s }));
    else localStorage.removeItem(STORE_KEY);
  } catch {
    // Private mode or blocked storage: progress just isn't remembered.
  }
}

/** Screenshots and dev only: ?email=<address> detects it on load, ?onb=<n> (1-based) then opens that step. */
function deepLink(): { email: string | null; step: number | null } {
  if (!import.meta.env.DEV && !isMock) return { email: null, step: null };
  const q = new URLSearchParams(location.search);
  const n = Number(q.get("onb"));
  return { email: q.get("email"), step: Number.isInteger(n) && n >= 1 ? n - 1 : null };
}

export function Onboarding({ onDone }: { onDone: () => void }) {
  const [initial] = useState(readSaved);
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [accountsError, setAccountsError] = useState<string | null>(null);
  const flow = useAddFlow({
    surface: "setup",
    accounts: accounts ?? [],
    allAccounts: accounts ?? [],
    onAdded: (a) => setAccounts((list) => [...(list ?? []).filter((x) => x.id !== a.id), a]),
    initial,
  });
  const mainRef = useRef<HTMLElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const loaded = flow.loaded && accounts !== null;
  const loadError = flow.loadError ?? accountsError;
  const { step, steps, index } = flow;

  useEffect(() => {
    api.listAccounts().then(setAccounts, (e) => setAccountsError(asCommandError(e).message));
  }, []);

  // Dev/screenshot deep links.
  const link = useRef(deepLink());
  useEffect(() => {
    if (!loaded || !link.current.email) return;
    const email = link.current.email;
    link.current.email = null;
    void flow.detect(undefined, email);
  }, [loaded, flow]);
  useEffect(() => {
    const n = link.current.step;
    if (!loaded || n === null || link.current.email || (!flow.detected && n > 0)) return;
    link.current.step = null;
    if (n < steps.length) flow.jump(n);
  }, [loaded, flow, steps.length]);

  const savedKey = JSON.stringify(flow.saved);
  useEffect(() => {
    if (loaded) writeSaved(JSON.parse(savedKey) as SavedFlow);
  }, [loaded, savedKey]);

  const finish = () => {
    writeSaved(null);
    onDone();
  };
  const next = () => {
    if (flow.next()) finish();
  };

  // New step: start at the top, and move focus to its heading for screen
  // readers (without showing a focus ring). The email step keeps its field focused.
  useLayoutEffect(() => {
    mainRef.current?.scrollTo({ top: 0 });
    if (loaded && step.id !== "email") headingRef.current?.focus({ preventScroll: true });
  }, [step.id, loaded]);

  // ↵ next, ⇧↵ back, unless focus is on something Enter already means
  // something for (buttons, links, text areas, disclosures, forms).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Enter" || e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || e.isComposing) return;
      const t = e.target as HTMLElement | null;
      if (t?.closest("button, a, textarea, summary, select, form, [role=dialog]")) return;
      e.preventDefault();
      if (e.shiftKey) flow.back();
      else next();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const importIdx = steps.findIndex((s) => s.id === "import");
  const showImportShortcut = loaded && importIdx > 0 && index > 1 && index < importIdx;

  return (
    <div className="onb s2-shell">
      <header className="onb-top" data-tauri-drag-region>
        <span className="onb-brand">
          <PenguinMark />
          <span>Penguin</span>
          <span className="onb-brand-sub">Setup</span>
        </span>
        <span className="grow" />
        {showImportShortcut && (
          <button className="btn btn-ghost btn-sm" onClick={() => flow.go(importIdx)}>
            <Icon name="file" size="xs" />I already have a client JSON
          </button>
        )}
      </header>

      <div className="onb-body">
        <nav className="onb-rail" aria-label="Setup steps">
          <ol>
            {steps.map((s, i) => {
              const current = i === index;
              const done = s.id === "email" ? !!flow.detected && index > 0 : !!s.done || (i < index && !s.blocked && s.id !== "done");
              const enabled = loaded && flow.canEnter(i);
              const groupStart = i === 0 || steps[i - 1].group !== s.group;
              return (
                <li key={s.id}>
                  {groupStart && s.group !== "Start" && <div className="onb-rail-group">{s.group}</div>}
                  <button
                    className={`onb-rail-step${current ? " is-current" : ""}${done ? " is-done" : ""}`}
                    onClick={() => flow.go(i)}
                    disabled={!enabled}
                    aria-current={current ? "step" : undefined}
                  >
                    <span className="onb-rail-mark">{done ? <Icon name="check" size="2xs" /> : i + 1}</span>
                    <span className="onb-rail-label">{s.label}</span>
                    {s.time && <span className="onb-rail-time">{s.time}</span>}
                  </button>
                </li>
              );
            })}
          </ol>
          <p className="onb-rail-foot">
            <Icon name="shield" size="xs" />
            No Penguin servers. Passwords and tokens stay in your Mac's Keychain.
          </p>
        </nav>

        <main className="onb-main" ref={mainRef}>
          {loadError ? (
            <div className="onb-panel">
              <div className="onb-error" role="alert">
                <Icon name="info" size="xs" />
                <span>Couldn't read Penguin's setup state: {loadError}</span>
              </div>
            </div>
          ) : loaded ? (
            <div className={`onb-panel${step.wide ? " is-wide" : ""}`} key={step.id}>
              {step.id === "email" ? (
                <div className="onb-hero">
                  <PenguinMark className="logo onb-hero-logo" />
                </div>
              ) : (
                <div className="onb-eyebrow">
                  Step {index + 1} of {steps.length}
                  {step.time && <> · {step.time === "optional" ? "optional" : `about ${step.time}`}</>}
                </div>
              )}
              <h1 className="onb-title" ref={headingRef} tabIndex={-1}>
                {step.heading}
              </h1>
              {step.lead && <p className="onb-lead">{step.lead}</p>}
              {step.chip && flow.detected && <WhoChip detected={flow.detected} onChange={flow.restart} />}
              <div className="onb-content">{step.content}</div>
            </div>
          ) : null}
        </main>
      </div>

      <footer className="onb-foot">
        <button className="btn btn-ghost" onClick={flow.back} disabled={!loaded || index === 0}>
          <Icon name="left" size="sm" />
          Back
        </button>
        <span className="onb-foot-hints">
          <span className="hint">
            <span className="kbd">↵</span>Next
          </span>
          <span className="hint">
            <span className="kbd">⇧</span>
            <span className="kbd">↵</span>Back
          </span>
        </span>
        <span className="grow" />
        {step.blocked && step.blockedHint && <span className="s2-muted onb-foot-why">{step.blockedHint}</span>}
        <button className="btn btn-primary" onClick={next} disabled={!loaded || !!step.blocked}>
          {step.nextLabel ?? "Next"}
          <span className="kbd">↵</span>
        </button>
      </footer>
    </div>
  );
}
