<script lang="ts">
  import { api } from "./api";
  import { departure } from "./departure.svelte";
  import { feed } from "./feed.svelte";
  import { fr } from "./i18n/fr";
  import Icon from "./Icon.svelte";

  /** In the popover: a button leads to the Appareils tab. */
  let { link = false }: { link?: boolean } = $props();

  const state = $derived.by((): { text: string; tone: "info" | "error" | "waiting" } | null => {
    const s = feed.status;
    if (!s) return null;
    if (s.unavailable) {
      return { text: fr.status.storageCodes[s.unavailable] ?? fr.status.storage, tone: "error" };
    }
    if (!s.paired) return { text: fr.status.notPaired, tone: "info" };
    // When this PC can leave its space, the departure banner owns "retiré" and "introuvable"
    // (it has the button that goes with them); otherwise they are only told here.
    const departureBanner = departure.available;
    switch (s.space) {
      case "revoked":
        return departureBanner ? null : { text: fr.status.revoked, tone: "error" };
      case "notFound":
        return departureBanner ? null : { text: fr.status.notFound, tone: "error" };
      case "updateRequired":
        return { text: fr.status.updateRequired, tone: "waiting" };
      case "blocked":
        return { text: fr.status.blocked, tone: "error" };
      default:
        return null;
    }
  });
</script>

{#if feed.demo}
  <div class="banner row" role="status">
    <Icon name="info" size={20} />
    <span class="grow">{fr.status.demo}</span>
    <button onclick={() => feed.run(() => api.leaveDemo())}>{fr.status.demoLeave}</button>
  </div>
{/if}
{#if state}
  <div class="banner row" class:error={state.tone === "error"} class:waiting={state.tone === "waiting"} role="status">
    <Icon name={state.tone === "error" ? "error" : state.tone === "waiting" ? "schedule" : "info"} size={20} />
    <span class="grow">{state.text}</span>
    {#if link}
      <button onclick={() => api.openWindow("devices")}>{fr.status.seeDevices}</button>
    {/if}
  </div>
{/if}

<style>
  .banner {
    background: var(--surface-alt);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-2) var(--space-3);
    align-items: flex-start;
    color: var(--text);
  }
  .banner :global(.icon) {
    margin-top: 1px;
    color: var(--muted);
  }
  .banner.waiting {
    background: var(--notice-bg);
  }
  .banner.waiting :global(.icon) {
    color: var(--secondary);
  }
  .banner.error {
    background: var(--error-bg);
    border-color: var(--error);
  }
  .banner.error :global(.icon) {
    color: var(--error);
  }
</style>