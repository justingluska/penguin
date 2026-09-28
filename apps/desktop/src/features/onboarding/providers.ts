// Add account, provider side: which flow an address takes, each provider's
// app-password instructions, the Microsoft (Entra) registration facts, and
// plain-words errors. Pure data and functions (tests/onboarding.test.ts); the
// screens live in ProviderSteps.tsx.
//
// Instructions match the providers' own help pages as of 2026-09-25
// (docs/providers-and-onboarding.md §2–3). In step text, [[x]] marks a label
// the user looks for on screen and {email} is the address being added.
import type { CommandError, ConnectAccountRequest, DetectedProvider, MailSecurity, ServerSettings, SetupKind } from "../../lib/types";

export type FlowPath = "google" | "microsoft" | "appPassword" | "proton" | "imap" | "unsupported";

export function pathFor(p: Pick<DetectedProvider, "setup">): FlowPath {
  switch (p.setup) {
    case "google":
      return "google";
    case "microsoft":
      return "microsoft";
    case "yahoo":
    case "aol":
    case "icloud":
    case "fastmail":
      return "appPassword";
    case "proton":
      return "proton";
    case "imap":
      return "imap";
    default:
      return "unsupported";
  }
}

/** The manual "Choose provider" buttons, in order. */
export const CHOICES: { setup: SetupKind; label: string; sub: string }[] = [
  { setup: "google", label: "Google", sub: "Gmail, Workspace" },
  { setup: "microsoft", label: "Microsoft", sub: "Outlook, Hotmail, 365" },
  { setup: "yahoo", label: "Yahoo", sub: "Yahoo Mail, Ymail" },
  { setup: "icloud", label: "iCloud", sub: "iCloud Mail, me.com" },
  { setup: "fastmail", label: "Fastmail", sub: "Incl. your own domain" },
  { setup: "imap", label: "Other", sub: "Any IMAP server" },
];

/** Good enough to ask the backend, which has the final word. */
export function looksLikeEmail(s: string): boolean {
  return /^[^\s@]+@[^\s@]+\.[^\s@.]{2,}$/.test(s.trim());
}

/** The domain part of whatever was typed, for "Checking harbor.example…". */
export function domainOf(s: string): string {
  const at = s.trim().lastIndexOf("@");
  return at < 0 ? "" : s.trim().slice(at + 1).toLowerCase();
}

// ---------------------------------------------------------------------------
// App passwords (Yahoo, AOL, iCloud, Fastmail) and Proton Mail Bridge
// ---------------------------------------------------------------------------

export interface GuideStep {
  title: string;
  detail?: string;
}

export interface PasswordGuide {
  /** "Yahoo", "iCloud". */
  name: string;
  /** The button that opens the right page. */
  page: { label: string; url: string };
  /** What the account needs first, if anything. */
  needs?: string;
  steps: GuideStep[];
  /** Field label and placeholder on the Connect step. */
  field: string;
  placeholder: string;
  note?: string;
}

const YAHOO_STEPS = (name: string): GuideStep[] => [
  { title: `Open ${name}'s [[Account Security]] page`, detail: "Sign in as {email} if asked." },
  {
    title: "Under [[External connections]], click [[Create app password]]",
    detail: `Not there? ${name} hides it in new and private browser windows; use one you've signed in with before.`,
  },
  { title: "Name it [[Penguin]], then click [[Generate password]]" },
  { title: "Copy the password", detail: `${name} shows it once. Paste it on the next step, then click [[Done]] in ${name}.` },
];

