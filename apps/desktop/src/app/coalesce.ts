// Turns a stream of "something changed" events into as few re-reads as keep
// the screen current. A backfill emits mail-changed for every batch it
// commits (several per second on a fast IMAP server); re-reading the list
// (up to 1,000 rows over IPC, parsed and diffed on the main thread) for each
// one competes with the keys you press while it runs.
//
// - The first event after a quiet spell runs `run` after `delay` (the burst
//   that one sync step emits lands in one read).
// - At most one `run` is in flight; events during it queue one follow-up.
// - Runs start at least `minGap` apart, so a steady storm costs a few reads a
//   second however fast the events come. New mail then shows up at most
//   `minGap` later than it would have; your own actions never wait on this
//   (they change the list optimistically, app/store.ts).

export interface Timers {
  setTimeout: (f: () => void, ms: number) => unknown;
  clearTimeout: (t: unknown) => void;
  now: () => number;
}

const realTimers: Timers = {
  setTimeout: (f, ms) => setTimeout(f, ms),
  clearTimeout: (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
  now: () => performance.now(),
};

export interface Coalescer {
  /** Something changed: make sure a run follows. */
  request(): void;
  /** Stop: cancel a pending run and ignore later requests. */
  dispose(): void;
}

export function coalesce(run: () => Promise<unknown>, { delay = 60, minGap = 250 } = {}, timers: Timers = realTimers): Coalescer {
  let timer: unknown = null;
  let running = false;
  let again = false;
  let disposed = false;
  let lastStart = -Infinity;

  const fire = () => {
    timer = null;
    if (disposed) return;
    running = true;
    lastStart = timers.now();
    void run().finally(() => {
      running = false;
      if (again) {
        again = false;
        schedule();
      }
    });
  };

  const schedule = () => {
    if (disposed) return;
    if (running) {
      again = true;
      return;
    }
    if (timer !== null) return;
    const wait = Math.max(delay, lastStart + minGap - timers.now());
    timer = timers.setTimeout(fire, wait);
  };

  return {
    request: schedule,
    dispose() {
      disposed = true;
      if (timer !== null) timers.clearTimeout(timer);
      timer = null;
    },
  };
}
