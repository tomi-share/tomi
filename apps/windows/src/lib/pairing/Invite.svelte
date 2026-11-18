<script lang="ts">
  import type { PairingSession } from "../api";
  import { format, formatRemaining, fr, pairingErrorMessage } from "../i18n/fr";
  import QrCode from "./QrCode.svelte";
  import Verify from "./Verify.svelte";

  /**
   * This PC is a member and adds a device (spec/pairing-v1.md §7.2): a code and a QR code,
   * then the verification code to compare with the new device, then the device is added.
   */
  let {
    session,
    onconfirm,
    onrefuse,
    onfinish,
    onagain,
  }: {
    session: PairingSession;
    onconfirm: (importHistory: boolean) => Promise<boolean>;
    /** Refuse, or cancel: the channel is closed and the code is gone. */
    onrefuse: () => void;
    /** A pairing that is over is closed. */
    onfinish: () => void;
    /** A new code, after one that led nowhere. */
    onagain: () => void;
  } = $props();

  let now = $state(Date.now());
  // The countdown of the code, while it is the only thing on screen.
  $effect(() => {
    if (session.stage !== "waiting") return;
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });

  const left = $derived(session.expiresAt === null ? null : session.expiresAt - now);
  const device = $derived(session.peer?.name ?? "");
</script>

{#if session.stage === "waiting"}
  <div class="invite">
    <h3>{fr.pairing.inviteTitle}</h3>
    <p class="muted">{fr.pairing.inviteIntro}</p>
    {#if session.renewed}<p class="note">{fr.pairing.renewed}</p>{/if}
    {#if session.qr && session.code}
      <div class="shown">
        <QrCode qr={session.qr} label={fr.pairing.qrLabel} />
        <div class="code-side">
          <span class="muted small">{fr.pairing.shortCode}</span>
          <!-- Shown, never copied by the app (spec/pairing-v1.md §12). -->
          <span class="code">{session.code}</span>
          {#if left !== null}
            <!-- Not announced every second. -->
            <span class="muted small" aria-live="off">
              {left > 0 ? format(fr.pairing.expiresIn, { time: formatRemaining(left) }) : fr.pairing.expired}
            </span>
          {/if}
        </div>
      </div>
    {:else}
      <progress aria-label={fr.pairing.preparing}></progress>
    {/if}
    {#if session.offline}<p class="warn small">{fr.pairing.offline}</p>{/if}
    <div class="row">
      <button onclick={onrefuse}>{fr.pairing.cancel}</button>
    </div>
  </div>
{:else if session.stage === "verify"}
  <Verify role="invite" {session} {onconfirm} {onrefuse} />
  {#if session.offline}<p class="warn small">{fr.pairing.offline}</p>{/if}
{:else if session.stage === "provisioning"}
  <div class="invite">
    <h3>{format(fr.pairing.addingTitle, { device })}</h3>
    <p>{format(fr.pairing.provisioningInvite, { device })}</p>
    <progress aria-label={fr.pairing.working}></progress>
    {#if session.offline}<p class="warn small">{fr.pairing.offline}</p>{/if}
  </div>
{:else if session.stage === "done"}
  <div class="invite">
    <p class="done">✓ {format(fr.pairing.doneInvite, { device })}</p>
    {#if session.history}
      <p class="muted">{format(fr.pairing.historyPreparingInvite, { device })}</p>
    {/if}
    <div class="row"><button class="primary" onclick={onfinish}>{fr.pairing.finish}</button></div>
  </div>
{:else}
  <div class="invite">
    <p class="error">{pairingErrorMessage(session.error, "invite")}</p>
    <div class="row">
      <button class="primary" onclick={onagain}>{fr.pairing.newCode}</button>
      <button onclick={onfinish}>{fr.pairing.close}</button>
    </div>
  </div>
{/if}

<style>
  .invite {
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
  }
  p {
    margin: 0;
  }
  .shown {
    display: flex;
    gap: 16px;
    align-items: center;
    flex-wrap: wrap;
  }
  .code-side {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .code {
    font-family: var(--font-mono);
    font-size: var(--fs-pairing-code);
    font-weight: 600;
    line-height: 1.2;
    font-feature-settings: "tnum" 1, "calt" 0, "liga" 0;
    letter-spacing: 0.02em;
    user-select: none;
  }
  .note {
    padding: 6px 10px;
    border-radius: var(--radius);
    background: var(--notice-bg);
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
