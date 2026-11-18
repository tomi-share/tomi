<script lang="ts">
  import { feed } from "./feed.svelte";
  import { msg } from "./i18n";
</script>

{#if feed.flash}
  {@const flash = feed.flash}
  <div
    class="flash"
    class:error={flash.kind === "error"}
    role={flash.kind === "error" ? "alert" : "status"}
    aria-live="polite"
  >
    <span class="text">{flash.text}</span>
    {#if flash.action}
      <button
        class="action"
        type="button"
        title="{flash.action.label} ({msg.card.undoKey})"
        onclick={() => {
          const run = flash.action?.run;
          feed.clearFlash();
          run?.();
        }}
      >
        {flash.action.label}
      </button>
    {/if}
  </div>
{/if}

<style>
  /* Snackbar: inverse surface; an error is a filled error surface. */
  .flash {
    position: fixed;
    left: var(--space-3);
    right: var(--space-3);
    bottom: var(--space-3);
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: var(--text);
    color: var(--bg);
    box-shadow: 0 4px 16px color-mix(in srgb, var(--text) 24%, transparent);
    z-index: 50;
    animation: rise var(--dur-medium) ease-out;
  }
  .flash.error {
    background: var(--error);
    color: var(--on-error);
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  /* The button of the banner (Android: the snackbar action): text only, in the colour of the banner. */
  .action {
    flex: none;
    min-height: 32px;
    padding: 0 var(--space-2);
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: inherit;
    font-weight: 600;
    text-decoration: underline;
  }
  .action:hover {
    background: color-mix(in srgb, var(--bg) 16%, transparent);
  }
  @keyframes rise {
    from {
      transform: translateY(8px);
      opacity: 0;
    }
  }
</style>
