// Shared reactive state of a window: the item feed, the send jobs, the flash notice.

import { tick } from "svelte";
import {
  api,
  asAppError,
  events,
  type CoreStatus,
  type ItemFilter,
  type ItemView,
  type JobView,
  type RenderMode,
  type RevealedSecret,
} from "./api";
import { errorMessage, msg, relayNoticeText } from "./i18n";
import { archiveAction, type ArchiveAction } from "./itemLogic";

/** A hidden value shown on screen goes back to "Masqué" after this long, or when the window loses focus. */
const REVEAL_MS = 15_000;
/** How long a window that is not focused yet is given to get the focus back after Windows Hello. */
const FOCUS_GRACE_MS = 600;
const FOCUS_GRACE_STEP_MS = 100;
/** "Archivé · Annuler" stays long enough to be read and to be answered (Android: about 5 s). */
const UNDO_MS = 5_000;
/** Cards seen at about the same time are read together after this delay. */
const SEEN_BATCH_MS = 300;
/** A sentence about a device removed by the relay stays long enough to be read. */
const RELAY_NOTICE_MS = 9_000;

/** The snackbar. `action` is a button of the banner (the "Annuler" of an archive). */
export type Flash = {
  text: string;
  kind: "info" | "error";
  action?: { label: string; run: () => void };
};

class Feed {
  items = $state<ItemView[]>([]);
  jobs = $state<JobView[]>([]);
  filter = $state<ItemFilter>("all");
  query = $state("");
  loaded = $state(false);
  unread = $state(0);
  flash = $state<Flash | null>(null);
  /** Item to highlight (from a toast or a deep link). */
  focusId = $state<string | null>(null);
  /** Paired or not, blocked or not, store usable or not. `null` until the first answer. */
  status = $state<CoreStatus | null>(null);
  /** The demo space of the core is the active space. */
  demo = $state(false);
  /** The pairing screen may offer to enter the demo space. */
  canEnterDemo = $state(false);
  /** The key rotation after a revocation keeps failing (retried hourly): a removed device may still hold the key. */
  rotationStalled = $state(false);
  /**
   * Items the core has already erased (read once) whose values are still on screen: the
   * card stays until the user masks it again, like the Android feed.
   */
  held = $state<Record<string, ItemView>>({});
  /** The last text mode chosen ("Aperçu" or "Markdown"), shared by all the cards; `null` = Aperçu. */
  preferredMode = $state<RenderMode | null>(null);
  /** What "Afficher" showed of a hidden item, by id. Dropped when the item is masked again. */
  revealed = $state<Record<string, RevealedSecret>>({});
  /** The item of the detail screen, if one is open. */
  detailId = $state<string | null>(null);
  detail = $state<ItemView | null>(null);
  /** The image viewer: one image of an item, by SLOT (never by position in the list). */
  viewer = $state<{ itemId: string; slot: number } | null>(null);
  /** The code screen: one code file of an item, by slot. */
  code = $state<{ itemId: string; slot: number } | null>(null);
  /** The relay takes "remove a file everywhere" (otherwise only "de cet appareil" is offered). */
  removalEverywhere = $state(false);
  /** A screen, a viewer or a dialog covers the window: Escape closes that, not the popover. */
  overlays = $state(0);
  /** Dialogs drawn outside the main tree (the departure): the window behind them is inert. */
  backdrops = $state(0);

  /** A full-window screen is open: what is behind it is inert. */
  screenOpen = $derived(this.detailId !== null || this.viewer !== null || this.code !== null);
  hasOverlay = $derived(this.screenOpen || this.overlays > 0);

  /** What can be sent: this PC is paired, not blocked, and its store opens. */
  canSend = $derived(
    this.status === null ||
      (this.status.paired && this.status.space === "active" && this.status.unavailable === null),
  );

  /** The feed as displayed: the items of the core plus the ones kept on screen. */
  visible = $derived.by(() => {
    const ids = new Set(this.items.map((i) => i.id));
    const kept = Object.values(this.held).filter((i) => !ids.has(i.id));
    return [...kept, ...this.items].sort((a, b) => b.createdAt - a.createdAt);
  });

  /**
   * "Épinglés" on top, most recently pinned first (the core never reorders for a pin,
   * spec/pin-contract.md), then the others, newest first (under the "Archives" filter these
   * are the archived items). An item is in one section; a pin wins over an archive. Archived
   * items are not in "Récents": they are only listed under "Archives" or by a search
   * (spec/archive-contract.md section 3); the core already filters, this keeps a stale item out.
   */
  sections = $derived.by(() => {
    const pinned = this.visible
      .filter((i) => i.pinned)
      .sort((a, b) => (b.pinnedAt ?? 0) - (a.pinnedAt ?? 0) || b.createdAt - a.createdAt);
    const withArchived = this.filter === "archives" || this.query.trim() !== "";
    const recent = this.visible.filter((i) => !i.pinned && (withArchived || !i.archived));
    return { pinned, recent };
  });

