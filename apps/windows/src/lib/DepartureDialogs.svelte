<script lang="ts">
  import { onMount } from "svelte";
  import type { RefusalReason } from "./api";
  import Dialog from "./Dialog.svelte";
  import { departure } from "./departure.svelte";
  import { msg } from "./i18n";

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
    return authenticating ? msg.departure.authenticating : working;
  }

  function leaveError(reason: RefusalReason | null, failed: boolean): string | null {
    if (departure.dialogError) return departure.dialogError;
    if (failed) return msg.departure.failed;
    if (reason === "updateRequired") return msg.departure.leaveErrorUpdate;
    if (reason) return msg.departure.leaveErrorUnexpected;
    return null;
  }

  function deleteError(reason: RefusalReason | null, failed: boolean): string | null {
    if (departure.dialogError) return departure.dialogError;
    if (failed) return msg.departure.failed;
    if (reason === "relayTooOld") return msg.departure.deleteErrorRelayTooOld;
    if (reason === "updateRequired") return msg.departure.deleteErrorUpdate;
    if (reason) return msg.departure.deleteErrorUnexpected;
    return null;
  }

  function eraseError(): string | null {
    if (departure.dialogError) return departure.dialogError;
    if (ui?.notAllowed) return msg.departure.eraseNotAllowed;
    if (ui?.failed) return msg.departure.failed;
    return null;
  }
</script>

{#if ui && departure.screen === "confirmLeave"}
  <Dialog
    title={msg.departure.leaveTitle}
    message={msg.departure.leaveMessage}
    confirmLabel={msg.departure.leave}
    busy={ui.busy}
    busyLabel={msg.departure.leaveInProgress}
    cancellable={cancellable}
    oncancelbusy={() => void departure.cancelRunning()}
    inertBackground
    error={leaveError(ui.error, ui.failed)}
    onconfirm={() => void departure.confirmLeave()}
    oncancel={() => void departure.dismiss()}
  />
{:else if ui && departure.screen === "confirmDelete"}
  <Dialog
    title={msg.departure.deleteTitle}
    message={msg.departure.deleteMessage}
    confirmLabel={msg.departure.deleteConfirm}
    confirmDisabled={!armed || ui.error === "relayTooOld"}
    busy={ui.busy}
    busyLabel={busyLabel(msg.departure.deleteInProgress)}
    cancellable={cancellable}
    oncancelbusy={() => void departure.cancelRunning()}
    inertBackground
    announce={armed && !ui.busy && ui.error !== "relayTooOld" ? msg.departure.ready : null}
    error={deleteError(ui.error, ui.failed)}
    onconfirm={() => void departure.confirmDelete()}
    oncancel={() => void departure.dismiss()}
  >
    {#snippet details()}
      <p class="detail">{msg.departure.deleteDetail}</p>
      <p class="detail hello">{msg.departure.helloNote}</p>
    {/snippet}
    {#if ui.error === "relayTooOld" && !ui.busy}
      <button class="danger" type="button" onclick={() => void departure.openErase()}>
        {msg.departure.eraseAnyway}
      </button>
    {/if}
  </Dialog>
{:else if ui && departure.screen === "eraseWithoutProof"}
  <Dialog
    title={msg.departure.eraseTitle}
    message={msg.departure.eraseMessage}
    confirmLabel={msg.departure.eraseConfirm}
    confirmDisabled={!ui.checked || !armed || ui.checking || ui.notAllowed}
    busy={ui.busy}
    busyLabel={busyLabel(msg.departure.eraseInProgress)}
    inertBackground
    announce={ui.checked && armed && !ui.busy && !ui.checking && !ui.notAllowed
      ? msg.departure.ready
      : null}
    error={eraseError()}
    onconfirm={() => void departure.confirmErase()}
    oncancel={() => void departure.dismiss()}
  >
    {#snippet details()}
      <p class="detail">{msg.departure.eraseDetail}</p>
      <p class="detail hello">{msg.departure.helloNote}</p>
      {#if ui.checking}<p class="detail" role="status">{msg.departure.eraseChecking}</p>{/if}
    {/snippet}
    <label class="check">
      <input
        type="checkbox"
        checked={ui.checked}
        disabled={ui.busy || ui.checking || ui.notAllowed}
        onchange={(e) => void departure.setEraseChecked(e.currentTarget.checked)}
      />
      {msg.departure.eraseCheckbox}
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
