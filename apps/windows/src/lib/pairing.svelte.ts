// Shared reactive state of the pairing screen (spec/pairing-v1.md): what this PC can do, and
// the pairing in progress. The pairing itself runs in the core, not in the window: this only
// reads it and is told when it changes, so a window that is closed and reopened finds the
// same screen.

import {
  api,
  events,
  type PairingOptions,
  type PairingStage,
  type PairingView,
  type PairLinkIn,
  type SetupLinkIn,
} from "./api";
import { feed } from "./feed.svelte";

const NONE: PairingView = { role: "none" };

/** Done, cancelled or failed: nothing is left to wait for. */
export const isOver = (stage: PairingStage): boolean =>
  stage === "done" || stage === "cancelled" || stage === "failed";

class Pairing {
  view = $state<PairingView>(NONE);
  options = $state<PairingOptions | null>(null);
  /**
   * A pairing link that opened the app (`tomi-share://pair#` or `https://<relay>/l/pair#`), waiting for
   * the user to press "Rejoindre". The code is a secret: it fills the form and is dropped as soon
   * as the form is gone. The relay it names is not trusted: it fills the "Relais" field, where
   * the user sees it, and is shown again with the verification code.
   */
  link = $state<PairLinkIn | null>(null);
  /** A creation link that opened the app, waiting for "Créer l'espace". Same trust as `link`. */
  setup = $state<SetupLinkIn | null>(null);

  /** A pairing is running: a code is shown, or the codes are being compared. */
  active = $derived(this.view.role !== "none" && !isOver(this.view.stage));

  async refreshView(): Promise<void> {
    try {
      this.view = await api.pairingState();
    } catch {
      // Keep what is shown: a missing answer must not make the screen vanish.
    }
    // What this PC can do changes when a pairing ends (it joined, or an interrupted one was settled).
    if (this.view.role === "none" || isOver(this.view.stage)) await this.refreshOptions();
  }

  async refreshOptions(): Promise<void> {
    try {
      this.options = await api.pairingOptions();
    } catch (e) {
      // A store that cannot be opened is told by the banner of the window.
      if (feed.canSend) feed.fail(e);
    }
  }

  async takeLink(): Promise<void> {
    try {
      const link = await api.takePairLink();
      if (link) this.link = link;
    } catch {
      // Nothing to pick up.
    }
  }

  async takeSetup(): Promise<void> {
    try {
      const setup = await api.takeSetupLink();
      if (setup) this.setup = setup;
    } catch {
      // Nothing to pick up.
    }
  }

  /** Subscribes to the Rust events and loads the screen. Returns the cleanup function. */
  async start(): Promise<() => void> {
    const unlisten = await Promise.all([
      events.pairingChanged(() => void this.refreshView()),
      events.spaceChanged(() => void this.refreshOptions()),
      events.pairLinkAvailable(() => void this.takeLink()),
      events.setupLinkAvailable(() => void this.takeSetup()),
    ]);
    await Promise.all([
      this.refreshView(),
      this.refreshOptions(),
      this.takeLink(),
      this.takeSetup(),
    ]);
    return () => unlisten.forEach((u) => u());
  }
}

export const pairing = new Pairing();
