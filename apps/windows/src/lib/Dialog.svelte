<script lang="ts">
  import { onMount, tick, type Snippet } from "svelte";
  import { feed } from "./feed.svelte";
  import { fr } from "./i18n/fr";

  /**
   * A confirmation (Android's AlertDialog): a title, a sentence, a destructive action and "Annuler".
   * Modal: the focus stays inside, Escape cancels (and does not close the popover), and the
   * focus starts on "Annuler" so that Enter never confirms a deletion by accident.
   *
   * The irreversible departure dialogs add what they need: more content (`details` is read with
   * the sentence, `children` holds the controls), a confirm button that stays disabled
   * (`confirmDisabled`: the 2 s anti-tap, the checkbox), a `busy` state in which nothing can close
   * the dialog (the focus then stays on the dialog itself, and comes back to "Annuler" when the
   * attempt fails), an "Annuler" that stays available during a long attempt (`cancellable`), a
   * sentence for screen readers (`announce`), an `error` line, and an inert window behind
   * (`inertBackground`, for a dialog drawn outside the main tree).
   */
  let {
    title,
    message,
    confirmLabel,
    onconfirm,
    secondaryLabel,
    onsecondary,
    oncancel,
    confirmDisabled = false,
    busy = false,
    busyLabel,
    cancellable = false,
    oncancelbusy,
    announce = null,
    inertBackground = false,
    error = null,
    details,
    children,
  }: {
    title: string;
    message: string;
    confirmLabel: string;
    onconfirm: () => void;
    /** A second destructive choice, before the confirm button (the lesser one). */
    secondaryLabel?: string;
    onsecondary?: () => void;
    oncancel: () => void;
    confirmDisabled?: boolean;
    busy?: boolean;
    busyLabel?: string;
    /** "Annuler" stays enabled while `busy`; it calls `oncancelbusy`. */
    cancellable?: boolean;
    oncancelbusy?: () => void;
    /** Said once to screen readers (a polite live region), e.g. when the button gets armed. */
    announce?: string | null;
    inertBackground?: boolean;
    error?: string | null;
    details?: Snippet;
    children?: Snippet;
  } = $props();

  let dialog: HTMLElement | undefined = $state();
  let cancelButton: HTMLButtonElement | undefined = $state();
  const uid = Math.random().toString(36).slice(2, 8);
  let previous: HTMLElement | null = null;
  let wasBusy = false;

  onMount(() => {
    previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    feed.overlays += 1;
    if (inertBackground) feed.backdrops += 1;
    cancelButton?.focus();
    return () => {
      feed.overlays -= 1;
      if (inertBackground) feed.backdrops -= 1;
      // The background is still inert at this very moment (the page applies it on the next
      // tick): focusing now would be refused, and the keyboard would be left on <body>.
      const back = previous;
      void tick().then(() => back?.focus());
    };
  });

  // A disabled button cannot hold the focus: while an attempt runs with nothing to press, the
  // dialog itself keeps it (a screen reader reads the status); when it fails the focus returns
  // to "Annuler", so the keyboard is never left on the page behind.
  $effect(() => {
    const nowBusy = busy;
    const canCancel = cancellable;
    void tick().then(() => {
      if (nowBusy && !canCancel) dialog?.focus();
      else if (nowBusy) cancelButton?.focus();
      else if (wasBusy) cancelButton?.focus();
      wasBusy = nowBusy;
    });
  });

  /** Escape and a click on the background: nothing closes the dialog while something is under way. */
  function cancel() {
    if (!busy) oncancel();
  }

  /** The button: during an attempt, it cancels the attempt (when the dialog allows it). */
  function cancelButtonClick() {
    if (busy) {
      if (cancellable) oncancelbusy?.();
    } else {
      oncancel();
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      cancel();
    } else if (e.key === "Tab" && dialog) {
      const focusable = Array.from(
        dialog.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)"),
      );
      if (focusable.length === 0) {
        // Nothing to press: the focus stays on the dialog, not on the window behind.
        e.preventDefault();
        dialog.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (e.shiftKey && (document.activeElement === first || document.activeElement === dialog)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="scrim" role="presentation" onclick={cancel} onkeydown={onKey}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    bind:this={dialog}
    class="dialog"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="dlg-title-{uid}"
    aria-describedby={details ? `dlg-msg-${uid} dlg-details-${uid}` : `dlg-msg-${uid}`}
    aria-busy={busy}
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
  >
    <h2 id="dlg-title-{uid}">{title}</h2>
    <p id="dlg-msg-{uid}">{message}</p>
    {#if details}<div id="dlg-details-{uid}" class="details">{@render details()}</div>{/if}
    {@render children?.()}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if busy && busyLabel}<p class="busy" role="status">{busyLabel}</p>
      <progress aria-label={busyLabel}></progress>{/if}
    <!-- Always mounted: a live region that appears together with its text is often not read.
         Only its text changes (e.g. when the confirm button gets armed). -->
    <p class="sr-only" role="status" aria-live="polite">{announce ?? ""}</p>
    <div class="actions">
      <button
        bind:this={cancelButton}
        class="ghost"
        type="button"
        disabled={busy && !cancellable}
        onclick={cancelButtonClick}
      >
        {fr.common.cancel}
      </button>
      {#if secondaryLabel && onsecondary}
        <button class="ghost danger" type="button" disabled={busy} onclick={onsecondary}>
          {secondaryLabel}
        </button>
      {/if}
      <button class="ghost danger" type="button" disabled={busy || confirmDisabled} onclick={onconfirm}>
        {confirmLabel}
      </button>
    </div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: grid;
    place-items: center;
    padding: var(--space-4);
    background: color-mix(in srgb, var(--bg) 55%, transparent);
    backdrop-filter: blur(1px);
  }
  .dialog {
    width: min(360px, 100%);
    max-height: 100%;
    overflow-y: auto;
    padding: var(--space-5);
    background: var(--surface);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: 0 8px 32px color-mix(in srgb, var(--text) 24%, transparent);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .dialog:focus {
    outline: none;
  }
  .details {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  p {
    margin: 0;
    color: var(--muted);
  }
  p.error {
    color: var(--error);
  }
  p.busy {
    color: var(--text);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }
</style>