  #seq = 0;
  #revealTimers = new Map<string, ReturnType<typeof setTimeout>>();
  #flashTimer: ReturnType<typeof setTimeout> | undefined;
  #focusTimer: ReturnType<typeof setTimeout> | undefined;
  #seenQueue = new Set<string>();
  #seenTimer: ReturnType<typeof setTimeout> | undefined;

  async refresh(): Promise<void> {
    const seq = ++this.#seq;
    try {
      const items = await api.listItems(this.filter, this.query);
      if (seq !== this.#seq) return; // a newer request is in flight
      this.items = items;
      this.unread = items.filter((i) => !i.read && !i.outgoing).length;
      this.loaded = true;
      await this.refreshDetail();
    } catch (e) {
      this.fail(e);
    }
  }

  /**
   * An incoming item stayed on screen (see `seenWatch`): it is read. The ids are gathered for a
   * moment so that cards seen together are read in one go, never a hidden one (showing it does).
   */
  markSeen(id: string): void {
    this.#seenQueue.add(id);
    this.#seenTimer ??= setTimeout(() => void this.#flushSeen(), SEEN_BATCH_MS);
  }

  async #flushSeen(): Promise<void> {
    const ids = [...this.#seenQueue];
    this.#seenQueue.clear();
    this.#seenTimer = undefined;
    for (const id of ids) await api.markRead(id).catch(() => {});
  }

  /** The window is the one the user looks at, and no screen or dialog covers the feed. */
  get feedAttended(): boolean {
    return !document.hidden && document.hasFocus() && !this.hasOverlay && this.backdrops === 0;
  }

  /** Opens the detail screen of an item (reads it again from the core, whatever the filter). */
  async openDetail(id: string): Promise<void> {
    this.detailId = id;
    await this.refreshDetail();
  }

  closeDetail(): void {
    this.detailId = null;
    this.detail = null;
  }

  async refreshDetail(): Promise<void> {
    const id = this.detailId;
    if (id === null) return;
    try {
      const view = await api.itemView(id);
      if (this.detailId === id) this.detail = view;
    } catch {
      // Gone (deleted here or elsewhere, expired): the screen shows "introuvable" or closes.
      if (this.detailId === id) this.detail = null;
    }
  }

  /** Remembers the text mode chosen on a card; every card follows it. */
  async chooseMode(mode: RenderMode): Promise<void> {
    this.preferredMode = mode;
    try {
      await api.setRenderMode(mode);
    } catch (e) {
      this.fail(e);
    }
  }

  openImage(itemId: string, slot: number): void {
    this.viewer = { itemId, slot };
    void this.refreshRemovalSupport();
  }

  openCode(itemId: string, slot: number): void {
    this.code = { itemId, slot };
  }

  /** Re-reads what the relay can do (a field of the core, not the network). Called when a screen offering the removal opens. */
  async refreshRemovalSupport(): Promise<void> {
    try {
      this.removalEverywhere = await api.attachmentRemovalEverywhere();
    } catch {
      this.removalEverywhere = false;
    }
  }

  /**
   * Shows a hidden item ("Afficher"). A read-once item WITHOUT files is consumed (erased on every
   * device) only AFTER its value is on screen: Rust does not consume it when revealing, and the
   * front asks for it (`consumeRevealed`) once the window is attended. Its card is held on screen
   * until it is masked again. If the window lost the focus, nothing is shown and nothing is
   * consumed: the item stays, and the Windows Hello window stays open. One WITH files is not
   * consumed by showing it, so it is not held.
   */
  async reveal(item: ItemView): Promise<void> {
    try {
      const shown = await api.revealSecret(item.id);
      // Windows Hello (and the core) took a moment: if the window lost the focus or was hidden in
      // the meantime, its `blur` already hid everything and fired before this value existed.
      // Storing it now would show a secret in a window nobody looks at.
      if (!(await this.#windowAttended())) return;
      this.revealed[item.id] = shown;
      if (shown.consumeAfterShow) this.hold(item);
      this.#armReveal(item.id);
      if (shown.consumeAfterShow) await api.consumeRevealed(item.id);
    } catch (e) {
      this.fail(e);
    }
  }

  /**
   * The window is the one the user looks at: visible and focused. The system gives the focus back
   * to the window a moment after the Windows Hello prompt closes, so a few short waits are allowed
   * before concluding that it really lost it.
   */
  async #windowAttended(): Promise<boolean> {
    for (let waited = 0; ; waited += FOCUS_GRACE_STEP_MS) {
      if (document.hidden) return false;
      if (document.hasFocus()) return true;
      if (waited >= FOCUS_GRACE_MS) return false;
      await new Promise((resolve) => setTimeout(resolve, FOCUS_GRACE_STEP_MS));
    }
  }

  #armReveal(id: string): void {
    clearTimeout(this.#revealTimers.get(id));
    this.#revealTimers.set(id, setTimeout(() => this.hideValues(id), REVEAL_MS));
  }

  /** Masks a hidden item again and forgets what was shown of it. */
  hideValues(id: string): void {
    clearTimeout(this.#revealTimers.get(id));
    this.#revealTimers.delete(id);
    delete this.revealed[id];
    this.release(id);
  }

  hideAllValues(): void {
    for (const id of Object.keys(this.revealed)) this.hideValues(id);
  }

  /**
   * Devices the relay removed on its own authority, said once and without blocking anything. The
   * core hands each one over ONCE, so only a window somebody looks at takes them: a hidden
   * popover leaves them for the next window that is shown.
   */
  async takeRelayNotices(): Promise<void> {
    if (document.hidden) return;
    try {
      const notices = await api.takeRelayNotices();
      if (notices.length > 0) this.say(relayNoticeText(notices), "info", RELAY_NOTICE_MS);
    } catch {
      // Nothing was taken: the next change or the next time the window is shown tries again.
    }
  }

  async refreshStatus(): Promise<void> {
    try {
      const info = await api.appInfo();
      const wasPaired = this.status?.paired === true;
      this.status = info.status;
      this.demo = info.demoEntered;
      this.canEnterDemo = info.canEnterDemo;
      this.rotationStalled = info.rotationStalled && info.status.paired;
      if (wasPaired && !info.status.paired) this.#forgetEverything();
      this.preferredMode = info.settings.renderMode;
    } catch {
      // Keep the last known state: a missing answer must not look like "not paired".
    }
  }

  /**
   * This PC left its space (or was erased): nothing of it may stay on screen, not a revealed
   * secret, a card kept after a read-once, an open detail or a viewer.
   */
  #forgetEverything(): void {
    this.hideAllValues();
    this.held = {};
    this.items = [];
    this.jobs = [];
    this.unread = 0;
    this.closeDetail();
    this.viewer = null;
    this.code = null;
  }

  setFilter(f: ItemFilter): void {
    this.filter = f;
    // A card kept on screen belongs to the view it was revealed in.
    this.held = {};
    void this.refresh();
  }

  setQuery(q: string): void {
    this.query = q;
    this.held = {};
    void this.refresh();
  }

  /**
   * A sentence in the snackbar. `ms`: how long it stays (a long sentence needs longer to read).
   * `action`: a button of the banner ("Annuler"); pressing it closes the banner.
   */
  say(text: string, kind: Flash["kind"] = "info", ms?: number, action?: Flash["action"]): void {
    this.flash = { text, kind, action };
    clearTimeout(this.#flashTimer);
    this.#flashTimer = setTimeout(() => (this.flash = null), ms ?? (kind === "error" ? 6000 : 2500));
  }

  fail(e: unknown): void {
    this.say(errorMessage(asAppError(e)), "error");
  }

  /** Runs an action and reports its failure. Returns true on success. */
  async run(action: () => Promise<unknown>, success?: string): Promise<boolean> {
    try {
      await action();
      if (success) this.say(success);
      return true;
    } catch (e) {
      this.fail(e);
      return false;
    }
  }

  /** Pins or unpins on all devices. The core answers at once; the feed follows its events. */
  async togglePin(item: ItemView): Promise<boolean> {
    const ok = await this.run(() => (item.pinned ? api.unpinItem(item.id) : api.pinItem(item.id)));
    if (ok) await this.refresh();
    return ok;
  }

  /** Closes the snackbar (its button was pressed). */
  clearFlash(): void {
    clearTimeout(this.#flashTimer);
    this.flash = null;
  }

  /**
   * Archives or unarchives on all devices (spec/archive-contract.md), then says so with an
   * "Annuler" button, like Android's "Archivé · Annuler". Never marks the item read. The core
   * unpins an item it archives, so "Annuler" gives the pin back (`wasPinned`). Returns true when
   * the core did it (the detail screen then goes back to the list).
   */
  async toggleArchive(item: ItemView): Promise<boolean> {
    const action = archiveAction(item);
    const wasPinned = item.pinned;
    try {
      await this.#applyArchive(action, item.id);
    } catch {
      this.say(msg.card.archiveFailed, "error");
      return false;
    }
    await this.refresh();
    this.say(
      action === "archive" ? msg.card.archivedToast : msg.card.unarchivedToast,
      "info",
      UNDO_MS,
      { label: msg.card.undo, run: () => void this.undoArchive(action, item.id, wasPinned) },
    );
    return true;
  }

  /**
   * "Annuler": the inverse call; if the archive had unpinned the item, the pin comes back
   * (pinning an archived item unarchives it first, so the order is free).
   */
  async undoArchive(action: ArchiveAction, id: string, wasPinned: boolean): Promise<void> {
    try {
      await this.#applyArchive(action === "archive" ? "unarchive" : "archive", id);
      if (action === "archive" && wasPinned) await api.pinItem(id);
    } catch {
      this.say(msg.card.archiveFailed, "error");
    }
    await this.refresh();
    // The item that came back is outlined for a moment, so that it is found again in the list.
    this.focus(id);
  }

  #applyArchive(action: ArchiveAction, id: string): Promise<void> {
    return action === "archive" ? api.archiveItem(id) : api.unarchiveItem(id);
  }

  /**
   * The card that takes the place of `id` in the list once it leaves it (the next one, else the
   * previous), or null: where the keyboard goes back to after a screen closed over the list.
   */
  neighborOf(id: string): string | null {
    const ids = [...this.sections.pinned, ...this.sections.recent].map((i) => i.id);
    const at = ids.indexOf(id);
    return at < 0 ? null : (ids[at + 1] ?? ids[at - 1] ?? null);
  }

  /** Puts the keyboard focus on a card of the list (its own, not the one of an open detail). */
  async focusCard(id: string | null): Promise<void> {
    if (id === null) return;
    await tick();
    document.querySelector<HTMLElement>(`[data-item-id="${CSS.escape(id)}"]:not(.expanded)`)?.focus();
  }

  focus(id: string): void {
    this.focusId = id;
    clearTimeout(this.#focusTimer);
    this.#focusTimer = setTimeout(() => (this.focusId = null), 3500);
  }

  /** Keeps a card on screen after the core erased its item (read once, values shown). */
  hold(item: ItemView): void {
    this.held[item.id] = item;
  }

  release(id: string): void {
    delete this.held[id];
  }

  #upsertJob(job: JobView): void {
    const rest = this.jobs.filter((j) => j.id !== job.id);
    // Final states are shown briefly by the job bar; the list keeps them out.
    this.jobs = job.state === "cancelled" ? rest : [...rest, job];
  }

  /** Subscribes to the Rust events. Returns the cleanup function. */
  async start(): Promise<() => void> {
    const unlisten = await Promise.all([
      events.itemsChanged(() => void this.refresh()),
      events.spaceChanged(() => void this.refreshStatus()),
      events.relayRevocations(() => void this.takeRelayNotices()),
      events.sessionLocked(() => this.hideAllValues()),
      events.jobChanged((job) => this.#upsertJob(job)),
      events.focusItem((id) => {
        this.filter = "all";
        this.query = "";
        this.held = {};
        void this.refresh().then(() => this.focus(id));
      }),
      events.notice((err) => this.say(errorMessage(err), "error")),
    ]);
    // A revealed item must not outlive the user's attention: hide it as soon as the window
    // loses focus or is hidden, not only when the 15 s timer ends.
    let watchFocus: ReturnType<typeof setInterval> | undefined;
    const stopWatching = () => {
      if (watchFocus !== undefined) clearInterval(watchFocus);
      watchFocus = undefined;
    };
    const onBlur = () => {
      if (document.activeElement?.tagName === "IFRAME") {
        // Focus moving into a text preview (an <iframe>) is not the window being left, but the
        // parent will not get another `blur` when the window really loses focus afterwards:
        // watch for it, so that a revealed secret is hidden at once.
        stopWatching();
        watchFocus = setInterval(() => {
          if (!document.hasFocus()) {
            stopWatching();
            this.hideAllValues();
          }
        }, 400);
        return;
      }
      this.hideAllValues();
    };
    const onFocus = () => stopWatching();
    const onVisibility = () => {
      if (document.hidden) this.hideAllValues();
      else void this.takeRelayNotices();
    };
    window.addEventListener("blur", onBlur);
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);
    this.jobs = await api.activeJobs().catch(() => []);
    await Promise.all([this.refresh(), this.refreshStatus()]);
    void this.takeRelayNotices();
    return () => {
      unlisten.forEach((u) => u());
      stopWatching();
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
      this.hideAllValues();
    };
  }

  dropJob(id: string): void {
    this.jobs = this.jobs.filter((j) => j.id !== id);
  }
}

export const feed = new Feed();
