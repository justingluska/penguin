// One mock backend for every window (developer mock and demo mode). The mock
// (src/lib/mock) is in-memory JS, so each window would otherwise have its own
// mailbox: archive in a conversation window and the main window would never
// hear of it. Instead the main window hosts it and the other windows call it
// over the window bus (lib/windowBus.ts); its events go to every window, the
// way the Rust backend's `app.emit` does.
import type { UnlistenFn } from "@tauri-apps/api/event";
import { busSend, onBus } from "./windowBus";
import { MAIN_LABEL } from "./windowRoute";

export interface MockLike {
  invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T>;
  listen(event: string, cb: (p: unknown) => void): Promise<UnlistenFn>;
}

export interface MockHostBackend {
  invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T>;
  /** Called with every event the mock emits. */
  onEmit(f: (event: string, payload: unknown) => void): void;
}

/** A command a window waits on this long before giving up on the main window. */
const CALL_TIMEOUT_MS = 60_000;

/** Errors cross as `{code, message}` (what asCommandError reads). */
function plainError(e: unknown): { code: string; message: string } {
  if (e && typeof e === "object" && "code" in e && "message" in e) {
    const x = e as { code: unknown; message: unknown };
    return { code: String(x.code), message: String(x.message) };
  }
  return { code: "other", message: e instanceof Error ? e.message : String(e) };
}

/** The main window: answer the other windows' calls and pass the mock's events on. */
export function hostMock(backend: MockHostBackend): void {
  onBus((m) => {
    if (m.type !== "mock-call") return;
    const id = m.id;
    backend
      .invoke<unknown>(String(m.cmd), (m.args as Record<string, unknown>) ?? {})
      .then(
        (value) => busSend(m.from, "mock-result", { id, ok: true, value }),
        (e) => busSend(m.from, "mock-result", { id, ok: false, error: plainError(e) }),
      )
      .catch((e) => console.warn("penguin: mock reply failed", e));
  });
  backend.onEmit((event, payload) => void busSend("*", "mock-event", { event, payload }));
}

/** Another window: the mock, reached through the main window. */
export function remoteMock(): MockLike {
  let seq = 0;
  const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: unknown) => void; timer: ReturnType<typeof setTimeout> }>();
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  onBus((m) => {
    if (m.type === "mock-result") {
      const p = pending.get(Number(m.id));
      if (!p) return;
      pending.delete(Number(m.id));
      clearTimeout(p.timer);
      if (m.ok) p.resolve(m.value);
      else p.reject(m.error);
    } else if (m.type === "mock-event") {
      listeners.get(String(m.event))?.forEach((cb) => cb(m.payload));
    }
  });
  return {
    invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
      const id = ++seq;
      return new Promise<T>((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject({ code: "other", message: "The main Penguin window didn't answer" });
        }, CALL_TIMEOUT_MS);
        pending.set(id, { resolve: resolve as (v: unknown) => void, reject, timer });
        busSend(MAIN_LABEL, "mock-call", { id, cmd, args }).catch((e) => {
          pending.delete(id);
          clearTimeout(timer);
          reject(plainError(e));
        });
      });
    },
    async listen(event, cb) {
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event)!.add(cb);
      return () => listeners.get(event)?.delete(cb);
    },
  };
}
