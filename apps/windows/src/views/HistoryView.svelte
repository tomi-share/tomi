<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { events, type ItemFilter } from "../lib/api";
  import Composer from "../lib/Composer.svelte";
  import DepartureBanner from "../lib/DepartureBanner.svelte";
  import FeedList from "../lib/FeedList.svelte";
  import Icon from "../lib/Icon.svelte";
  import JobBar from "../lib/JobBar.svelte";
  import Logo from "../lib/Logo.svelte";
  import StatusBanner from "../lib/StatusBanner.svelte";
  import { feed } from "../lib/feed.svelte";
  import { fr } from "../lib/i18n/fr";
  import DevicesPanel from "./DevicesPanel.svelte";
  import OnboardingPanel from "./OnboardingPanel.svelte";
  import SettingsPanel from "./SettingsPanel.svelte";

  type Tab = "history" | "settings" | "devices" | "onboarding";
  const TABS: Tab[] = ["history", "settings", "devices"];
  const FILTERS: ItemFilter[] = ["all", "links", "text", "files", "secrets", "archives"];

  let { initialTab = null }: { initialTab?: string | null } = $props();

  const isTab = (t: string | null | undefined): t is Tab =>
    t === "history" || t === "settings" || t === "devices" || t === "onboarding";

  // The URL only picks the first tab; later changes come from the "navigate" event.
  /** The composer is open above the feed (Android's "Envoyer" button). */
  let composing = $state(false);

  let tab = $state<Tab>(untrack(() => (isTab(initialTab) ? initialTab : "history")));

  onMount(() => {
    let stop: (() => void) | undefined;
    let unNavigate: (() => void) | undefined;
    let gone = false;
    void (async () => {
      stop = await feed.start();
      unNavigate = await events.navigate((t) => {
        if (isTab(t)) tab = t;
      });
      if (gone) {
        stop();
        unNavigate();
      }
    })();
    return () => {
      gone = true;
      stop?.();
      unNavigate?.();
    };
  });
</script>

<div class="window">
  {#if tab !== "onboarding"}
    <header class="top">
      <div class="brand row">
        <Logo />
        <span class="name">{fr.appName}</span>
      </div>
      <nav class="tabs row" aria-label={fr.sectionsLabel}>
        {#each TABS as t (t)}
          <button
            class="tab"
            class:active={tab === t}
            aria-current={tab === t ? "page" : undefined}
            onclick={() => (tab = t)}>{fr.tabs[t]}</button
          >
        {/each}
      </nav>
    </header>
    <!-- On every tab but the welcome screen: a pending departure is never hidden by a tab. -->
    <div class="departure-slot"><DepartureBanner /></div>
  {/if}

  <main class="content">
    {#if tab === "history"}
      <div class="tools">
        <div class="search">
          <span class="search-icon"><Icon name="search" size={20} /></span>
          <input
            type="search"
            placeholder={fr.popover.search}
            aria-label={fr.popover.search}
            value={feed.query}
            oninput={(e) => feed.setQuery(e.currentTarget.value)}
            onkeydown={(e) => {
              // The keyboard "back" of Android's search: Escape closes it and empties the query.
              if (e.key === "Escape" && feed.query !== "") {
                e.preventDefault();
                feed.setQuery("");
              }
            }}
          />
        </div>
        <div class="row filters" role="group" aria-label={fr.filtersLabel}>
          {#each FILTERS as f (f)}
            <button
              class="chip"
              class:secret={f === "secrets"}
              aria-pressed={feed.filter === f}
              onclick={() => feed.setFilter(f)}
            >
              {#if f === "secrets"}<Icon name="visibility_off" size={16} />{:else if f === "archives"}<Icon name="archive" size={16} />{/if}
              {fr.filters[f]}
            </button>
          {/each}
        </div>
        {#if feed.canSend}
          <button class="primary send" aria-expanded={composing} onclick={() => (composing = !composing)}>
            <Icon name="send" size={20} />
            {fr.composer.send}
          </button>
        {/if}
      </div>
      <StatusBanner />
      <section class="list scroll">
        {#if composing && feed.canSend}
          <div class="compose"><Composer /></div>
        {/if}
        <JobBar />
        <FeedList wide />
      </section>
    {:else if tab === "settings"}
      <SettingsPanel />
    {:else if tab === "devices"}
      <DevicesPanel />
    {:else}
      <OnboardingPanel ondone={() => (tab = "history")} />
    {/if}
  </main>
</div>

<style>
  .window {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .top {
    display: flex;
    align-items: center;
    gap: var(--space-5);
    padding: var(--space-2) var(--space-4);
    border-bottom: var(--border-width) solid var(--border);
    background: var(--bg);
  }
  .brand {
    gap: var(--space-2);
  }
  .name {
    font-size: var(--fs-subtitle);
    font-weight: 600;
    letter-spacing: 0.01em;
  }
  .tabs {
    gap: var(--space-1);
  }
  .tab {
    border: 0;
    background: transparent;
    border-radius: var(--radius-full);
    padding: 0 var(--space-4);
    color: var(--muted);
  }
  .tab:hover:not(:disabled) {
    background: var(--hover);
    color: var(--text);
  }
  .tab.active {
    background: var(--brand-container);
    color: var(--brand);
  }
  .departure-slot {
    padding: var(--space-2) var(--space-4) 0;
  }
  .departure-slot:empty {
    display: none;
  }
  .content {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: var(--space-3) var(--space-4);
    gap: var(--space-3);
  }
  .tools {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-3);
  }
  .search {
    position: relative;
    flex: 0 1 300px;
    min-width: 200px;
  }
  .search-icon {
    position: absolute;
    left: var(--space-2);
    top: 50%;
    transform: translateY(-50%);
    display: inline-flex;
    color: var(--muted);
    pointer-events: none;
  }
  .search input {
    padding-left: 36px;
    border-radius: var(--radius-full);
  }
  .filters {
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .send {
    margin-left: auto;
  }
  .list {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding-bottom: var(--space-4);
  }
  .list > :global(*) {
    flex-shrink: 0;
  }
  .compose {
    max-width: 640px;
  }
</style>