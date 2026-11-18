<script lang="ts">
  import { onMount, type Component } from "svelte";
  import { api, asAppError, type AppInfo } from "../lib/api";
  import { departure } from "../lib/departure.svelte";
  import { feed } from "../lib/feed.svelte";
  import { errorMessage, format, fr } from "../lib/i18n/fr";
  import { theme, type ThemeChoice } from "../lib/theme.svelte";

  let info = $state<AppInfo | null>(null);
  // The demo panel exists in a debug build only: this import is dead code in a release.
  let DemoPanel = $state<Component | null>(null);
  if (import.meta.env.DEV) {
    void import("../lib/DemoPanel.svelte").then((m) => (DemoPanel = m.default));
  }
  let shortcut = $state("");
  let shortcutError = $state<string | null>(null);

  const THEMES: ThemeChoice[] = ["system", "light", "dark"];

  async function load() {
    try {
      info = await api.appInfo();
      shortcut = info.settings.shortcut;
    } catch (e) {
      feed.fail(e);
    }
  }

  onMount(() => void load());

  async function apply<T>(action: () => Promise<T>) {
    if (await feed.run(action)) await load();
  }

  async function saveShortcut() {
    shortcutError = null;
    try {
      const s = await api.setShortcut(shortcut);
      feed.say(format(fr.settings.shortcutSaved, { shortcut: s }));
      await load();
    } catch (e) {
      shortcutError = errorMessage(asAppError(e));
    }
  }
</script>

{#if info}
  <section class="panel scroll">
    <div class="inner">
    <h1 class="wide">{fr.settings.title}</h1>

    <div class="group">
      <div class="field">
        <span id="theme-label" class="label">{fr.settings.theme}</span>
        <div class="row themes" role="radiogroup" aria-labelledby="theme-label">
          {#each THEMES as c (c)}
            <button
              class="chip"
              role="radio"
              aria-checked={theme.choice === c}
              onclick={() => theme.set(c)}>{fr.settings.themes[c]}</button
            >
          {/each}
        </div>
        <span class="muted small">{fr.settings.themeHelp}</span>
      </div>
    </div>

    <div class="group">
      <div class="field">
        <label class="label" for="shortcut">{fr.settings.shortcut}</label>
        <div class="row">
          <input
            id="shortcut"
            type="text"
            class="mono"
            bind:value={shortcut}
            onkeydown={(e) => e.key === "Enter" && saveShortcut()}
          />
          <button onclick={saveShortcut} disabled={shortcut === info.settings.shortcut}>
            {fr.settings.shortcutApply}
          </button>
        </div>
        <span class="muted small">{fr.settings.shortcutHelp}</span>
        {#if shortcutError}<span class="error small" role="alert">{shortcutError}</span>{/if}
        {#if !info.shortcutRegistered}<span class="error small">{fr.settings.shortcutNotRegistered}</span>{/if}
      </div>

      <label class="check">
        <input
          type="checkbox"
          checked={info.settings.autostart}
          onchange={(e) => apply(() => api.setAutostart(e.currentTarget.checked))}
        />
        {fr.settings.autostart}
      </label>

      <label class="check">
        <input
          type="checkbox"
          checked={info.settings.notifications}
          onchange={(e) => apply(() => api.setNotifications(e.currentTarget.checked))}
        />
        {fr.settings.notifications}
      </label>
    </div>

    <div class="group">
      <div class="field">
        <span class="label">{fr.settings.downloadDir}</span>
        <div class="row">
          <span class="grow selectable ellipsis mono dir">
            {info.settings.downloadDir ?? fr.settings.downloadDirDefault}
          </span>
          {#if !info.settings.downloadDir}
            <span class="muted small tag">{fr.settings.downloadDirIsDefault}</span>
          {/if}
          <button onclick={() => apply(() => api.chooseDownloadDir())}>{fr.settings.choose}</button>
          {#if info.settings.downloadDir}
            <button class="ghost" onclick={() => apply(() => api.chooseDownloadDir(true))}>
              {fr.settings.reset}
            </button>
          {/if}
        </div>
      </div>
    </div>

    {#if feed.status?.paired}
      <div class="group">
        <div class="field">
          <span class="label">{fr.departure.sectionTitle}</span>
          <span class="muted small">{fr.departure.sectionHelp}</span>
          {#if !departure.available}
            <span class="muted small">{fr.departure.unavailable}</span>
          {/if}
        </div>
        <div class="row">
          <button
            class="danger"
            disabled={!departure.available ||
              departure.snap?.departure != null ||
              (departure.snap?.working ?? false)}
            onclick={() => void departure.openLeave()}
          >
            {fr.departure.leave}
          </button>
        </div>
      </div>
    {/if}

    {#if import.meta.env.DEV && info.demo && DemoPanel}
      <div class="wide"><DemoPanel /></div>
    {/if}

    <footer class="row wide">
      <span class="muted small grow">{format(fr.settings.about, { version: info.version })}</span>
      <button class="danger" onclick={() => api.quitApp()}>{fr.settings.quit}</button>
    </footer>
    </div>
  </section>
{/if}

<style>
  /* The section scrolls over the whole width (its scrollbar sits at the window edge); the
     content is a responsive grid: one column in a narrow window, as many as fit when wide. */
  .inner {
    width: 100%;
    max-width: 1000px;
    margin: 0 auto;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 340px), 1fr));
    align-items: start;
    gap: var(--space-4);
  }
  .wide {
    grid-column: 1 / -1;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--surface);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-md);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .label {
    font-weight: 500;
    font-size: var(--fs-label);
  }
  .themes {
    gap: var(--space-2);
  }
  .check {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    cursor: pointer;
  }
  .error {
    color: var(--error);
  }
  .dir {
    font-size: 13px;
  }
  /* "Par défaut" is a status, not part of the path: not selectable, never shrunk by the ellipsis. */
  .tag {
    flex: none;
    user-select: none;
  }
</style>
