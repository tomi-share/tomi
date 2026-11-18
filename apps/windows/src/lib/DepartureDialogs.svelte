<script lang="ts">
  import { onMount } from "svelte";
  import type { RefusalReason } from "./api";
  import Dialog from "./Dialog.svelte";
  import { departure } from "./departure.svelte";
  import { fr } from "./i18n/fr";

  // The three confirmations of the departure, drawn from the snapshot Rust keeps
  // (`departure.rs`): "Se déconnecter", "Supprimer l'espace" (the last device) and
  // "Effacer quand même". The 2 s anti-tap and Windows Hello are enforced in Rust; the disabled
  // button here is only the visible side of the same rule.

  const ui = $derived(departure.snap?.ui ?? null);
  let now = $state(Date.now());

  onMount(() => {
    const tick = setInterval(() => (now = Date.now()), 250);
    return () => clearInterval(tick);
  });

  const armed = $derived(ui?.armedAtMs != null && now >= ui.armedAtMs);

  /** Windows Hello is on screen: its own sentence, and nothing to cancel from here yet. */
  const authenticating = $derived(departure.snap?.authenticating ?? false);
  /** "Annuler" works during the request of a leave or a deletion (not during Hello, nor the erasure). */
  const cancellable = $derived(!authenticating);

  function busyLabel(working: string): string {
    return authenticating ? fr.departure.authenticating : working;
  }

  function leaveError(reason: RefusalReason | null, failed: boolean): string | null {
    if (departure.dialogError) return departure.dialogError;
    if (failed) return fr.departure.failed;
    if (reason === "updateRequired") return fr.departure.leaveErrorUpdate;
    if (reason) return fr.departure.leaveErrorUnexpected;
    return null;
  }

  function deleteError(reason: RefusalReason | null, failed: boolean): string | null {
    if (departure.dialogError) return departure.dialogError;
    if (failed) return fr.departure.failed;
    if (reason === "relayTooOld") return fr.departure.deleteErrorRelayTooOld;
    if (reason === "updateRequired") return fr.departure.deleteErrorUpdate;
    if (reason) return fr.departure.deleteErrorUnexpected;
    return null;
  }

  function eraseError(): string | null {
    if (departure.dialogError) return departure.dialogError;
    if (ui?.notAllowed) return fr.departure.eraseNotAllowed;
    if (ui?.failed) return fr.departure.failed;
    return null;
  }
</script>

{#if ui && departure.screen === "confirmLeave"}
  <Dialog
    title={fr.departure.leaveTitle}
    message={fr.departure.leaveMessage}
    confirmLabel={fr.departure.leave}
    busy={ui.busy}
    busyLabel={fr.departure.leaveInProgress}
    cancellable={cancellable}
    oncancelbusy={() => void departure.cancelRunning()}
    inertBackground
    error={leaveError(ui.error, ui.failed)}
    onconfirm={() => void departure.confirmLeave()}
    oncancel={() => void departure.dismiss()}
  />
{:else if ui && departure.screen === "confirmDelete"}
  <Dialog
    title={fr.departure.deleteTitle}
    message={fr.departure.deleteMessage}
    confirmLabel={fr.departure.deleteConfirm}
    confirmDisabled={!armed || ui.error === "relayTooOld"}
    busy={ui.busy}
    busyLabel={busyLabel(fr.departure.deleteInProgress)}
    cancellable={cancellable}
    oncancelbusy={() => void departure.cancelRunning()}
    inertBackground
    announce={armed && !ui.busy && ui.error !== "relayTooOld" ? fr.departure.ready : null}
    error={deleteError(ui.error, ui.failed)}
    onconfirm={() => void departure.confirmDelete()}
    oncancel={() => void departure.dismiss()}
  >
    {#snippet details()}
      <p class="detail">{fr.departure.deleteDetail}</p>
      <p class="detail hello">{fr.departure.helloNote}</p>
    {/snippet}
    {#if ui.error === "relayTooOld" && !ui.busy}
      <button class="danger" type="button" onclick={() => void departure.openErase()}>
        {fr.departure.eraseAnyway}
      </button>
    {/if}
  </Dialog>
{:else if ui && departure.screen === "eraseWithoutProof"}
  <Dialog
    title={fr.departure.eraseTitle}
    message={fr.departure.eraseMessage}
    confirmLabel={fr.departure.eraseConfirm}
    confirmDisabled={!ui.checked || !armed || ui.checking || ui.notAllowed}
    busy={ui.busy}
    busyLabel={busyLabel(fr.departure.eraseInProgress)}
    inertBackground
    announce={ui.checked && armed && !ui.busy && !ui.checking && !ui.notAllowed
      ? fr.departure.ready
      : null}
    error={eraseError()}
    onconfirm={() => void departure.confirmErase()}
    oncancel={() => void departure.dismiss()}
  >
    {#snippet details()}
      <p class="detail">{fr.departure.eraseDetail}</p>
      <p class="detail hello">{fr.departure.helloNote}</p>
      {#if ui.checking}<p class="detail" role="status">{fr.departure.eraseChecking}</p>{/if}
    {/snippet}
    <label class="check">
      <input
        type="checkbox"
        checked={ui.checked}
        disabled={ui.busy || ui.checking || ui.notAllowed}
        onchange={(e) => void departure.setEraseChecked(e.currentTarget.checked)}
      />
      {fr.departure.eraseCheckbox}
    </label>
  </Dialog>
{/if}

<style>
  .detail {
    margin: 0;
    color: var(--muted);
  }
  .hello {
    font-size: var(--fs-label);
  }
  .check {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    cursor: pointer;
  }
</style>
