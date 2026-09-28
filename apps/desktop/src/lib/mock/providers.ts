// OWNER: onboarding agent. Mock provider detection, the Microsoft client and
// connect_account, so every Add account path is clickable without Rust.
// Mirrors src-tauri/src/providers/ loosely: a few real consumer domains as
// data, plus fictional .example fixtures for the custom-domain cases:
//
//   northwind.example → Google Workspace (MX smtp.google.com)
//   contoso.example   → Microsoft 365 (MX *.mail.protection.outlook.com)
//   harbor.example    → Other IMAP from the ISPDB (imap.harbor.example)
//   family.example    → iCloud (MX mx01.mail.icloud.com)
//   bigcorp.example   → unknown behind Proofpoint
//   offline.example   → unknown, offline
//   anything else     → unknown, MX mail.<domain>
//
// Dev knobs (localStorage "1", or ?mock=<name>):
//   penguin.mock.providersPending → non-Google providers aren't available
//                                   yet, like the real app today ("coming in
//                                   the next update").
//   penguin.mock.connectFails     → connect_account rejects (Microsoft with
//                                   AADSTS65001, others with the provider's
//                                   "didn't accept that password" invalidInput).
//   penguin.mock.connectOffline   → password connects fail with `network`
//                                   (server unreachable).
//
// Password accounts (app password, IMAP, Proton Bridge) connect like the real
// backend does: the account is added as provider "imap" and its sync-status
// events count up from "backfilling" to "incremental".
import type { Account, CommandError, DetectedProvider, MicrosoftClientStatus, ProviderKind, ServerSettings, SetupKind } from "../types";
import { mockProviderFields } from "./accounts";
import type { MockHandler } from "./index";
import { mockMail } from "./mail";

