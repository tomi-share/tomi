// Light / dark theme: follows the system, or the setting "Thème" (Système / Clair / Sombre).
//
// The choice is a display preference, not a secret: it lives in the webview's localStorage, which
// the two windows (popover and history) share, so a change in one reaches the other through the
// `storage` event. The resolved theme is `data-theme` on <html>; app.css holds the tokens.

export type ThemeChoice = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

const KEY = "tomi.theme";

function read(): ThemeChoice {
  try {
    const v = localStorage.getItem(KEY);
    return v === "light" || v === "dark" ? v : "system";
  } catch {
    return "system";
  }
}

class Theme {
  choice = $state<ThemeChoice>(read());
  /** What the system asks for right now. */
  #system = $state<ResolvedTheme>("light");

  /** The theme in use. */
  resolved = $derived<ResolvedTheme>(this.choice === "system" ? this.#system : this.choice);
  dark = $derived(this.resolved === "dark");

  #started = false;

  /** Applies the theme and keeps it up to date. Call once, before the first render. */
  start(): void {
    if (this.#started) return;
    this.#started = true;
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    this.#system = query.matches ? "dark" : "light";
    query.addEventListener("change", (e) => {
      this.#system = e.matches ? "dark" : "light";
      this.#apply();
    });
    window.addEventListener("storage", (e) => {
      if (e.key === KEY || e.key === null) {
        this.choice = read();
        this.#apply();
      }
    });
    this.#apply();
  }

  set(choice: ThemeChoice): void {
    this.choice = choice;
    try {
      if (choice === "system") localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, choice);
    } catch {
      // The choice then lasts for this session only.
    }
    this.#apply();
  }

  #apply(): void {
    document.documentElement.dataset.theme = this.resolved;
  }
}

export const theme = new Theme();
