// The demo mode (a debug build started with `TOMI_DEMO=1`): the in-memory stand-in core, nothing
// encrypted or sent. Nothing of this reaches a release: the screens use it only behind
// `import.meta.env.DEV`, and the Rust commands it calls are compiled in debug builds only.

import { invoke } from "@tauri-apps/api/core";

export const demo = {
  title: "Mode démonstration",
  text: "Rien n'est chiffré ni envoyé et rien n'est gardé : les contenus ne vivent qu'en mémoire sur ce PC. Simule une réception pour essayer les notifications.",
  receiveLink: "Recevoir un lien",
  receiveText: "Recevoir un texte",
  receiveMarkdown: "Recevoir du Markdown",
  receiveSecret: "Recevoir un contenu masqué",
  departureTitle: "Départ simulé",
  departureModes: {
    normal: "Serveur normal",
    offline: "Hors ligne",
    server: "Serveur en panne",
    reset: "Espace réinitialisé",
    oldRelay: "Serveur trop ancien",
    solo: "Seul appareil",
  } as Record<string, string>,
  /** Simulates an item arriving from another device. */
  receive: (kind: string) => invoke<void>("demo_receive", { kind }),
  /** "normal", "offline", "server", "reset", "oldRelay", or "solo". */
  departure: (mode: string) => invoke<void>("demo_departure", { mode }),
};
