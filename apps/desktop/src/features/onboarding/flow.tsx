// The Add account flow, shared by first-run setup (index.tsx, with a progress
// rail) and the Add account modal (AddAccountModal.tsx): "Enter your email"
// → detect_provider → the steps for that provider → Done. One decision and
// little text per screen; ↵ advances. Adding the account jumps to Done. A manual "Choose provider" pick re-asks detect_provider with
// `choose`, so every preset lives in Rust (src-tauri/src/providers/).
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { api, asCommandError } from "../../lib/api";
import type { Account, DetectedProvider, MicrosoftClientStatus, OAuthClientStatus, PendingSetup, SetupKind } from "../../lib/types";
import { AccountsPanel } from "./Accounts";
import { ClientImport, clientProblems } from "./ClientImport";
import { SheetClientStep } from "./SheetClient";
import { rememberProjectId, rememberedProjectId } from "./links";
import { Troubleshooting } from "./parts";
import { ApiStep, BrandingStep, ClientStep, DoneStep, ProjectStep, PublishStep, WelcomeStep } from "./guide";
import {
  EmailForm,
  MicrosoftClientStep,
  MicrosoftPermissionsStep,
  MicrosoftRegisterStep,
  MicrosoftSignInStep,
  PasswordConnectStep,
  PasswordGuideStep,
  QuickSetupOffer,
  ServerSettingsStep,
  UnsupportedStep,
  addedAccount,
  unsupportedLead,
} from "./ProviderSteps";
import { domainOf, looksLikeEmail, passwordGuide, pathFor, quickSetupAllowed } from "./providers";
import { Icon } from "../../components/Icon";
import { clearPendingSetup, notePendingSetup, usePendingSetups } from "./pending";

export type Surface = "setup" | "modal";

export interface FlowStep {
  id: string;
  /** Rail label. */
  label: string;
  group: string;
  heading: string;
  lead?: ReactNode;
  time?: string;
  /** Room for the browser sketches. */
  wide?: boolean;
  /** Can't move past it yet; `blockedHint` says why. */
  blocked?: boolean;
  blockedHint?: string;
  nextLabel?: string;
  /** Rail check mark. */
  done?: boolean;
  /** Show who is being added above the content. */
  chip?: boolean;
  content: ReactNode;
}

export interface SavedFlow {
  stepId: string;
  email: string;
  detected: DetectedProvider | null;
  projectId: string;
  /** The Google Cloud guide is part of this path (no client when it started). */
  cloudSetup: boolean;
  /** Google path: Gmail quick setup (app password over IMAP) instead of the Cloud client. */
  googleQuick?: boolean;
}

export interface AddFlowOptions {
  surface: Surface;
  /** What counts as added: every account in setup, this session's in the modal. */
  accounts: Account[];
  allAccounts: Account[];
  onAdded: (a: Account) => void;
  initial?: Partial<SavedFlow>;
  /** An unfinished setup to pick up where it stopped (Settings → Accounts → Resume). */
  resume?: PendingSetup | null;
}