export const PASSWORD_GUIDES: Record<"yahoo" | "aol" | "icloud" | "fastmail" | "proton", PasswordGuide> = {
  yahoo: {
    name: "Yahoo",
    page: { label: "Open Account Security", url: "https://login.yahoo.com/account/security" },
    steps: YAHOO_STEPS("Yahoo"),
    field: "App password",
    placeholder: "16 letters from Yahoo",
  },
  aol: {
    name: "AOL",
    page: { label: "Open Account Security", url: "https://login.aol.com/account/security" },
    steps: YAHOO_STEPS("AOL"),
    field: "App password",
    placeholder: "16 letters from AOL",
  },
  icloud: {
    name: "iCloud",
    page: { label: "Open Apple Account", url: "https://account.apple.com" },
    needs: "Two-factor authentication on your Apple Account. Most accounts already have it.",
    steps: [
      { title: "Sign in to your Apple Account", detail: "Use your Apple Account, even if it has a different email than {email}." },
      { title: "In [[Sign-In and Security]], choose [[App-Specific Passwords]]" },
      { title: "Click [[Generate an app-specific password]] and name it [[Penguin]]", detail: "Apple asks for your Apple Account password to confirm." },
      { title: "Copy the password", detail: "It looks like xxxx-xxxx-xxxx-xxxx." },
    ],
    field: "App-specific password",
    placeholder: "xxxx-xxxx-xxxx-xxxx",
  },
  fastmail: {
    name: "Fastmail",
    page: { label: "Open Privacy & Security", url: "https://app.fastmail.com/settings/security" },
    needs: "A Fastmail plan that includes IMAP. Basic plans can't be used with other mail apps.",
    steps: [
      { title: "Open [[Settings]] → [[Privacy & Security]]" },
      { title: "Under [[Connected apps & API tokens]], click [[Manage app passwords and access]]" },
      { title: "Click [[New app password]]", detail: "Fastmail may ask for your password first." },
      {
        title: "Name it [[Penguin]], keep access at [[Mail, Contacts & Calendars]], then [[Generate password]]",
        detail: "Copy the password. Keep the page open until Penguin connects, then click [[Done]].",
      },
    ],
    field: "App password",
    placeholder: "16 characters from Fastmail",
  },
  proton: {
    name: "Proton Mail",
    page: { label: "Get Proton Mail Bridge", url: "https://proton.me/mail/bridge" },
    needs: "A paid Proton plan.",
    steps: [
      { title: "Install and open Proton Mail Bridge" },
      { title: "Sign in to your Proton account in Bridge" },
      {
        title: "Select your account to see its mail settings",
        detail: "Bridge shows IMAP and SMTP settings and a Bridge password. Use that password, not your Proton password.",
      },
    ],
    field: "Bridge password",
    placeholder: "From Bridge's mail settings",
    note: "Keep Bridge running while Penguin is open; Penguin talks to it on this Mac only.",
  },
};

export function passwordGuide(setup: SetupKind): PasswordGuide | null {
  return setup === "yahoo" || setup === "aol" || setup === "icloud" || setup === "fastmail" || setup === "proton" ? PASSWORD_GUIDES[setup] : null;
}

// ---------------------------------------------------------------------------
// Gmail quick setup: an app password over IMAP, no Google Cloud project
// (docs/providers-and-onboarding.md → "Gmail quick setup over IMAP")
// ---------------------------------------------------------------------------

export const GMAIL_QUICK: PasswordGuide = {
  name: "Google",
  page: { label: "Open App passwords", url: "https://myaccount.google.com/apppasswords" },
  needs: "2-Step Verification on your Google Account. Google only offers app passwords once it's on, and not with Advanced Protection.",
  steps: [
    {
      title: "Open [[App passwords]] in your Google Account",
      detail: "Sign in as {email} if asked. If Google says the setting isn't available, turn on [[2-Step Verification]] under [[Security]] first.",
    },
    { title: "Type [[Penguin]] as the app name, then click [[Create]]" },
    { title: "Copy the password", detail: "Google shows it once, as 16 letters in groups of four. Paste it on the next step, then click [[Done]]." },
  ],
  field: "App password",
  placeholder: "16 letters from Google",
};

