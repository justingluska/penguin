// Share links in the mock (src-tauri/src/share/): storage settings in memory
// (not set up at first, so the setup flow can be tried), a Test whose outcome
// follows what's typed, and uploads that take a moment and return fake links
// on share.example. Nothing leaves the browser.
//
// Test outcomes: a key ID containing "bad" → unknown key; the secret "wrong"
// → signature mismatch; the bucket "missing" → no such bucket; an endpoint
// on "offline.example" → unreachable; anything else passes.
import type { MockHandler } from "./index";
import { mailHandlers } from "./mail";
import type { LinkLifetime, ShareLinkConfig, ShareLinkConfigInput, ShareRequest, ShareTestReport, ShareTestStep } from "../types";

const HOURS: Record<LinkLifetime, number> = { "1h": 1, "24h": 24, "7d": 168 };

const empty = (): ShareLinkConfig => ({
  configured: false,
  endpoint: "",
  bucket: "",
  region: "auto",
  accessKeyId: "",
  hasSecret: false,
  lifetime: "24h",
  deleteOnExpiry: true,
  allowAgents: false,
});

let config: ShareLinkConfig = empty();
let secret: string | null = null;
const uploads = new Set<string>();

const invalid = (message: string) => ({ code: "invalidInput", message });
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** The same checks as share/config.rs validate_target, in brief. */
function check(c: ShareLinkConfigInput): string | null {
  let url: URL;
  try {
    url = new URL(c.endpoint.trim());
  } catch {
    return "The endpoint isn't a web address. It looks like https://<account id>.r2.cloudflarestorage.com";
  }
  if (url.protocol !== "https:") return "The endpoint must start with https://";
  if (url.pathname !== "/" && url.pathname !== "") return "The endpoint is only https:// and the host: put the bucket name in Bucket";
  if (!/^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/.test(c.bucket.trim()) || c.bucket.includes("..")) {
    return c.bucket.trim() ? "A bucket name is 3 to 63 lowercase letters, digits, dots and dashes" : "Enter the bucket's name";
  }
  if (c.region.trim() && !/^[a-z0-9-]{1,32}$/.test(c.region.trim())) return "The region is a name like auto or us-east-1";
  if (!c.accessKeyId.trim()) return "Enter the access key ID";
  return null;
}

function token(): string {
  const a = "abcdefghijklmnopqrstuvwxyz234567";
  let s = "";
  for (let i = 0; i < 26; i++) s += a[Math.floor(Math.random() * 32)];
  return s;
}

const keyName = (name: string) =>
  name
    .replace(/\s+/g, "-")
    .replace(/[^A-Za-z0-9._-]+/g, "_")
    .replace(/^[._-]+/, "") || "file";

function nameOf(r: ShareRequest): string {
  if (r.kind === "picture") return r.name || "image.png";
  const path = String(mailHandlers.save_attachment({ accountId: r.accountId, messageId: r.messageId, attachmentId: r.attachmentId }));
  return path.split("/").pop() || "file";
}

export const shareHandlers: Record<string, MockHandler> = {
  share_link_config_get: () => ({ ...config }),
  share_link_config_set: ({ config: input }) => {
    const c = input as ShareLinkConfigInput;
    const problem = check(c);
    if (problem) throw invalid(problem);
    const typed = c.secretAccessKey?.trim();
    if (!typed && !secret) throw invalid("Enter the secret access key");
    if (typed) secret = typed;
    config = {
      configured: true,
      endpoint: c.endpoint.trim().replace(/\/+$/, ""),
      bucket: c.bucket.trim(),
      region: c.region.trim() || "auto",
      accessKeyId: c.accessKeyId.trim(),
      hasSecret: true,
      lifetime: c.lifetime,
      deleteOnExpiry: c.deleteOnExpiry,
      allowAgents: c.allowAgents,
    };
    return { ...config };
  },
  share_link_config_clear: () => {
    config = empty();
    secret = null;
    return { ...config };
  },
  share_link_config_test: async ({ config: input }): Promise<ShareTestReport> => {
    const c = input as ShareLinkConfigInput;
    const problem = check(c);
    if (problem) throw invalid(problem);
    const typed = c.secretAccessKey?.trim() || secret;
    if (!typed) throw invalid("Enter the secret access key");
    await wait(900);
    const host = new URL(c.endpoint).host;
    const bucket = c.bucket.trim();
    const fail = (message: string): ShareTestReport => ({
      ok: false,
      steps: [
        { step: "upload", status: "failed", message: `Upload failed. ${message}` },
        ...(["link", "private", "delete"] as const).map((step): ShareTestStep => ({ step, status: "skipped", message: "Skipped: nothing was uploaded" })),
      ],
    });
    if (host.endsWith("offline.example")) return fail(`Couldn't reach ${host}. Check the endpoint (for R2 it is https://<account id>.r2.cloudflarestorage.com) and your connection.`);
    if (/bad/i.test(c.accessKeyId)) return fail("The storage doesn't know this access key ID. Check it, or make a new API token.");
    if (typed === "wrong") return fail("The secret access key doesn't match the access key ID. Paste the secret again (it is shown only once when the token is made).");
    if (bucket === "missing") return fail(`There is no bucket named ${bucket} at this endpoint.`);
    return {
      ok: true,
      steps: [
        { step: "upload", status: "ok", message: `Uploaded a small test file to ${bucket}` },
        { step: "link", status: "ok", message: "Downloaded it through a share link" },
        { step: "private", status: "ok", message: "The bucket is private: files open only through a link" },
        { step: "delete", status: "ok", message: "Deleted the test file" },
      ],
    };
  },
  share_file: async ({ request }) => {
    const r = request as ShareRequest;
    if (!config.configured) throw { code: "notConfigured", message: "Share links aren't set up. Add your storage in Settings → Share links." };
    const name = nameOf(r);
    const size = r.kind === "attachment" ? 1_842_311 : r.src.startsWith("data:") ? Math.floor(((r.src.length - r.src.indexOf(",") - 1) * 3) / 4) : 240_000;
    // A slow upload, so the progress toast shows.
    await wait(700 + Math.min(2500, size / 2000));
    const key = `penguin/${token()}/${keyName(name)}`;
    uploads.add(key);
    const seconds = HOURS[config.lifetime] * 3600;
    return {
      url: `https://share.example/${config.bucket}/${key}?X-Amz-Expires=${seconds}&X-Amz-Signature=mock`,
      expiresAt: Date.now() + seconds * 1000,
      key,
      name,
      size,
    };
  },
  share_delete: ({ key }) => {
    if (!uploads.delete(String(key))) throw { code: "notFound", message: "Penguin has no record of this upload (it may already be deleted)" };
  },
};
