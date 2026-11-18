<script lang="ts">
  import PopoverView from "./views/PopoverView.svelte";
  import HistoryView from "./views/HistoryView.svelte";
  import Flash from "./lib/Flash.svelte";
  import Overlays from "./lib/Overlays.svelte";
  import { feed } from "./lib/feed.svelte";

  // Both windows load the same bundle; the URL (see popover.rs) picks the view.
  const params = new URLSearchParams(window.location.search);
  const view = params.get("view") === "history" ? "history" : "popover";
  const tab = params.get("tab");
</script>

<!-- Behind a full-window screen, nothing can be reached with the keyboard. -->
<div class="app" inert={feed.screenOpen || feed.backdrops > 0}>
  {#if view === "history"}
    <HistoryView initialTab={tab} />
  {:else}
    <PopoverView />
  {/if}
</div>
<Overlays />
<Flash />

<style>
  .app {
    height: 100%;
  }
</style>
