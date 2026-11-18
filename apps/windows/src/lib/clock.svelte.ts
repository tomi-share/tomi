/**
 * A shared, reactive clock for countdowns ("expire dans 2 heures"). `Date.now()` alone is not reactive: a card that
 * reads it keeps showing the value of its last render. Readers call `clock.subscribe()` from an `$effect` (it returns
 * the cleanup) and read `clock.now`; the timer only runs while at least one reader is subscribed.
 */
const TICK_MS = 15_000;

let now = $state(Date.now());
let readers = 0;
let timer: ReturnType<typeof setInterval> | undefined;

function refresh(): void {
  now = Date.now();
}

export const clock = {
  get now(): number {
    return now;
  },
  subscribe(): () => void {
    readers += 1;
    // Catch up at once: the previous reader may have left a long time ago.
    refresh();
    if (readers === 1) {
      timer = setInterval(refresh, TICK_MS);
      // A suspended window or machine does not run timers: re-read when it comes back.
      document.addEventListener("visibilitychange", refresh);
      window.addEventListener("focus", refresh);
    }
    let done = false;
    return () => {
      if (done) return;
      done = true;
      readers -= 1;
      if (readers === 0) {
        clearInterval(timer);
        timer = undefined;
        document.removeEventListener("visibilitychange", refresh);
        window.removeEventListener("focus", refresh);
      }
    };
  },
};
