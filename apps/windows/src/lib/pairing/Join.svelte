<script lang="ts">
  import type { PairingSession } from "../api";
  import { format, fr, pairingErrorMessage } from "../i18n/fr";
  import Verify from "./Verify.svelte";

  /**
   * This PC joins a space (spec/pairing-v1.md §7.3): the inviter answers, the verification
   * code is compared with the one on the other device, the history question is answered, and
   * the PC is provisioned by the core.
   */
  let {
    session,
    onconfirm,
    onrefuse,
    onfinish,
  }: {
    session: PairingSession;
    onconfirm: (importHistory: boolean) => Promise<boolean>;
    /** Refuse, or cancel: the channel is closed and nothing is joined. */
    onrefuse: () => void;
    /** A pairing that is over is closed. */
    onfinish: () => void;
  } = $props();

  const device = $derived(session.peer?.name ?? "");
</script>

{#if session.stage === "waiting"}
  <div class="join">
    <h3>{fr.pairing.joiningTitle}</h3>
    <p class="muted">{fr.pairing.waitingJoin}</p>
    <progress aria-label={fr.pairing.joining}></progress>
    {#if session.offline}<p class="warn small">{fr.pairing.offline}</p>{/if}
    <div class="row"><button onclick={onrefuse}>{fr.pairing.cancel}</button></div>
  </div>
{:else if session.stage === "verify"}
  <Verify role="join" {session} {onconfirm} {onrefuse} />
  {#if session.offline}<p class="warn small">{fr.pairing.offline}</p>{/if}
{:else if session.stage === "provisioning"}
  <div class="join">
    <h3>{fr.pairing.joiningTitle}</h3>
    <p>{format(fr.pairing.provisioningJoin, { device })}</p>
    <progress aria-label={fr.pairing.working}></progress>
    {#if session.offline}<p class="warn small">{fr.pairing.offline}</p>{/if}
    <div class="row"><button onclick={onrefuse}>{fr.pairing.cancel}</button></div>
  </div>
{:else if session.stage === "done"}
  <div class="join">
    <p class="done">✓ {fr.pairing.doneJoin}</p>
    {#if session.history}<p class="muted">{fr.pairing.historyPreparingJoin}</p>{/if}
    <div class="row"><button class="primary" onclick={onfinish}>{fr.pairing.finish}</button></div>
  </div>
{:else}
  <div class="join">
    <p class="error">{pairingErrorMessage(session.error, "join")}</p>
    <div class="row"><button class="primary" onclick={onfinish}>{fr.pairing.close}</button></div>
  </div>
{/if}

<style>
  .join {
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
  }
  p {
    margin: 0;
  }
  .done {
    font-weight: 600;
  }
  .warn {
    color: var(--muted);
  }
  .error {
    color: var(--error);
  }
  progress {
    width: 100%;
  }
</style>