/** Shown next to the quick setup password field. */
export const GMAIL_QUICK_DISCLOSURE =
  "An app password gives full access to your Google account, can't be limited, and can be revoked any time in your Google Account. Sync is slower than with your own Google Cloud project.";

/** Why a Workspace address gets no quick setup. */
export const WORKSPACE_NO_APP_PASSWORDS =
  "Google Workspace turned off app passwords for IMAP and SMTP in March 2025, so Workspace accounts sign in with Google through your own Google Cloud client.";

/** Quick setup is for personal Gmail only: @gmail.com and @googlemail.com. */
export function quickSetupAllowed(d: Pick<DetectedProvider, "kind" | "domain">): boolean {
  return d.kind === "gmail" && /^(gmail|googlemail)\.com$/i.test(d.domain);
}

/** connect_account's request for Gmail quick setup. */
export function gmailQuickSetupRequest(email: string, password: string): ConnectAccountRequest {
  const username = email.trim();
  return {
    email: username,
    kind: "gmail",
    auth: "appPassword",
    password: cleanAppPassword(password),
    imap: { host: "imap.gmail.com", port: 993, security: "tls", username },
    smtp: { host: "smtp.gmail.com", port: 465, security: "tls", username },
  };
}

/**
 * A failed connect_account in plain words (after "Couldn't connect."). The
 * backend says what the server refused (`invalidInput`, already user-facing,
 * e.g. "Yahoo didn't accept that password. Use an app password: …");
 * `network` means the server couldn't be reached, which `unreachable` explains
 * for this flow, with the raw reason after it.
 */
export function connectErrorText(err: { code: CommandError["code"] | string; message: string }, unreachable: string): string {
  const m = err.message.trim();
  if (err.code === "network") return m ? `${unreachable} (${m})` : unreachable;
  if (!m) return "Something went wrong. Try again.";
  return m.charAt(0).toUpperCase() + m.slice(1);
}

