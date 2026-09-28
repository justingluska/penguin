// The Welcome setup's rules, kept pure for tests (tests/welcome.test.ts).
// Screens, when it opens by itself, and when the search model may start
// downloading. Rationale and sources: docs/ONBOARDING.md.

export type WelcomeScreen = "search" | "ai" | "mail" | "inbox" | "look" | "keys";

export interface ScreenInfo {
  id: WelcomeScreen;
  /** Short label for the step list. */
  label: string;
}

const ALL: ScreenInfo[] = [
  { id: "search", label: "Search" },
  { id: "ai", label: "Apple Intelligence" },
  { id: "mail", label: "Mail on this Mac" },
  { id: "inbox", label: "Inbox" },
  { id: "look", label: "Look" },
  { id: "keys", label: "Keys & alerts" },
];

/**
 * The screens, in order. The download question comes first so the model can
 * fetch while the rest is answered; Apple Intelligence is left out when the
 * Mac can't run it (null = still checking, also left out rather than
 * flashing a screen that may vanish).
 */
export function welcomeScreens(aiAvailable: boolean | null): ScreenInfo[] {
  return ALL.filter((s) => s.id !== "ai" || aiAvailable === true);
}

/**
 * Open by itself after boot? Only on a new install (the flag is false) once
 * an account exists.
 */
export function shouldAutoOpen(s: { welcomeCompleted: boolean }, accounts: number): boolean {
  return !s.welcomeCompleted && accounts > 0;
}

/**
 * Whether leaving `screen` should start the model download now: the user
 * has just seen the search-by-meaning choice and kept it on. Finishing or
 * skipping the setup opens the gate anyway (welcomeCompleted, backend).
 */
export function startsDownload(screen: WelcomeScreen, semanticOn: boolean): boolean {
  return screen === "search" && semanticOn;
}
