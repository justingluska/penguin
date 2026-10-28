// Settings → Accounts → Pair iPhone, simulated: the code "is scanned" after a
// few seconds by a fictional iPhone, which already has the first account;
// every other account (Gmail, IMAP, Microsoft) moves except one this Mac has
// no sign-in for.
// ?mockPairFail=1 makes the phone never come (the code expires after 20 s).
import type { Account, PairAccount, PairEvent, PairStart } from "../types";
import { mockBackend, type MockHandler } from "./index";
import { mailHandlers } from "./mail";

const EVENT = "penguin://pair-iphone";
let current: { id: string; timers: ReturnType<typeof setTimeout>[]; accounts: PairAccount[] } | null = null;

function emit(e: PairEvent) {
  mockBackend.emit(EVENT, e);
}

function stop() {
  current?.timers.forEach(clearTimeout);
  current = null;
}

/** A plausible-looking QR pattern (not a real code: nothing scans the mock). */
function fakeQr(size: number): string {
  let path = "";
  let seed = 7;
  const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
  const finder = (x: number, y: number) => `M${x} ${y}h7v7h-7zM${x + 1} ${y + 1}v5h5v-5zM${x + 2} ${y + 2}h3v3h-3z`;
  path += finder(0, 0) + finder(size - 7, 0) + finder(0, size - 7);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const inFinder = (x < 8 && y < 8) || (x >= size - 8 && y < 8) || (x < 8 && y >= size - 8);
      if (!inFinder && rnd() < 0.48) path += `M${x} ${y}h1v1h-1z`;
    }
  }
  return path;
}

function describe(a: Account, i: number): PairAccount {
  const signedOut = i === 2;
  return {
    email: a.email,
    name: a.nickname ?? a.displayName ?? null,
    color: a.color,
    status: signedOut ? "signIn" : "token",
    reason: signedOut ? "This Mac has no sign-in for it right now." : null,
  };
}

export const pairIphoneHandlers: Record<string, MockHandler> = {
  pair_iphone_start: async (): Promise<PairStart> => {
    stop();
    const all = (await mailHandlers.list_accounts({})) as Account[];
    const id = Math.random().toString(16).slice(2);
    const accounts = all.map(describe);
    const fail = typeof location !== "undefined" && new URLSearchParams(location.search).get("mockPairFail") === "1";
    const ttl = fail ? 20_000 : 120_000;
    current = { id, timers: [], accounts };
    const session = current;
    if (fail) {
      session.timers.push(setTimeout(() => {
        emit({ state: "failed", sessionId: id, expired: true, message: "The pairing code expired. Start again on your Mac (Settings → Accounts → Pair iPhone)." });
        stop();
      }, ttl));
    } else {
      session.timers.push(setTimeout(() => {
        emit({
          state: "connected",
          sessionId: id,
          phone: { name: "iPhone", model: "iPhone17,1", system: "iOS 26.0", appVersion: "ios-1.0" },
          code: "482 193",
          accounts: accounts.map((a, i) => (i === 0 ? { ...a, status: "alreadyOnPhone", reason: null } : a)),
          clientNote: null,
        });
      }, 3500));
    }
    return {
      sessionId: id,
      qr: { size: 33, path: fakeQr(33) },
      expiresAt: Date.now() + ttl,
      hosts: ["192.168.1.20", "Mock-Mac.local"],
      port: 50123,
      accounts,
      macClientId: "123456789012-mock.apps.googleusercontent.com",
    };
  },
  pair_iphone_confirm: ({ sessionId, send }) => {
    if (!current || current.id !== sessionId) throw { code: "notFound", message: "That pairing has ended. Start again." };
    const session = current;
    if (!send) {
      emit({ state: "declined", sessionId });
      stop();
      return;
    }
    emit({ state: "sending", sessionId });
    session.timers.push(setTimeout(() => {
      const moved = session.accounts.slice(1).filter((a) => a.status === "token").map((a) => a.email);
      const signIn = session.accounts.filter((a) => a.status === "signIn").map((a) => a.email);
      emit({ state: "done", sessionId, phoneName: "iPhone", imported: moved, failed: [], signIn });
      stop();
    }, 1200));
  },
  pair_iphone_cancel: ({ sessionId }) => {
    if (current?.id === sessionId) stop();
  },
};
