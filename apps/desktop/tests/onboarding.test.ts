// Add account: which flow an address takes, the provider instructions, and
// the plain-words errors (features/onboarding/providers.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  CHOICES,
  GMAIL_QUICK,
  GMAIL_QUICK_DISCLOSURE,
  PASSWORD_GUIDES,
  WORKSPACE_NO_APP_PASSWORDS,
  gmailQuickSetupRequest,
  quickSetupAllowed,
  adminConsentUrl,
  cleanAppPassword,
  connectErrorText,
  defaultPort,
  domainOf,
  initialServerForm,
  linkParts,
  looksLikeEmail,
  microsoftErrorText,
  normalizeClientId,
  passwordGuide,
  pathFor,
  serverFormErrors,
  validHost,
} from "../src/features/onboarding/providers.ts";
import type { DetectedProvider } from "../src/lib/types.ts";

const base: DetectedProvider = {
  email: "sam@harbor.example",
  domain: "harbor.example",
  kind: "imapGeneric",
  displayName: "Harbor Mail",
  auth: "imapPassword",
  setup: "imap",
  imap: { host: "imap.harbor.example", port: 993, security: "tls", username: "sam@harbor.example" },
  smtp: { host: "smtp.harbor.example", port: 587, security: "starttls", username: "sam@harbor.example" },
  source: "ispdb",
  mxHost: "mx1.harbor.example",
  gateway: null,
  offline: false,
  available: true,
};

test("each setup kind has one flow", () => {
  assert.equal(pathFor({ setup: "google" }), "google");
  assert.equal(pathFor({ setup: "microsoft" }), "microsoft");
  for (const s of ["yahoo", "aol", "icloud", "fastmail"] as const) assert.equal(pathFor({ setup: s }), "appPassword");
  assert.equal(pathFor({ setup: "proton" }), "proton");
  assert.equal(pathFor({ setup: "imap" }), "imap");
  assert.equal(pathFor({ setup: "unsupported" }), "unsupported");
});

test("the manual picker offers the six providers from the brief, in order", () => {
  assert.deepEqual(
    CHOICES.map((c) => c.setup),
    ["google", "microsoft", "yahoo", "icloud", "fastmail", "imap"],
  );
});

test("addresses are checked loosely before asking the backend", () => {
  assert.ok(looksLikeEmail(" sam@harbor.example "));
  assert.ok(looksLikeEmail("sam+news@mail.harbor.example"));
  for (const bad of ["", "sam", "sam@", "sam@harbor", "sam @harbor.example", "sam@harbor.e"]) assert.ok(!looksLikeEmail(bad), bad);
  assert.equal(domainOf("Sam@Harbor.EXAMPLE"), "harbor.example");
  assert.equal(domainOf("nothing"), "");
});