export function useAddFlow({ surface, accounts, allAccounts, onAdded: added, initial, resume }: AddFlowOptions) {
  const [client, setClient] = useState<OAuthClientStatus | null>(null);
  const [ms, setMs] = useState<MicrosoftClientStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [email, setEmailRaw] = useState(initial?.email ?? "");
  const [detected, setDetected] = useState<DetectedProvider | null>(initial?.detected ?? null);
  const [detecting, setDetecting] = useState<string | null>(null);
  const [detectError, setDetectError] = useState<string | null>(null);
  const [stepId, setStepId] = useState(initial?.stepId ?? "email");
  const [cloudSetup, setCloudSetup] = useState(initial?.cloudSetup ?? true);
  const [googleQuick, setGoogleQuick] = useState(initial?.googleQuick ?? false);
  const [projectId, setProjectId] = useState(() => initial?.projectId || rememberedProjectId());
  /** A provider button pressed before the address was typed: used when it is. */
  const [chosen, setChosen] = useState<SetupKind | null>(null);
  /** The account this flow just added (Done shows it). */
  const [lastAdded, setLastAdded] = useState<Account | null>(null);
  const request = useRef(0);
  const pending = usePendingSetups();

  // An added account's setup is finished: clear it and show Done.
  const onAdded = useCallback(
    (a: Account) => {
      clearPendingSetup(a.email);
      setLastAdded(a);
      setStepId("done");
      added(a);
    },
    [added],
  );

  useEffect(() => {
    Promise.all([api.oauthClientStatus(), api.microsoftClientStatus().catch(() => null)])
      .then(([c, m]) => {
        setClient(c);
        setMs(m);
      })
      .catch((e) => setLoadError(asCommandError(e).message));
  }, []);

  useEffect(() => rememberProjectId(projectId), [projectId]);

  const setEmail = useCallback((s: string) => {
    setEmailRaw(s);
    setDetectError(null);
    // A different address makes the old answer stale.
    setDetected((d) => (d && d.email.toLowerCase() !== s.trim().toLowerCase() ? null : d));
  }, []);

  const enter = useCallback(
    (d: DetectedProvider, step?: string) => {
      const cloud = !client?.configured;
      setCloudSetup(cloud);
      setGoogleQuick(false);
      setDetected(d);
      setEmailRaw(d.email);
      setChosen(null);
      setLastAdded(null);
      // A resumed step that isn't on this path falls back to the start (see `index` below).
      setStepId(step ?? startStep(d, cloud, ms));
    },
    [client, ms],
  );

  /** `step`: open this step once detected (resuming an unfinished setup). */
  const detect = useCallback(
    async (choose?: SetupKind, address?: string, step?: string) => {
      const addr = (address ?? email).trim();
      if (!looksLikeEmail(addr)) {
        setDetectError("Type your full email address, like name@example.com.");
        return;
      }
      choose = choose ?? chosen ?? undefined;
      const id = ++request.current;
      setDetecting(domainOf(addr));
      setDetectError(null);
      try {
        const d = await api.detectProvider(addr, choose ?? null);
        if (id === request.current) enter(d, step);
      } catch (e) {
        if (id === request.current) setDetectError(asCommandError(e).message);
      } finally {
        if (id === request.current) setDetecting(null);
      }
    },
    [email, chosen, enter],
  );

  const restart = useCallback(() => {
    request.current++;
    setDetecting(null);
    setStepId("email");
  }, []);

  const loaded = client !== null;

  // Pick up a setup from Settings → Accounts once the clients are known.
  const resumed = useRef(false);
  useEffect(() => {
    if (!loaded || !resume || resumed.current) return;
    resumed.current = true;
    void detect(resume.kind, resume.email, resume.step);
  }, [loaded, resume, detect]);

  // Remember how far this setup got, so leaving doesn't lose it.
  useEffect(() => {
    if (!detected || stepId === "email" || stepId === "done" || detected.setup === "unsupported") return;
    if (lastAdded || allAccounts.some((a) => a.email.toLowerCase() === detected.email.toLowerCase())) return;
    notePendingSetup(detected.email, detected.setup, stepId);
  }, [detected, stepId, allAccounts, lastAdded]);

  const ctx: PathCtx = {
    surface,
    client: client ?? { configured: false, path: "", iosClientId: null },
    setClient,
    ms,
    setMs,
    projectId,
    setProjectId,
    accounts,
    allAccounts,
    onAdded,
    lastAdded,
    cloudSetup,
    googleQuick,
    setGoogleQuick: (on) => {
      setGoogleQuick(on);
      setStepId(on ? "gq-create" : cloudSetup ? "g-intro" : "accounts");
    },
    goTo: setStepId,
    restart,
    choose: (s) => void detect(s, detected?.email),
    retry: () => void detect(undefined, detected?.email),
  };

  const emailStep: FlowStep = {
    id: "email",
    label: "Your email",
    group: "Start",
    // The modal's own title ("Add account") is the heading.
    heading: surface === "setup" ? "Every email, instantly." : "",
    lead: surface === "setup" ? "Fast mail that runs on your Mac. Start with your address." : undefined,
    nextLabel: "Continue",
    blocked: !looksLikeEmail(email) || !!detecting,
    content: (
      <EmailForm
        email={email}
        setEmail={setEmail}
        detecting={detecting}
        error={detectError}
        onSubmit={() => void detect()}
        chosen={chosen}
        onChoose={(s) => {
          if (looksLikeEmail(email)) void detect(s);
          else setChosen((c) => (c === s ? null : s));
        }}
        pending={pending.filter((p) => !allAccounts.some((a) => a.email.toLowerCase() === p.email))}
        onResume={(p) => void detect(p.kind, p.email, p.step)}
        autoFocus
      />
    ),
  };

  const steps: FlowStep[] = [emailStep, ...(detected && loaded ? pathSteps(detected, ctx) : placeholderSteps(surface))];
  let index = steps.findIndex((s) => s.id === stepId);
  if (index < 0 && detected && loaded) index = steps.findIndex((s) => s.id === startStep(detected, cloudSetup, ms));
  if (index < 0) index = 0;

  const canEnter = (i: number) => i >= 0 && i < steps.length && (i === 0 || (!!detected && steps.slice(0, i).every((s, j) => j === 0 || !s.blocked)));
  const go = (i: number) => {
    if (canEnter(i)) setStepId(steps[i].id);
  };
  const isLast = index === steps.length - 1;
  /** ↵ / the primary button. Returns true when the flow is finished. */
  const next = (): boolean => {
    const step = steps[index];
    if (step.id === "email") {
      void detect();
      return false;
    }
    if (step.blocked) return false;
    if (isLast) return true;
    go(index + 1);
    return false;
  };
  const back = () => go(index - 1);

  /** Dev deep links (?onb=n): open a step without the usual gating. */
  const jump = (i: number) => steps[i] && setStepId(steps[i].id);

  const saved: SavedFlow = { stepId: steps[index].id, email, detected, projectId, cloudSetup, googleQuick };
  return { loaded, loadError, steps, index, step: steps[index], canEnter, go, jump, next, back, isLast, restart, detect, detected, saved, client };
}

