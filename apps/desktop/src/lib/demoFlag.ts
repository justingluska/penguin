// Demo mode's flag and storage, without side effects (src/lib/demo.ts applies
// them at boot; tests/demoMode.test.ts covers them). Demo mode runs the real app
// on the mock backend (src/lib/mock) with fictional data, for screenshots: see
// docs/DEMO.md.

/** localStorage key of the per-device flag. The value "1" means on. */
export const DEMO_KEY = "penguin.demoMode";

type Read = Pick<Storage, "getItem">;
type Write = Pick<Storage, "setItem" | "removeItem">;

/** Whether this device is in demo mode. Unreadable storage means no. */
export function readDemoFlag(storage: Read | null): boolean {
  try {
    return storage?.getItem(DEMO_KEY) === "1";
  } catch {
    return false;
  }
}

/** Persist the flag. Throws when storage refuses: the mode must not flip silently half-way. */
export function writeDemoFlag(storage: Write | null, on: boolean): void {
  if (!storage) throw new Error("This window has no local storage, so demo mode can't be saved.");
  if (on) storage.setItem(DEMO_KEY, "1");
  else storage.removeItem(DEMO_KEY);
}

/**
 * A Storage that lives in memory. In demo mode it stands in for localStorage,
 * so the UI's per-device state (recent and saved searches, snippets, the
 * onboarding resume point, layout, calendar view) starts empty and never reads
 * or overwrites the real one.
 */
export class MemoryStorage implements Storage {
  private map = new Map<string, string>();
  get length() {
    return this.map.size;
  }
  clear() {
    this.map.clear();
  }
  getItem(key: string) {
    return this.map.has(key) ? this.map.get(key)! : null;
  }
  key(i: number) {
    return [...this.map.keys()][i] ?? null;
  }
  removeItem(key: string) {
    this.map.delete(key);
  }
  setItem(key: string, value: string) {
    this.map.set(key, String(value));
  }
}

/**
 * Put a MemoryStorage in place of `target.localStorage` (window.localStorage
 * is a configurable own property of the global object, so every bare
 * `localStorage` after this sees the stand-in). Returns whether it took.
 */
export function shadowLocalStorage(target: object, storage: Storage = new MemoryStorage()): boolean {
  try {
    Object.defineProperty(target, "localStorage", { value: storage, configurable: true, enumerable: true, writable: false });
    return (target as { localStorage?: Storage }).localStorage === storage;
  } catch {
    return false;
  }
}

export interface SwitchDeps {
  /** The real localStorage (captured before any shadowing). */
  storage: Write | null;
  /** A message is counting down to send (Undo send): switching now would drop it. */
  sendPending: () => boolean;
  /** Save open drafts before the page goes away. */
  flushDrafts: () => Promise<void>;
  reload: () => void;
}

export type SwitchResult = "reloading" | "unchanged" | "send-pending";

/**
 * Turn demo mode on or off: save open drafts, persist the flag, reload onto the
 * other backend. Refuses while a send is counting down. Throws when the flag
 * can't be saved (nothing reloads then).
 */
export async function switchDemoMode(on: boolean, current: boolean, deps: SwitchDeps): Promise<SwitchResult> {
  if (on === current) return "unchanged";
  if (deps.sendPending()) return "send-pending";
  await deps.flushDrafts();
  writeDemoFlag(deps.storage, on);
  deps.reload();
  return "reloading";
}
