// Google Cloud console pages the setup guide opens. Every link after project
// creation takes the optional Project ID so it lands in the right project
// instead of whichever one the console last had selected.

const CONSOLE = "https://console.cloud.google.com";

function withProject(path: string, projectId: string): string {
  return projectId ? `${CONSOLE}${path}?project=${encodeURIComponent(projectId)}` : `${CONSOLE}${path}`;
}

export const links = {
  createProject: () => `${CONSOLE}/projectcreate`,
  gmailApi: (p: string) => withProject("/apis/library/gmail.googleapis.com", p),
  branding: (p: string) => withProject("/auth/branding", p),
  audience: (p: string) => withProject("/auth/audience", p),
  createClient: (p: string) => withProject("/auth/clients/create", p),
  clients: (p: string) => withProject("/auth/clients", p),
  quotas: (p: string) => withProject("/apis/api/gmail.googleapis.com/quotas", p),
  billing: (p: string) => withProject("/billing/linkedaccount", p),
  adminAppAccess: () => "https://admin.google.com/ac/owl/list?tab=configuredApps",
};

const PROJECT_KEY = "penguin.gcpProject";

/** The Cloud project from setup, kept after setup so Settings links can use it. */
export function rememberedProjectId(): string {
  try {
    return localStorage.getItem(PROJECT_KEY) ?? "";
  } catch {
    return "";
  }
}

export function rememberProjectId(id: string): void {
  try {
    if (id) localStorage.setItem(PROJECT_KEY, id);
    else localStorage.removeItem(PROJECT_KEY);
  } catch {
    // Storage blocked: links just open the console's current project.
  }
}

/** Suggested names; any name works. */
export const NAMES = { project: "Penguin", app: "Penguin", client: "Penguin Mac" };

/**
 * A Project ID as typed, or pulled out of a pasted console URL (…?project=id).
 * Returns "" for empty input and null when it can't be a project ID.
 */
export function parseProjectId(input: string): string | null {
  let v = input.trim();
  if (!v) return "";
  const fromUrl = /[?&]project=([^&#\s]+)/.exec(v);
  if (fromUrl) v = decodeURIComponent(fromUrl[1]);
  v = v.toLowerCase();
  // Google: 6–30 chars, lowercase letters, digits and hyphens, starts with a
  // letter, doesn't end with a hyphen. Older projects can have a domain prefix.
  return /^([a-z0-9.-]+:)?[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(v) ? v : null;
}
