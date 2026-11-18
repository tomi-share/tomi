<script lang="ts">
  import { onMount, tick } from "svelte";
  import { api, events, type ItemFilter } from "../lib/api";
  import Composer from "../lib/Composer.svelte";
  import DepartureBanner from "../lib/DepartureBanner.svelte";
  import FeedList from "../lib/FeedList.svelte";
  import Icon from "../lib/Icon.svelte";
  import JobBar from "../lib/JobBar.svelte";
  import Logo from "../lib/Logo.svelte";
  import StatusBanner from "../lib/StatusBanner.svelte";
  import { feed } from "../lib/feed.svelte";
  import { fr } from "../lib/i18n/fr";

  const FILTERS: ItemFilter[] = ["all", "links", "text", "files", "secrets", "archives"];

  let composer: Composer | undefined = $state();
  let pinned = $state(false);
  let searching = $state(false);

  async function togglePin() {
    pinned = !pinned;
    await feed.run(() => api.setPinned(pinned));
  }

  async function consumePrefill() {
    // Without a composer (this PC cannot send) the prefill stays where it is.
    if (!composer) return;
    try {
      const p = await api.takePrefill();
      if (p) composer?.applyPrefill(p);
    } catch (e) {
      feed.fail(e);
    }
  }

  onMount(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    const unlisten: Array<() => void> = [];
    void (async () => {
      stop = await feed.start();
      if (gone) stop();
      unlisten.push(
        await events.popoverShown(async () => {
          // Fetch what waits on the relay; the feed follows through the events.
          void api.syncNow().catch(() => {});
          await feed.refresh();
          await tick();
          composer?.focus();
          await consumePrefill();
        }),
        await events.prefillAvailable(() => void consumePrefill()),
      );
      if (gone) unlisten.forEach((u) => u());
      await consumePrefill();
    })();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !pinned && !e.defaultPrevented && !feed.hasOverlay) void api.hidePopover();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      gone = true;
      stop?.();
      unlisten.forEach((u) => u());
      window.removeEventListener("keydown", onKey);
    };
  });
</script>

<main class="popover">
  <header class="row top">
    <Logo />
    <strong class="grow name">{fr.appName}</strong>
    <button
      class="ghost icon"
      class:on={searching}
      title={fr.popover.search}
      aria-label={fr.popover.search}
      aria-pressed={searching}
      onclick={() => {
        if (searching) feed.setQuery("");
        searching = !searching;
      }}
    >
      <Icon name="search" size={20} />
    </button>
    <button
      class="ghost icon"
      class:on={pinned}
      title={pinned ? fr.popover.unpin : fr.popover.pin}
      aria-label={pinned ? fr.popover.unpin : fr.popover.pin}
      aria-pressed={pinned}
      onclick={togglePin}
    >
      <Icon name={pinned ? "push_pin_fill1" : "push_pin"} size={20} />
    </button>
    <button class="ghost icon" title={fr.popover.history} aria-label={fr.popover.history} onclick={() => api.openWindow("history")}>
      <Icon name="open_in_new" size={20} />
    </button>
    <button class="ghost icon" title={fr.popover.settings} aria-label={fr.popover.settings} onclick={() => api.openWindow("settings")}>
      <Icon name="tune" size={20} />
    </button>
  </header>

  <div class="body scroll">
    <StatusBanner link />
    <DepartureBanner />
    {#if feed.canSend}
      <Composer bind:this={composer} />
    {/if}
    <JobBar />

    {#if searching}
      <input
        type="search"
        placeholder={fr.popover.search}
        aria-label={fr.popover.search}
        value={feed.query}
        oninput={(e) => feed.setQuery(e.currentTarget.value)}
        onkeydown={(e) => {
          // Escape closes the search and empties the query; the popover itself stays.
          if (e.key === "Escape") {
            e.preventDefault();
            feed.setQuery("");
            searching = false;
          }
        }}
      />
    {/if}

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

    <section class="list">
      <FeedList />
    </section>
  </div>
</main>

<style>
  .popover {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .top {
    padding: var(--space-2) var(--space-2) var(--space-1) var(--space-4);
    gap: var(--space-1);
  }
  .name {
    margin-left: var(--space-1);
    font-size: var(--fs-subtitle);
  }
  .body {
    flex: 1;
    min-height: 0;
    padding: var(--space-1) var(--space-3) var(--space-3);
    overflow-x: hidden;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .body > :global(*),
  .list > :global(*) {
    flex-shrink: 0;
  }
  .filters {
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
</style>