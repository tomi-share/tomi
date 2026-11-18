<script lang="ts">
  // The demo mode panel (a debug build started with `TOMI_DEMO=1`). Loaded only through a dynamic
  // import behind `import.meta.env.DEV`, so nothing of it (nor of `demo.ts`) is in a release build.
  import { demo } from "./demo";
  import { feed } from "./feed.svelte";
</script>

<div class="demo">
  <strong>{demo.title}</strong>
  <p class="muted small">{demo.text}</p>
  <div class="row wrap">
    <button onclick={() => feed.run(() => demo.receive("link"))}>{demo.receiveLink}</button>
    <button onclick={() => feed.run(() => demo.receive("text"))}>{demo.receiveText}</button>
    <button onclick={() => feed.run(() => demo.receive("markdown"))}>{demo.receiveMarkdown}</button>
    <button onclick={() => feed.run(() => demo.receive("secret"))}>{demo.receiveSecret}</button>
  </div>
  <strong class="demo-sub">{demo.departureTitle}</strong>
  <div class="row wrap">
    {#each Object.keys(demo.departureModes) as mode (mode)}
      <button onclick={() => feed.run(() => demo.departure(mode))}>{demo.departureModes[mode]}</button>
    {/each}
  </div>
</div>

<style>
  .demo {
    background: var(--notice-bg);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3);
  }
  .demo p {
    margin: var(--space-1) 0 var(--space-2);
  }
  .wrap {
    flex-wrap: wrap;
  }
  .demo-sub {
    display: block;
    margin-top: var(--space-3);
  }
</style>