test("every app-password guide opens an https page and ends with copying the password", () => {
  for (const [key, g] of Object.entries(PASSWORD_GUIDES)) {
    assert.match(g.page.url, /^https:\/\//, key);
    assert.ok(g.steps.length >= 3, key);
    assert.ok(g.field && g.placeholder, key);
    for (const s of g.steps) assert.equal((s.title.match(/\[\[/g) ?? []).length, (s.title.match(/\]\]/g) ?? []).length, `${key}: ${s.title}`);
  }
  // The exact paths from the providers' help pages.
  assert.ok(PASSWORD_GUIDES.yahoo.steps.some((s) => s.title.includes("[[External connections]]") && s.title.includes("[[Create app password]]")));
  assert.ok(PASSWORD_GUIDES.icloud.steps.some((s) => s.title.includes("[[Sign-In and Security]]") && s.title.includes("[[App-Specific Passwords]]")));
  assert.ok(PASSWORD_GUIDES.fastmail.steps.some((s) => s.title.includes("[[Manage app passwords and access]]")));
  assert.equal(passwordGuide("google"), null);
  assert.equal(passwordGuide("aol")?.page.url, "https://login.aol.com/account/security");
});

test("app passwords lose the spaces providers show them with", () => {
  assert.equal(cleanAppPassword(" abcd efgh\tijkl mnop "), "abcdefghijklmnop");
  assert.equal(cleanAppPassword("abcd-efgh-ijkl-mnop"), "abcd-efgh-ijkl-mnop");
});

test("Microsoft client IDs are found in whatever was pasted", () => {
  const id = "1b2c3d4e-0000-1111-2222-333344445555";
  assert.equal(normalizeClientId(id), id);
  assert.equal(normalizeClientId("{1B2C3D4E-0000-1111-2222-333344445555}"), id);
  assert.equal(normalizeClientId(`Application (client) ID : ${id}`), id);
  assert.equal(normalizeClientId("penguin"), null);
  assert.equal(normalizeClientId("1b2c3d4e-0000-1111-2222-33334444555"), null);
});

test("the admin approval link names the organization's domain and the client", () => {
  assert.equal(
    adminConsentUrl("contoso.example", "1b2c3d4e-0000-1111-2222-333344445555"),
    "https://login.microsoftonline.com/contoso.example/adminconsent?client_id=1b2c3d4e-0000-1111-2222-333344445555",
  );
});

test("Microsoft sign-in errors become plain words", () => {
  assert.equal(microsoftErrorText("AADSTS65001: The user or administrator has not consented").adminConsent, true);
  assert.equal(microsoftErrorText("Need admin approval").adminConsent, true);
  assert.match(microsoftErrorText("AADSTS700016: Application with identifier was not found in the directory").text, /tenant/);
  assert.match(microsoftErrorText("unauthorized_client: The client does not exist or is not enabled for consumers").text, /personal Microsoft accounts/);
  assert.match(microsoftErrorText("AADSTS50011: The redirect URI does not match").text, /http:\/\/localhost/);
  assert.match(microsoftErrorText("AADSTS7000218: client_assertion or client_secret required").text, /Mobile and desktop/);
  const other = microsoftErrorText("something odd");
  assert.equal(other.adminConsent, false);
  assert.equal(other.text, "Something odd");
});

test("server settings: default ports follow security, and the form is validated", () => {
  assert.equal(defaultPort("imap", "tls"), 993);
  assert.equal(defaultPort("imap", "starttls"), 143);
  assert.equal(defaultPort("smtp", "tls"), 465);
  assert.equal(defaultPort("smtp", "starttls"), 587);
  assert.ok(validHost("imap.harbor.example"));
  assert.ok(validHost("127.0.0.1"));
  for (const bad of ["", "imap harbor", "-imap.example", "imap..example", "imap.example/"]) assert.ok(!validHost(bad), bad);

  const filled = initialServerForm(base);
  assert.equal(filled.imap.host, "imap.harbor.example");
  assert.deepEqual(serverFormErrors({ ...filled, password: "pw" }), {});
  assert.deepEqual(Object.keys(serverFormErrors(filled)), ["password"]);

  const empty = initialServerForm({ ...base, imap: null, smtp: null });
  assert.equal(empty.imap.username, "sam@harbor.example");
  assert.equal(empty.imap.port, 993);
  assert.equal(empty.smtp.port, 465);
  const errs = serverFormErrors({ ...empty, imap: { ...empty.imap, port: 0 } });
  assert.deepEqual(Object.keys(errs).sort(), ["imapHost", "imapPort", "password", "smtpHost"]);
});

test("Gmail quick setup: personal Gmail only, and the exact connect_account request", () => {
  const gmail = { ...base, email: "sam.okafor@gmail.com", domain: "gmail.com", kind: "gmail" as const, setup: "google" as const };
  assert.ok(quickSetupAllowed(gmail));
  assert.ok(quickSetupAllowed({ ...gmail, domain: "googlemail.com" }));
  assert.ok(!quickSetupAllowed({ ...gmail, domain: "northwind.example", kind: "googleWorkspace" }));
  // Not a Gmail address, even if something called it "gmail".
  assert.ok(!quickSetupAllowed({ ...gmail, domain: "northwind.example" }));

  assert.deepEqual(gmailQuickSetupRequest(" sam.okafor@gmail.com ", "abcd efgh ijkl mnop"), {
    email: "sam.okafor@gmail.com",
    kind: "gmail",
    auth: "appPassword",
    password: "abcdefghijklmnop",
    imap: { host: "imap.gmail.com", port: 993, security: "tls", username: "sam.okafor@gmail.com" },
    smtp: { host: "smtp.gmail.com", port: 465, security: "tls", username: "sam.okafor@gmail.com" },
  });

  assert.equal(GMAIL_QUICK.page.url, "https://myaccount.google.com/apppasswords");
  assert.match(GMAIL_QUICK.needs ?? "", /2-Step Verification/);
  assert.match(GMAIL_QUICK.steps[GMAIL_QUICK.steps.length - 1].title, /^Copy/);
  assert.equal(
    GMAIL_QUICK_DISCLOSURE,
    "An app password gives full access to your Google account, can't be limited, and can be revoked any time in your Google Account. Sync is slower than with your own Google Cloud project.",
  );
  assert.match(WORKSPACE_NO_APP_PASSWORDS, /March 2025/);
});

test("connect errors: the server's refusal is shown as said, unreachable servers get this flow's advice", () => {
  const yahoo = "Yahoo didn't accept that password. Use an app password: https://login.yahoo.com/account/security";
  assert.equal(connectErrorText({ code: "invalidInput", message: yahoo }, "unused"), yahoo);
  assert.equal(connectErrorText({ code: "other", message: "login failed" }, "unused"), "Login failed");
  assert.equal(
    connectErrorText({ code: "network", message: "connection timed out" }, "Couldn't reach Yahoo."),
    "Couldn't reach Yahoo. (connection timed out)",
  );
  assert.equal(connectErrorText({ code: "network", message: "" }, "Couldn't reach Yahoo."), "Couldn't reach Yahoo.");
});

test("web addresses in an error become links, without the sentence's punctuation", () => {
  assert.deepEqual(linkParts("Use an app password: https://login.yahoo.com/account/security."), [
    { text: "Use an app password: " },
    { text: "https://login.yahoo.com/account/security", url: "https://login.yahoo.com/account/security" },
    { text: "." },
  ]);
  assert.deepEqual(linkParts("No link here"), [{ text: "No link here" }]);
  assert.deepEqual(
    linkParts("(see https://a.example/x) and https://b.example").map((p) => p.url ?? null),
    [null, "https://a.example/x", null, "https://b.example"],
  );
  assert.deepEqual(linkParts("http://insecure.example stays text"), [{ text: "http://insecure.example stays text" }]);
});