interface PathCtx {
  surface: Surface;
  client: OAuthClientStatus;
  setClient: (c: OAuthClientStatus) => void;
  ms: MicrosoftClientStatus | null;
  setMs: (m: MicrosoftClientStatus) => void;
  projectId: string;
  setProjectId: (p: string) => void;
  accounts: Account[];
  allAccounts: Account[];
  onAdded: (a: Account) => void;
  lastAdded: Account | null;
  cloudSetup: boolean;
  googleQuick: boolean;
  /** Switch the Google path between quick setup and the Cloud client, at its first step. */
  setGoogleQuick: (on: boolean) => void;
  goTo: (id: string) => void;
  restart: () => void;
  choose: (s: SetupKind) => void;
  retry: () => void;
}

/** Where a provider's steps begin: past whatever is already set up. */
function startStep(d: DetectedProvider, cloudSetup: boolean, ms: MicrosoftClientStatus | null): string {
  switch (pathFor(d)) {
    case "google":
      return cloudSetup ? "g-intro" : "accounts";
    case "microsoft":
      return ms?.clientId ? "ms-signin" : "ms-register";
    case "appPassword":
      return "pw-create";
    case "proton":
      return "proton-bridge";
    case "imap":
      return "imap-connect";
    default:
      return "unsupported";
  }
}

function placeholderSteps(surface: Surface): FlowStep[] {
  const base = { group: "Penguin", heading: "", content: null };
  return surface === "setup"
    ? [
        { ...base, id: "pending-connect", label: "Connect" },
        { ...base, id: "pending-done", label: "Done" },
      ]
    : [];
}

