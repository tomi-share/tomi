<script lang="ts">
  import { api } from "./api";
  import { feed } from "./feed.svelte";
  import { fr } from "./i18n/fr";
  import Icon from "./Icon.svelte";

  /** In the popover: a button leads to the Appareils tab. */
  let { link = false }: { link?: boolean } = $props();

  // The removal of a device is not finished: the key rotation that follows it keeps failing and
  // the core retries every hour. Calm on purpose (an info banner, not an error): the user can do
  // little but open Tomi elsewhere. Nothing at all when it is not the case, or when this PC has
  // left or lost its space (the other banners speak then).
  const shown = $derived(
    feed.rotationStalled && feed.status?.paired === true && feed.status.space === "active",
  );
</script>

{#if shown}
  <div class="banner row" role="status">
    <Icon name="info" size={20} />
    <span class="grow">
      <strong>{fr.status.rotationStalledTitle}</strong>
      <span class="text">{fr.status.rotationStalled}</span>
    </span>
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
  .text {
    display: block;
    color: var(--muted);
  }
</style>
