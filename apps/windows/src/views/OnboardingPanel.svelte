<script lang="ts">
  import { onMount } from "svelte";
  import { api, type AppInfo } from "../lib/api";
  import Logo from "../lib/Logo.svelte";
  import StatusBanner from "../lib/StatusBanner.svelte";
  import { feed } from "../lib/feed.svelte";
  import { format, msg } from "../lib/i18n";
  import PairingPanel from "./PairingPanel.svelte";

  let { ondone }: { ondone: () => void } = $props();

  let info = $state<AppInfo | null>(null);
  /**
   * The pairing screens are offered to a PC that is not in a space yet. Once shown they stay,
   * so that "Ce PC fait partie de ton espace" is read before the window moves on.
   */
  let showPairing = $state(false);

  $effect(() => {
    if (feed.status && !feed.status.paired && feed.status.unavailable === null) showPairing = true;
  });

  onMount(() => {
    api.appInfo().then((i) => (info = i)).catch((e) => feed.fail(e));
  });

  async function finish() {
    const ok = await feed.run(() => api.completeOnboarding());
    if (ok) ondone();
  }
</script>

<section class="onboarding scroll">
  <Logo />
  <h1>{msg.onboarding.title}</h1>
  <p>{msg.onboarding.intro}</p>
  <StatusBanner />
  {#if showPairing}
    <div class="pairing"><PairingPanel /></div>
  {/if}
  <ol>
    {#each msg.onboarding.steps as step (step)}
      <li>{format(step, { shortcut: info?.settings.shortcut ?? "" })}</li>
    {/each}
  </ol>
  {#if info && !info.shortcutRegistered}
    <p class="warn">{msg.settings.shortcutNotRegistered}</p>
  {/if}
  <button class="primary" onclick={finish}>{msg.onboarding.done}</button>
</section>

<style>
  .onboarding {
    max-width: 520px;
    margin: var(--space-5) auto;
    padding: 0 var(--space-4);
    display: flex;
    flex-direction: column;
    gap: 12px;
    align-items: flex-start;
  }
  .pairing {
    align-self: stretch;
  }
  ol {
    margin: 0;
    padding-left: 20px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .warn {
    color: var(--error);
  }
</style>
