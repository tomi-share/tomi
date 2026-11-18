<script lang="ts">
  import ItemCard from "./ItemCard.svelte";
  import { masonryCell } from "./actions";
  import { feed } from "./feed.svelte";
  import { msg, plural } from "./i18n";

  /** The full window shows the cards in a masonry grid (no gap under a short card), the popover in a column. */
  let { wide = false }: { wide?: boolean } = $props();

  const count = $derived(feed.sections.pinned.length + feed.sections.recent.length);
  const empty = $derived(count === 0);
  const searching = $derived(feed.query.trim() !== "");
  /** Title of the section below "Épinglés": "Archives" under that filter, "Récents" otherwise. */
  const otherTitle = $derived(feed.filter === "archives" ? msg.sections.archives : msg.sections.recent);
  /** The counter is noise for an empty query with the "Tous" filter (Android: FeedSearchDisplay). */
  const showCounter = $derived(searching || feed.filter !== "all");
  /** Only the boolean `secret` is read, never a title or a content (golden rule 3). */
  const showMaskedNote = $derived(
    searching && [...feed.sections.pinned, ...feed.sections.recent].some((i) => i.secret),
  );
</script>

{#snippet card(item: (typeof feed.sections.recent)[number])}
  <div class="cell" use:masonryCell>
    <ItemCard {item} />
  </div>
{/snippet}

{#if showCounter && feed.loaded}
  <p class="muted small count" role="status">
    {plural(msg.searchResults, count)}{#if showMaskedNote} · {msg.searchMaskedNote}{/if}
  </p>
{/if}

{#if searching}
  <!-- Flat results (Android): pinned first, then the others, no section titles. -->
  <div class="cards" class:grid={wide}>
    {#each [...feed.sections.pinned, ...feed.sections.recent] as item (item.id)}
      {@render card(item)}
    {/each}
  </div>
{:else}
  {#if feed.sections.pinned.length > 0}
    <section class="group" aria-label={msg.sections.pinned}>
      <h3 class="title">{msg.sections.pinned}</h3>
      <div class="cards" class:grid={wide}>
        {#each feed.sections.pinned as item (item.id)}
          {@render card(item)}
        {/each}
      </div>
    </section>
  {/if}

  {#if feed.sections.recent.length > 0}
    <section class="group" aria-label={otherTitle}>
      <!-- Without a pinned section there is nothing to tell the cards apart from; "Archives" is always titled (Android: FeedScreen). -->
      {#if feed.sections.pinned.length > 0 || feed.filter === "archives"}<h3 class="title">{otherTitle}</h3>{/if}
      <div class="cards" class:grid={wide}>
        {#each feed.sections.recent as item (item.id)}
          {@render card(item)}
        {/each}
      </div>
    </section>
  {/if}
{/if}

{#if empty && feed.loaded}
  <p class="muted empty">
    {searching
      ? msg.empty.search
      : feed.filter === "archives"
        ? msg.empty.archives
        : feed.filter !== "all"
          ? msg.empty.filtered
          : feed.canSend
            ? msg.empty.all
            : msg.empty.receiveOnly}
  </p>
{/if}

<style>
  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  /* Air above the section titles, not between the cards (owner feedback on Android). */
  .group + .group {
    margin-top: var(--space-4);
  }
  .title {
    font-size: var(--fs-label);
    line-height: var(--lh-label);
    font-weight: 600;
    color: var(--muted);
    margin: var(--space-2) var(--space-1) 0;
  }
  .count {
    margin: 0 var(--space-1);
  }
  .cards {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  /* Masonry: 1 px rows, each cell spans its own height (masonryCell) and "dense" lets the next card
     come up under the shortest column. The gap between rows is the bottom padding of the cell. */
  .cards.grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    grid-auto-rows: 1px;
    grid-auto-flow: row dense;
    column-gap: var(--space-3);
    row-gap: 0;
    align-content: start;
    align-items: start;
  }
  .cell {
    min-width: 0;
  }
  .cards.grid > .cell {
    padding-bottom: var(--space-3);
  }
  .cards > :global(*) {
    flex-shrink: 0;
  }
  .empty {
    text-align: center;
    padding: var(--space-6) var(--space-4);
    margin: 0;
  }
</style>