function flag(key: string): boolean {
  try {
    const param = new URLSearchParams(window.location.search).get("mock");
    if (param && key === `penguin.mock.${param}`) return true;
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

function reject(code: CommandError["code"], message: string): never {
  throw { code, message } satisfies CommandError;
}

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

type Preset = Pick<DetectedProvider, "kind" | "setup" | "displayName" | "auth"> & { imap: [string, number, ServerSettings["security"], "addr" | "local"]; smtp: [string, number, ServerSettings["security"]] };

const PRESETS: Record<string, Preset> = {
  gmail: { kind: "gmail", setup: "google", displayName: "Gmail", auth: "googleOAuth", imap: ["imap.gmail.com", 993, "tls", "addr"], smtp: ["smtp.gmail.com", 465, "tls"] },
  workspace: { kind: "googleWorkspace", setup: "google", displayName: "Google Workspace", auth: "googleOAuth", imap: ["imap.gmail.com", 993, "tls", "addr"], smtp: ["smtp.gmail.com", 465, "tls"] },
  outlook: { kind: "outlookPersonal", setup: "microsoft", displayName: "Outlook.com", auth: "microsoftOAuth", imap: ["outlook.office365.com", 993, "tls", "addr"], smtp: ["smtp-mail.outlook.com", 587, "starttls"] },
  m365: { kind: "microsoft365", setup: "microsoft", displayName: "Microsoft 365", auth: "microsoftOAuth", imap: ["outlook.office365.com", 993, "tls", "addr"], smtp: ["smtp.office365.com", 587, "starttls"] },
  yahoo: { kind: "yahoo", setup: "yahoo", displayName: "Yahoo Mail", auth: "appPassword", imap: ["imap.mail.yahoo.com", 993, "tls", "addr"], smtp: ["smtp.mail.yahoo.com", 465, "tls"] },
  aol: { kind: "aol", setup: "aol", displayName: "AOL Mail", auth: "appPassword", imap: ["imap.aol.com", 993, "tls", "addr"], smtp: ["smtp.aol.com", 465, "tls"] },
  icloud: { kind: "icloud", setup: "icloud", displayName: "iCloud Mail", auth: "appPassword", imap: ["imap.mail.me.com", 993, "tls", "local"], smtp: ["smtp.mail.me.com", 587, "starttls"] },
  fastmail: { kind: "fastmail", setup: "fastmail", displayName: "Fastmail", auth: "appPassword", imap: ["imap.fastmail.com", 993, "tls", "addr"], smtp: ["smtp.fastmail.com", 465, "tls"] },
  proton: { kind: "imapGeneric", setup: "proton", displayName: "Proton Mail", auth: "imapPassword", imap: ["127.0.0.1", 1143, "starttls", "addr"], smtp: ["127.0.0.1", 1025, "starttls"] },
};

const TABLE: [RegExp, keyof typeof PRESETS][] = [
  [/^(gmail|googlemail)\.com$/, "gmail"],
  [/^(outlook|hotmail|live)\.[a-z.]+$|^(msn|windowslive)\.com$/, "outlook"],
  [/^yahoo\.[a-z.]+$|^(ymail|rocketmail|myyahoo)\.com$/, "yahoo"],
  [/^aol\.[a-z.]+$|^aim\.com$/, "aol"],
  [/^(icloud|me|mac)\.com$/, "icloud"],
  [/^fastmail\.[a-z.]+$|^sent\.com$/, "fastmail"],
  [/^(proton\.me|protonmail\.(com|ch)|pm\.me)$/, "proton"],
];

const FIXTURES: Record<string, Partial<DetectedProvider> & { preset?: keyof typeof PRESETS }> = {
  "northwind.example": { preset: "workspace", source: "mx", mxHost: "smtp.google.com" },
  "contoso.example": { preset: "m365", source: "mx", mxHost: "contoso-example.mail.protection.outlook.com" },
  "family.example": { preset: "icloud", source: "mx", mxHost: "mx01.mail.icloud.com" },
  "harbor.example": {
    kind: "imapGeneric",
    setup: "imap",
    displayName: "Harbor Mail",
    auth: "imapPassword",
    source: "ispdb",
    mxHost: "mx1.harbor.example",
    imap: { host: "imap.harbor.example", port: 993, security: "tls", username: "" },
    smtp: { host: "smtp.harbor.example", port: 587, security: "starttls", username: "" },
  },
  "bigcorp.example": { source: "unknown", mxHost: "mxa-001.gslb.pphosted.com", gateway: "Proofpoint" },
  "offline.example": { source: "unknown", offline: true },
};

const CHOICE_PRESET: Partial<Record<SetupKind, keyof typeof PRESETS>> = { yahoo: "yahoo", aol: "aol", icloud: "icloud", fastmail: "fastmail", proton: "proton" };

function split(input: string): { email: string; local: string; domain: string } {
  const s = String(input ?? "").trim();
  const at = s.lastIndexOf("@");
  const local = s.slice(0, at);
  const domain = s.slice(at + 1).toLowerCase().replace(/\.$/, "");
  if (at < 1 || !/^[^\s@]+$/.test(local) || !/^[a-z0-9-]+(\.[a-z0-9-]+)*\.[a-z]{2,}$/.test(domain)) {
    reject("invalidInput", "Type your full email address, like name@example.com.");
  }
  return { email: `${local}@${domain}`, local, domain };
}

function fromPreset(key: keyof typeof PRESETS, a: { email: string; local: string }): Pick<DetectedProvider, "kind" | "setup" | "displayName" | "auth" | "imap" | "smtp"> {
  const p = PRESETS[key];
  return {
    kind: p.kind,
    setup: p.setup,
    displayName: p.displayName,
    auth: p.auth,
    imap: { host: p.imap[0], port: p.imap[1], security: p.imap[2], username: p.imap[3] === "local" ? a.local : a.email },
    smtp: { host: p.smtp[0], port: p.smtp[1], security: p.smtp[2], username: a.email },
  };
}

function detect(input: string, choose: SetupKind | null): DetectedProvider {
  const a = split(input);
  const base: DetectedProvider = {
    email: a.email,
    domain: a.domain,
    kind: "unknown",
    displayName: a.domain,
    auth: "imapPassword",
    setup: "unsupported",
    imap: null,
    smtp: null,
    source: "unknown",
    mxHost: `mail.${a.domain}`,
    gateway: null,
    offline: false,
    available: false,
  };
  let d: DetectedProvider;
  if (choose) {
    if (choose === "unsupported") reject("invalidInput", "Choose a provider.");
    const fixture = FIXTURES[a.domain];
    const personal = TABLE.find(([re]) => re.test(a.domain))?.[1];
    const key: keyof typeof PRESETS | undefined =
      choose === "google" ? (personal === "gmail" ? "gmail" : "workspace") : choose === "microsoft" ? (personal === "outlook" ? "outlook" : "m365") : CHOICE_PRESET[choose];
    d = key
      ? { ...base, ...fromPreset(key, a), source: "manual", mxHost: null }
      : {
          ...base,
          kind: "imapGeneric",
          setup: "imap",
          displayName: fixture?.displayName ?? "Other mail server",
          source: "manual",
          mxHost: null,
          imap: fixture?.imap ? { ...fixture.imap, username: a.email } : null,
          smtp: fixture?.smtp ? { ...fixture.smtp, username: a.email } : null,
        };
  } else {
    const hit = TABLE.find(([re]) => re.test(a.domain));
    const fixture = FIXTURES[a.domain];
    if (hit) d = { ...base, ...fromPreset(hit[1], a), source: "domainTable", mxHost: null };
    else if (fixture?.preset) d = { ...base, ...fromPreset(fixture.preset, a), source: fixture.source!, mxHost: fixture.mxHost ?? null };
    else if (fixture) {
      const { preset: _preset, ...rest } = fixture;
      d = {
        ...base,
        ...rest,
        imap: rest.imap ? { ...rest.imap, username: a.email } : null,
        smtp: rest.smtp ? { ...rest.smtp, username: a.email } : null,
        mxHost: rest.offline ? null : (rest.mxHost ?? null),
      } as DetectedProvider;
    } else d = base;
  }
  // The mock can connect everything, unless it's asked to behave like today's app.
  d.available = d.setup !== "unsupported" && (d.auth === "googleOAuth" || !flag("penguin.mock.providersPending"));
  return d;
}

/** What the backend says when a provider refuses the password (invalidInput), in the mock. */
const REFUSED: Partial<Record<ProviderKind, string>> = {
  gmail: "Gmail didn't accept that password. Use an app password: https://myaccount.google.com/apppasswords",
  yahoo: "Yahoo didn't accept that password. Use an app password: https://login.yahoo.com/account/security",
  aol: "AOL didn't accept that password. Use an app password: https://login.aol.com/account/security",
  icloud: "iCloud didn't accept that password. Use an app-specific password: https://account.apple.com",
  fastmail: "Fastmail didn't accept that password. Use an app password: https://app.fastmail.com/settings/security",
};

let msClient: string | null = null;
const MS_PATH = "~/Library/Application Support/co.gluska.penguin/microsoft-oauth-client.json";
const msStatus = (): MicrosoftClientStatus => ({ clientId: msClient, path: MS_PATH });
const COLORS = ["#8e4ec6", "#12a594", "#e5484d", "#0090ff"];

export const providerHandlers: Record<string, MockHandler> = {
  detect_provider: async ({ email, choose }) => {
    // Detection takes a moment on a custom domain; the table is instant.
    const d = detect(email as string, (choose as SetupKind | null) ?? null);
    if (d.source !== "domainTable" && d.source !== "manual") await wait(d.offline ? 1400 : 700);
    return d;
  },
  microsoft_client_status: () => msStatus(),
  set_microsoft_client: ({ clientId }) => {
    const raw = (clientId as string | null)?.trim() ?? "";
    if (!raw) msClient = null;
    else {
      const m = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i.exec(raw);
      if (!m) reject("invalidInput", "That isn't an Application (client) ID. Copy the ID from the app's Overview page; it looks like 1b2c3d4e-0000-1111-2222-333344445555.");
      msClient = m[0].toLowerCase();
    }
    return msStatus();
  },
  connect_account: async ({ request }) => {
    const req = request as import("../types").ConnectAccountRequest;
    await wait(req.auth === "microsoftOAuth" ? 1800 : 1100);
    if (flag("penguin.mock.connectOffline") && req.auth !== "microsoftOAuth") {
      reject("network", `connecting to ${req.imap?.host ?? "the server"}:${req.imap?.port ?? 993}: connection timed out`);
    }
    // Like the backend: Workspace turned off app passwords for IMAP/SMTP.
    if (req.auth === "appPassword" && req.imap?.host === "imap.gmail.com" && !/@(gmail|googlemail)\.com$/i.test(req.email)) {
      reject(
        "invalidInput",
        "Google Workspace accounts can't use app passwords for mail apps (Google turned them off in March 2025). Sign in with Google instead.",
      );
    }
    if (flag("penguin.mock.connectFails")) {
      if (req.auth === "microsoftOAuth") reject("other", "AADSTS65001: The user or administrator has not consented to use the application. Need admin approval.");
      reject("invalidInput", REFUSED[req.kind] ?? "The server rejected the username or password (AUTHENTICATIONFAILED).");
    }
    const live = mockMail.accounts();
    const id = req.email.toLowerCase();
    if (live.some((a) => a.id === id)) reject("invalidInput", `${req.email} is already in Penguin.`);
    const provider = req.auth === "microsoftOAuth" ? "microsoft" : req.auth === "googleOAuth" ? "gmail" : "imap";
    const config =
      provider === "imap"
        ? { auth: req.auth, imap: req.imap ?? null, smtp: req.smtp ?? null, host: req.kind }
        : provider === "microsoft"
          ? { auth: req.auth, clientId: req.clientId ?? null, host: req.kind }
          : {};
    const acc: Account = {
      id,
      email: req.email,
      displayName: null,
      nickname: null,
      color: COLORS[live.length % COLORS.length],
      addedAt: Date.now(),
      ...mockProviderFields(provider, config),
    };
    live.push(acc);
    const total = 12_480;
    let indexed = 0;
    const step = () => {
      const doneNow = indexed >= total;
      mockMail.setSync({
        accountId: acc.id,
        phase: doneNow ? "incremental" : "backfilling",
        indexed: Math.min(indexed, total),
        totalEstimate: total,
        lastSyncedAt: doneNow ? Date.now() : null,
        error: null,
        ratePerMin: doneNow ? null : 9_000,
        etaSecs: doneNow ? null : Math.ceil((total - indexed) / 150),
      });
      if (!doneNow) {
        indexed += 1_040;
        setTimeout(step, 250);
      }
    };
    setTimeout(step, 50);
    return acc;
  },
};
