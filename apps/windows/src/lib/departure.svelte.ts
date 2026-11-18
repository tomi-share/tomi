// The departure of this PC (leave the space, delete it, erase without proof), as the windows see
// it: a snapshot read from Rust and the clicks sent back. Every rule that protects the user
// (Windows Hello, the 2 s anti-tap, "Effacer quand même" only when the core allows it) is
// enforced in Rust (`departure.rs`); nothing here decides what is a proof.

import { api, asAppError, events, type Cancelled, type DepartureSnapshot } from "./api";
import { feed } from "./feed.svelte";
import { errorMessage, msg } from "./i18n";

/** How long the sentence after "Annuler" stays on screen (it is long, and it matters). */
const CANCEL_SAY_MS = 9000;

/**
 * The label of this window, as Rust knows it (`popover` or `history`): the page of the full
 * window is loaded with `?view=history`, the popover without.
 */
const THIS_WINDOW = new URLSearchParams(globalThis.location?.search ?? "").get("view") === "history"
  ? "history"
  : "popover";

class Departure {
  snap = $state<DepartureSnapshot | null>(null);
  /** The sentence of the last refusal of Windows Hello or of the core, shown in the dialog. */
  dialogError = $state<string | null>(null);

  /**
   * The dialog to draw in this window, if any. Both windows (popover, history) hear the same
   * events; Rust stamps each dialog with the window of the gesture that opened it
   * (`ui.window`, the one source of truth) and only that window draws it, so there is never
   * one on top of the other.
   */
  screen = $derived(
    this.snap && this.snap.ui.window === THIS_WINDOW ? this.snap.ui.screen : "hidden",
  );

  /** The core can do a departure: otherwise the entries are disabled with a note. */
  available = $derived(this.snap?.available ?? false);

  #take(snap: DepartureSnapshot): void {
    const before = this.snap?.ui.screen;
    this.snap = snap;
    if (snap.ui.screen === "hidden") {
      this.dialogError = null;
    } else if (snap.ui.screen !== before) {
      // A new dialog (a refusal reopened one): its own error line starts empty.
      this.dialogError = null;
    }
  }

  async refresh(): Promise<void> {
    try {
      this.#take(await api.departureState());
    } catch {
      // Keep the last snapshot: a missing answer must not hide a pending departure.
    }
  }

  /** Runs a command that answers a snapshot. A failure is shown in the dialog if one is open. */
  async #run(command: () => Promise<DepartureSnapshot>): Promise<boolean> {
    this.dialogError = null;
    try {
      this.#take(await command());
      return true;
    } catch (e) {
      const message = errorMessage(asAppError(e));
      if (this.screen !== "hidden") this.dialogError = message;
      else feed.say(message, "error");
      await this.refresh();
      return false;
    }
  }

  // ---- the dialogs ----

  /** Menu entry "Se déconnecter". */
  async openLeave(): Promise<void> {
    await this.#run(() => api.departureOpenLeave());
  }

  confirmLeave = () => this.#run(() => api.departureConfirmLeave());
  confirmDelete = () => this.#run(() => api.departureConfirmDelete());
  confirmErase = () => this.#run(() => api.departureConfirmErase());
  setEraseChecked = (checked: boolean) => this.#run(() => api.departureSetEraseChecked(checked));

  /** "Effacer quand même", from the banner or from the refusal "serveur trop ancien". */
  async openErase(): Promise<void> {
    await this.#run(() => api.departureOpenErase());
  }

  async dismiss(): Promise<void> {
    await this.#run(() => api.departureDismiss());
  }

  // ---- the banner ----

  /** "Réessayer": a gesture. A refusal may reopen the dialog it belongs to, in this window. */
  async retry(): Promise<void> {
    await this.#run(() => api.departureRetry());
  }

  /** "Annuler" on the banner: immediate, never erases. */
  cancel = () => this.#cancel(() => api.departureCancel());

  /** "Annuler" in a dialog whose request is on its way: the dialog closes, the core stops waiting. */
  cancelRunning = () => this.#cancel(() => api.departureCancelRunning());

  async #cancel(command: () => Promise<Cancelled>): Promise<void> {
    try {
      const done = await command();
      if (done.cancelled) {
        // Long sentences: they stay long enough to be read.
        feed.say(
          done.kind === "deleteSpace" ? msg.departure.cancelledDelete : msg.departure.cancelledLeave,
          "info",
          CANCEL_SAY_MS,
        );
      }
    } catch (e) {
      feed.fail(e);
    }
    await this.refresh();
    await feed.refreshStatus();
  }

  // ---- life of the window ----

  /** Subscribes to the events of the departure. Returns the cleanup function. */
  async start(): Promise<() => void> {
    const unlisten = await Promise.all([
      events.departureChanged(() => void this.refresh()),
      // A pending departure is looked at again when the window comes back (Rust: at most once a
      // minute, and only if no series of attempts is already working on it).
      events.popoverShown(() => this.#poke()),
    ]);
    // The window gets the focus back, or the network comes back (the webview's `online` event):
    // one attempt, whatever the number of windows or events (Rust: at most once a minute).
    const onFocus = () => this.#poke();
    window.addEventListener("focus", onFocus);
    window.addEventListener("online", onFocus);
    await this.refresh();
    return () => {
      unlisten.forEach((u) => u());
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("online", onFocus);
    };
  }

  #poke(): void {
    if (this.snap?.departure) void api.departurePoke().catch(() => {});
  }
}

export const departure = new Departure();