function pathSteps(d: DetectedProvider, c: PathCtx): FlowStep[] {
  const added = !!c.lastAdded || !!addedAccount(d, c.accounts);
  const setup = c.surface === "setup";
  const addedEmail = c.lastAdded?.email ?? d.email;
  const done: FlowStep[] = [
    setup
      ? {
          id: "done",
          label: "Done",
          group: "Penguin",
          heading: "You're all set.",
          nextLabel: "Open inbox",
          content: <DoneStep projectId={c.projectId} setProjectId={c.setProjectId} gmail={pathFor(d) === "google" && !c.googleQuick} />,
        }
      : {
          id: "done",
          label: "Done",
          group: "Penguin",
          heading: `${addedEmail} is connected`,
          nextLabel: "Done",
          content: <AddedDone />,
        },
  ];
  // Connect steps: only Done comes after, and only once the account is in.
  const connectGate = { blocked: !added, blockedHint: d.available ? "Sign in to continue" : undefined, done: added, nextLabel: "Continue" };
  const common = { chip: true };

  switch (pathFor(d)) {
    case "google": {
      if (c.googleQuick) {
        // Gmail quick setup: an app password over IMAP (personal Gmail only).
        const allowed = quickSetupAllowed(d);
        const back = () => c.setGoogleQuick(false);
        return [
          {
            ...common,
            id: "gq-create",
            label: "App password",
            group: "Gmail quick setup",
            time: "2 min",
            heading: allowed ? "Create a Google app password" : "Quick setup isn't available for Workspace",
            lead: allowed
              ? "Instead of a Google Cloud client, Penguin can use an app password: a separate password just for Penguin that you can revoke any time."
              : `${d.domain} is on Google Workspace, which doesn't allow app passwords for mail apps.`,
            blocked: !allowed,
            blockedHint: "Sign in with Google instead",
            content: <PasswordGuideStep detected={d} quick onBack={back} />,
          },
          {
            ...common,
            id: "gq-connect",
            label: "Connect",
            group: "Penguin",
            heading: "Connect Gmail",
            ...connectGate,
            content: (
              <PasswordConnectStep detected={d} accounts={c.accounts} allAccounts={c.allAccounts} onAdded={c.onAdded} onRestart={c.restart} quick onBack={back} />
            ),
          },
          ...done,
        ];
      }
      const offer = <QuickSetupOffer detected={d} onChoose={() => c.setGoogleQuick(true)} />;
      const guide = { projectId: c.projectId, setProjectId: c.setProjectId };
      const cloud: FlowStep[] = c.cloudSetup
        ? [
            {
              ...common,
              id: "g-intro",
              label: "Before you start",
              group: "Google Cloud",
              time: "10 min",
              heading: "Connect Google with your own client",
              lead: "A one-time, 10-minute setup of your own private Google client. It covers every Google account you add.",
              nextLabel: "Start setup",
              content: (
                <>
                  <WelcomeStep onShortcut={() => c.goTo("import")} />
                  {offer}
                </>
              ),
            },
            {
              id: "project",
              label: "Create a project",
              group: "Google Cloud",
              time: "2 min",
              heading: "Create a Google Cloud project",
              lead: "Free, and only you can see it.",
              content: <ProjectStep {...guide} />,
            },
            { id: "api", label: "Enable Gmail API", group: "Google Cloud", time: "30 sec", heading: "Turn on the Gmail API", content: <ApiStep {...guide} /> },
            {
              id: "branding",
              label: "Branding",
              group: "Google Cloud",
              time: "2 min",
              heading: "Name your app",
              lead: "You'll see this name when you sign in.",
              content: <BrandingStep {...guide} />,
            },
            {
              id: "publish",
              label: "Audience & publish",
              group: "Google Cloud",
              time: "30 sec",
              heading: "Publish the app",
              lead: "Keeps your accounts signed in.",
              content: <PublishStep {...guide} />,
            },
            {
              id: "client",
              label: "Create the client",
              group: "Google Cloud",
              time: "1 min",
              heading: "Create a Desktop client",
              lead: "You'll download it as a small JSON file.",
              content: <ClientStep {...guide} />,
            },
            {
              id: "import",
              label: "Add the client",
              group: "Penguin",
              time: "30 sec",
              heading: "Add the client to Penguin",

              blocked: !c.client.configured,
              blockedHint: "Add the client JSON to continue",
              done: c.client.configured,
              nextLabel: "Continue",
              content: (
                <>
                  <ClientImport status={c.client} onStatus={c.setClient} onProjectId={(p) => c.setProjectId(c.projectId || p)} />
                  <Troubleshooting problems={clientProblems} />
                </>
              ),
            },
            {
              id: "sheet",
              label: "Sign-in sheet",
              group: "Penguin",
              time: "optional",
              heading: "Sign in without a browser tab (optional)",
              lead: "With an iOS client, sign-in uses the macOS sheet instead of a browser tab.",
              done: !!c.client.iosClientId,
              nextLabel: c.client.iosClientId ? "Next" : "Skip",
              content: <SheetClientStep projectId={c.projectId} status={c.client} onStatus={c.setClient} />,
            },
          ]
        : [];
      const signIn: FlowStep = {
        ...common,
        ...connectGate,
        id: "accounts",
        label: "Sign in",
        group: "Penguin",
        time: c.cloudSetup ? "1 min" : undefined,
        heading: "Sign in with Google",
        content: (
          <>
            <AccountsPanel allAccounts={c.allAccounts} projectId={c.projectId} autoFocus sheet={!!c.client.iosClientId} loginHint={d.email} onAdded={c.onAdded} />
            {!added && <QuickSetupOffer detected={d} onChoose={() => c.setGoogleQuick(true)} compact />}
          </>
        ),
      };
      return [...cloud, signIn, ...done];
    }

    case "microsoft":
      return [
        {
          ...common,
          id: "ms-register",
          label: "Register the app",
          group: "Microsoft Entra",
          time: "3 min",
          heading: "Register Penguin with Microsoft",
          lead: "A free, one-time registration of your own private app. It covers every Microsoft account you add.",
          content: <MicrosoftRegisterStep />,
        },
        {
          ...common,
          id: "ms-permissions",
          label: "Mail permissions",
          group: "Microsoft Entra",
          time: "1 min",
          heading: "Let the app read and send mail",
          content: <MicrosoftPermissionsStep detected={d} />,
        },
        {
          ...common,
          id: "ms-client",
          label: "Add the client ID",
          group: "Penguin",
          time: "30 sec",
          heading: "Add the client ID to Penguin",
          blocked: !c.ms?.clientId,
          blockedHint: "Add the client ID to continue",
          done: !!c.ms?.clientId,
          nextLabel: "Continue",
          content: <MicrosoftClientStep ms={c.ms} setMs={c.setMs} />,
        },
        {
          ...common,
          id: "ms-signin",
          label: "Sign in",
          group: "Penguin",
          heading: "Sign in with Microsoft",
          ...connectGate,
          content: (
            <MicrosoftSignInStep
              detected={d}
              ms={c.ms}
              accounts={c.accounts}
              allAccounts={c.allAccounts}
              onAdded={c.onAdded}
              onRestart={c.restart}
              onEditClient={() => c.goTo("ms-client")}
            />
          ),
        },
        ...done,
      ];

    case "appPassword":
    case "proton": {
      const g = passwordGuide(d.setup)!;
      const proton = d.setup === "proton";
      const connect: FlowStep = proton
        ? {
            ...common,
            id: "imap-connect",
            label: "Connect",
            group: "Penguin",
            heading: "Connect through Bridge",
            lead: d.available ? "Check the settings against Bridge's, then paste the Bridge password." : undefined,
            ...connectGate,
            content: <ServerSettingsStep detected={d} accounts={c.accounts} allAccounts={c.allAccounts} onAdded={c.onAdded} onRestart={c.restart} />,
          }
        : {
            ...common,
            id: "pw-connect",
            label: "Connect",
            group: "Penguin",
            heading: `Connect ${d.displayName}`,
            ...connectGate,
            content: <PasswordConnectStep detected={d} accounts={c.accounts} allAccounts={c.allAccounts} onAdded={c.onAdded} onRestart={c.restart} />,
          };
      return [
        {
          ...common,
          id: proton ? "proton-bridge" : "pw-create",
          label: proton ? "Proton Mail Bridge" : "App password",
          group: g.name,
          time: proton ? "5 min" : "2 min",
          heading: proton ? "Set up Proton Mail Bridge" : `Create ${d.setup === "icloud" ? "an app-specific" : `a ${g.name} app`} password`,
          lead: proton
            ? "Other mail apps reach Proton Mail through its Bridge app on this Mac."
            : `${g.name} needs a separate password just for Penguin. You can revoke it any time.`,
          content: <PasswordGuideStep detected={d} />,
        },
        connect,
        ...done,
      ];
    }

    case "imap":
      return [
        {
          ...common,
          id: "imap-connect",
          label: "Server settings",
          group: "Penguin",
          heading: "Enter your server settings",
          lead: d.available && d.imap ? `Found for ${d.domain}. Check them, then add your password.` : undefined,
          ...connectGate,
          content: <ServerSettingsStep detected={d} accounts={c.accounts} allAccounts={c.allAccounts} onAdded={c.onAdded} onRestart={c.restart} />,
        },
        ...done,
      ];

    default:
      return [
        {
          ...common,
          id: "unsupported",
          label: d.offline ? "Couldn't check" : "Not supported yet",
          group: "Penguin",
          heading: d.offline ? `Couldn't check ${d.domain}` : `We don't support ${d.domain} automatically yet`,
          lead: unsupportedLead(d),
          blocked: true,
          blockedHint: "Choose a provider or enter server settings",
          content: <UnsupportedStep detected={d} onChoose={c.choose} onRetry={c.retry} />,
        },
      ];
  }
}

/** The Add account modal's last screen: the heading says who; this says what happens now. */
function AddedDone() {
  return (
    <p className="onb-added">
      <Icon name="check" size="sm" />
      Syncing now, newest mail first.
    </p>
  );
}
