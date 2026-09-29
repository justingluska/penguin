// "Copy debug info": one block to paste into a bug report or a chat. App,
// platform, accounts (address, provider, sync phase), the sign-in clients
// in use (client IDs aren't secrets) and the log's recent problems. Never
// tokens, passwords or mail content; the log itself holds none.
import { api } from "./api";
import type { SemanticIndexStatus } from "./types";
import { parseLog, isProblem } from "../features/logs/parse";

const RECENT_PROBLEMS = 40;

async function settle<T>(p: Promise<T>): Promise<T | string> {
  try {
    return await p;
  } catch (e) {
    return `unavailable (${e instanceof Error ? e.message : String((e as { message?: string })?.message ?? e)})`;
  }
}

export async function debugInfo(): Promise<string> {
  const [diag, oauth, accounts, sync, log, semantic] = await Promise.all([
    settle(api.diagnostics()),
    settle(api.oauthClientStatus()),
    settle(api.listAccounts()),
    settle(api.syncStatus()),
    settle(api.readLog()),
    settle(api.semanticStatus()),
  ]);
  const lines: string[] = ["Penguin debug info", `when: ${new Date().toISOString()}`];
  lines.push(`app: ${typeof diag === "string" ? diag : diag.appVersion}`);
  if (typeof diag !== "string" && diag.osVersion) lines.push(`os: ${diag.osVersion}`);
  lines.push(`platform: ${navigator.platform} · ${navigator.userAgent}`);
  lines.push(`screen: ${window.innerWidth}x${window.innerHeight} @${window.devicePixelRatio}x`);
  if (typeof oauth === "string") lines.push(`google clients: ${oauth}`);
  else {
    lines.push(`google desktop client configured: ${oauth.configured}`);
    lines.push(`google iOS client: ${oauth.iosClientId ?? "none"}`);
  }
  if (typeof accounts === "string") lines.push(`accounts: ${accounts}`);
  else {
    lines.push(`accounts (${accounts.length}):`);
    const phases = typeof sync === "string" ? [] : sync;
    for (const a of accounts) {
      const s = phases.find((x) => x.accountId === a.id);
      lines.push(`  - ${a.email} · ${a.provider ?? "gmail"} · ${s ? `${s.phase}, ${s.indexed} indexed${s.failure ? `, ${s.failure.count} failed tries since ${new Date(s.failure.firstAt).toISOString()} (${s.failure.kind}${s.failure.alert ? ", shown" : ", retrying quietly"})` : ""}${s.error ? `, error: ${s.error}` : ""}` : "no sync status"}`);
    }
  }
  lines.push(`search by meaning: ${semanticDebugLine(semantic)}`);
  if (typeof log === "string") lines.push(`log: ${log}`);
  else {
    const problems = parseLog(log.lines).filter(isProblem).slice(-RECENT_PROBLEMS);
    lines.push(`recent problems (${problems.length}):`);
    for (const e of problems) {
      const at = e.at === null ? "" : new Date(e.at).toISOString();
      lines.push(`  ${at} ${e.level} ${e.target}: ${e.text.split("\n")[0].slice(0, 300)}`);
    }
  }
  return lines.join("\n");
}

/** One line for the search-by-meaning index: state, model, progress. */
export function semanticDebugLine(s: SemanticIndexStatus | string): string {
  if (typeof s === "string") return s;
  const parts = [s.state, s.modelId, `${s.indexed}/${s.total} messages`, `${s.chunks} vectors`];
  if (s.state === "downloading") parts.push(`download ${s.downloadDone}/${s.downloadTotal} bytes`);
  if (s.pausedReason) parts.push(`paused: ${s.pausedReason}`);
  if (s.rate > 0) parts.push(`${s.rate.toFixed(1)} msg/s`);
  if (s.error) parts.push(`error: ${s.error}`);
  return parts.join(" · ");
}