/** Text split around its https:// addresses (a trailing period or bracket stays text). */
export function linkParts(text: string): { text: string; url?: string }[] {
  const out: { text: string; url?: string }[] = [];
  const re = /https:\/\/[^\s<>"]+/g;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    const url = m[0].replace(/[.,;:!?)\]]+$/, "");
    if (m.index > last) out.push({ text: text.slice(last, m.index) });
    out.push({ text: url, url });
    last = m.index + url.length;
    re.lastIndex = last;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/** App passwords never contain spaces; providers show them in groups. */
export function cleanAppPassword(s: string): string {
  return s.replace(/\s+/g, "");
}

// ---------------------------------------------------------------------------
// Microsoft: the user's own Entra app registration
// ---------------------------------------------------------------------------

export const MICROSOFT = {
  /** Entra admin center → App registrations. */
  appRegistrations: "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade",
  appName: "Penguin",
  /** Mobile and desktop applications platform; the port is ignored for localhost. */
  redirectUri: "http://localhost",
  /** Delegated Microsoft Graph permissions (openid, profile and email come with sign-in). */
  permissions: ["offline_access", "User.Read", "Mail.ReadWrite", "Mail.Send", "MailboxSettings.Read"],
  azureFree: "https://azure.microsoft.com/free/",
};

const GUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i;

/** The first GUID in what was pasted, lowercased (mirrors ms_client::normalize_client_id). */
export function normalizeClientId(input: string): string | null {
  const m = GUID.exec(input);
  return m ? m[0].toLowerCase() : null;
}

/** The link a work account's IT admin opens to approve the app for everyone. */
export function adminConsentUrl(domain: string, clientId: string): string {
  return `https://login.microsoftonline.com/${encodeURIComponent(domain)}/adminconsent?client_id=${encodeURIComponent(clientId)}`;
}

export interface MicrosoftError {
  text: string;
  /** Show the "send this link to your IT admin" card. */
  adminConsent: boolean;
}

/** A Microsoft sign-in failure in plain words, from the AADSTS code in its message. */
export function microsoftErrorText(message: string): MicrosoftError {
  const m = message;
  if (/AADSTS65001|AADSTS90094|admin (approval|consent)|consent_required|need admin/i.test(m)) {
    return { adminConsent: true, text: "Your organization needs an admin to approve Penguin before you can sign in." };
  }
  // Before "client does not exist": Microsoft's consumer error says both.
  if (/unauthorized_client|not enabled for consumers|AADSTS50020|AADSTS500200/i.test(m)) {
    return {
      adminConsent: false,
      text: "The app registration doesn't allow this kind of account. Set Supported account types to include personal Microsoft accounts and any organization.",
    };
  }
  if (/AADSTS700016|AADSTS90002|client does not exist|not found in the directory/i.test(m)) {
    return { adminConsent: false, text: "Microsoft doesn't know that client ID. Copy the Application (client) ID, not the Directory (tenant) ID." };
  }
  if (/AADSTS50011|redirect_uri/i.test(m)) {
    return { adminConsent: false, text: `The app registration is missing the ${MICROSOFT.redirectUri} redirect under Mobile and desktop applications.` };
  }
  if (/AADSTS7000218|client_assertion|client_secret/i.test(m)) {
    return { adminConsent: false, text: `The redirect is on the Web platform. Remove it and add ${MICROSOFT.redirectUri} under Mobile and desktop applications.` };
  }
  if (/AADSTS53000|AADSTS53003|compliant|conditional access/i.test(m)) {
    return { adminConsent: false, text: "Your organization only allows approved devices or apps. Ask your IT admin whether Penguin can be allowed." };
  }
  return { adminConsent: false, text: m.charAt(0).toUpperCase() + m.slice(1) };
}

// ---------------------------------------------------------------------------
// Server settings form (Other IMAP, Proton Bridge)
// ---------------------------------------------------------------------------

export function defaultPort(kind: "imap" | "smtp", security: MailSecurity): number {
  if (kind === "imap") return security === "starttls" || security === "plain" ? 143 : 993;
  return security === "starttls" || security === "plain" ? 587 : 465;
}

export function validHost(host: string): boolean {
  const h = host.trim();
  return (
    h.length > 0 &&
    h.length <= 253 &&
    h.split(".").every((l) => l.length > 0 && l.length <= 63 && /^[a-z0-9-]+$/i.test(l) && !l.startsWith("-") && !l.endsWith("-"))
  );
}

export interface ServerForm {
  imap: ServerSettings;
  smtp: ServerSettings;
  password: string;
}

export type ServerFormErrors = Partial<Record<"imapHost" | "imapPort" | "smtpHost" | "smtpPort" | "username" | "password", string>>;

export function serverFormErrors(f: ServerForm): ServerFormErrors {
  const e: ServerFormErrors = {};
  const port = (p: number) => Number.isInteger(p) && p > 0 && p < 65536;
  if (!validHost(f.imap.host)) e.imapHost = "Enter the incoming server, like imap.example.com.";
  if (!port(f.imap.port)) e.imapPort = "Port 1–65535.";
  if (!validHost(f.smtp.host)) e.smtpHost = "Enter the outgoing server, like smtp.example.com.";
  if (!port(f.smtp.port)) e.smtpPort = "Port 1–65535.";
  if (!f.imap.username.trim()) e.username = "Enter the username, usually your full address.";
  if (!f.password) e.password = "Enter the password.";
  return e;
}

/** A form prefilled from detection, or empty fields keyed to the address. */
export function initialServerForm(p: DetectedProvider): ServerForm {
  const blank = (kind: "imap" | "smtp"): ServerSettings => ({ host: "", port: defaultPort(kind, "tls"), security: "tls", username: p.email });
  return { imap: p.imap ? { ...p.imap } : blank("imap"), smtp: p.smtp ? { ...p.smtp } : blank("smtp"), password: "" };
}
