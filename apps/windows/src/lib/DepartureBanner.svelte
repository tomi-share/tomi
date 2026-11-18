<script lang="ts">
  import type { PendingReason } from "./api";
  import { departure } from "./departure.svelte";
  import { feed } from "./feed.svelte";
  import { msg } from "./i18n";
  import Icon from "./Icon.svelte";

  // The departure, always visible while it lasts (never a waiting departure nobody can see):
  // "Déconnexion en attente" with Réessayer / Annuler, "Effacer quand même" only when the
  // relay no longer finds the space, the erasure under way, and the two states that call for a
  // departure ("Espace introuvable", "Retiré de l'espace"). Charter 6.3: no circular arrows, a
  // linear bar only.

  const snap = $derived(departure.snap);
  const pending = $derived(snap?.departure ?? null);

  const REASONS: Record<PendingReason, string> = {
    network: msg.departure.pendingNetwork,
    server: msg.departure.pendingServer,
    notFound: msg.departure.pendingNotFound,
    notSent: msg.departure.pendingNotSent,
  };

  type View = {
    tone: "waiting" | "error";
    title: string;
    text: string;
    wiping?: boolean;
    cancelling?: boolean;
    retry?: boolean;
    cancel?: boolean;
    erase?: boolean;
    leave?: boolean;
  };

  const view = $derived.by((): View | null => {
    if (!snap || !snap.available) return null;
    if (pending?.wiping) {
      return { tone: "waiting", title: msg.departure.wiping, text: "", wiping: true };
    }
    // "Annuler" was pressed while an attempt was still out: the marker is already gone, but the
    // attempt goes on to its end (it erases nothing). Said until it ends, with no buttons.
    if (snap.cancelling) {
      return {
        tone: "waiting",
        title: msg.departure.cancellingTitle,
        text: msg.departure.cancellingText,
        cancelling: true,
      };
    }
    if (pending) {
      return {
        tone: pending.last === "notFound" ? "error" : "waiting",
        title:
          pending.kind === "deleteSpace"
            ? msg.departure.pendingDeleteTitle
            : msg.departure.pendingLeaveTitle,
        text: pending.last ? REASONS[pending.last] : msg.departure.pendingChecking,
        retry: true,
        cancel: true,
        erase: pending.last === "notFound",
      };
    }
    if (feed.status?.space === "notFound") {
      return {
        tone: "error",
        title: msg.departure.notFoundTitle,
        text: msg.departure.notFoundMessage,
        erase: true,
      };
    }
    if (feed.status?.paired && feed.status.space === "revoked") {
      return {
        tone: "error",
        title: msg.departure.revokedTitle,
        text: msg.departure.revokedMessage,
        leave: true,
      };
    }
    return null;
  });

  const working = $derived(snap?.working ?? false);
  // Windows Hello is on screen, or the erasure of "Effacer quand même" is under way: "Annuler"
  // would only say "reste lisible" while the erasure then goes through. Rust refuses it too.
  const cancelLocked = $derived(
    !!snap && (snap.authenticating || (snap.ui.busy && snap.ui.screen === "eraseWithoutProof")),
  );
</script>

{#if view}
  <div class="banner" class:error={view.tone === "error"} class:waiting={view.tone === "waiting"} role="status">
    <div class="row head">
      <Icon name={view.tone === "error" ? "error" : "schedule"} size={20} />
      <div class="grow">
        <strong>{view.title}</strong>
        {#if view.text}<div class="text">{view.text}</div>{/if}
      </div>
    </div>
    {#if view.wiping || view.cancelling || working}
      <progress
        aria-label={view.wiping
          ? msg.departure.wiping
          : view.cancelling
            ? msg.departure.cancellingTitle
            : msg.departure.pendingChecking}
      ></progress>
    {/if}
    {#if snap?.authenticating}
      <!-- One live region only (the banner): no role="status" inside it. -->
      <div class="text">{msg.departure.authenticating}</div>
    {/if}
    {#if view.retry || view.cancel || view.erase || view.leave}
      <div class="row actions">
        {#if view.retry}
          <button type="button" disabled={working} onclick={() => void departure.retry()}>
            {msg.departure.retry}
          </button>
        {/if}
        {#if view.cancel}
          <button
            class="ghost"
            type="button"
            disabled={cancelLocked}
            onclick={() => void departure.cancel()}
          >
            {msg.departure.cancel}
          </button>
        {/if}
        {#if view.erase}
          <button class="danger" type="button" disabled={working} onclick={() => void departure.openErase()}>
            {msg.departure.eraseAnyway}
          </button>
        {/if}
        {#if view.leave}
          <button class="danger" type="button" onclick={() => void departure.openLeave()}>
            {msg.departure.leave}
          </button>
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .banner {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    background: var(--surface-alt);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-2) var(--space-3);
    color: var(--text);
  }
  .head {
    align-items: flex-start;
  }
  .head :global(.icon) {
    margin-top: 1px;
    color: var(--muted);
  }
  .text {
    color: var(--muted);
  }
  .banner.waiting {
    background: var(--notice-bg);
  }
  .banner.waiting .head :global(.icon) {
    color: var(--secondary);
  }
  .banner.error {
    background: var(--error-bg);
    border-color: var(--error);
  }
  .banner.error .head :global(.icon) {
    color: var(--error);
  }
  .actions {
    flex-wrap: wrap;
  }
</style>
