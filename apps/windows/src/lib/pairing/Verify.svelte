<script lang="ts">
  import type { PairingSession } from "../api";
  import { format, formatBytes, fr } from "../i18n/fr";

  /**
   * The two codes to compare (spec/pairing-v1.md §5.3): shown, grouped by three, never typed,
   * copied or sent. On the new device the single yes / no question about the history is asked
   * here too (§9).
   */
  let {
    role,
    session,
    onconfirm,
    onrefuse,
  }: {
    role: "invite" | "join";
    session: PairingSession;
    /** Resolves to `true` once the decision is taken. */
    onconfirm: (importHistory: boolean) => Promise<boolean>;
    onrefuse: () => void;
  } = $props();

  /** "Yes" is preselected, as on Android. */
  let importHistory = $state(true);
  let sending = $state(false);

  const device = $derived(session.peer?.name ?? "");
  const history = $derived(role === "join" ? session.history : null);

  async function confirm() {
    sending = true;
    // Stays disabled once the decision is taken: the screen moves on by itself.
    sending = await onconfirm(importHistory);
  }
</script>

<div class="verify">
  <h3>{fr.pairing.verifyTitle}</h3>
  <p>
    {format(role === "join" ? fr.pairing.verifyJoin : fr.pairing.verifyInvite, { device })}
  </p>

  {#if session.sas}
    <div class="sas" aria-label={format(fr.pairing.codeDescription, { code: session.sas })}>
      {session.sas}
    </div>
  {/if}

  <!-- The relay may come from a link or a QR code, which whoever shows it chooses: it is shown in
       full, next to the code, and confirmed with it (§4.4). -->
  {#if role === "join" && session.relayAuthority}
    <div class="relay">
      <strong class="mono">{format(fr.pairing.relayOnScreen, { relay: session.relayAuthority })}</strong>
      <span class="small">{fr.pairing.relayVerify}</span>
    </div>
  {/if}

  {#if role === "join" && session.memberCount}
    <p class="muted small">
      {session.memberCount === 1
        ? fr.pairing.spaceSizeOne
        : format(fr.pairing.spaceSizeMany, { n: session.memberCount })}
    </p>
  {/if}

  {#if history}
    <fieldset class="history">
      <legend>{fr.pairing.historyTitle}</legend>
      <p class="muted small">
        {format(fr.pairing.historySize, { items: history.items, size: formatBytes(history.bytes) })}
      </p>
      <label class="choice">
        <input
          type="radio"
          name="history"
          checked={importHistory}
          disabled={sending}
          onchange={() => (importHistory = true)}
        />
        <span>
          <strong>{fr.pairing.historyYes}</strong>
          <span class="muted small hint">{fr.pairing.historyYesHint}</span>
        </span>
      </label>
      <label class="choice">
        <input
          type="radio"
          name="history"
          checked={!importHistory}
          disabled={sending}
          onchange={() => (importHistory = false)}
        />
        <span>
          <strong>{fr.pairing.historyNo}</strong>
          <span class="muted small hint">{fr.pairing.historyNoHint}</span>
        </span>
      </label>
    </fieldset>
  {/if}

  <div class="row actions">
    <button class="primary" disabled={sending} onclick={confirm}>
      {role === "join" ? fr.pairing.same : format(fr.pairing.add, { device })}
    </button>
    <button disabled={sending} onclick={onrefuse}>
      {role === "join" ? fr.pairing.different : fr.pairing.refuse}
    </button>
  </div>
  <p class="muted small">{fr.pairing.verifyHint}</p>
</div>

<style>
  .verify {
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
  }
  p {
    margin: 0;
  }
  .sas {
    align-self: stretch;
    text-align: center;
    padding: 14px 8px;
    border-radius: var(--radius);
    background: var(--surface-alt);
    border: 1px solid var(--border);
    font-family: var(--font-mono);
    font-size: var(--fs-pairing-code);
    font-weight: 600;
    line-height: 1.2;
    font-feature-settings: "tnum" 1, "calt" 0, "liga" 0;
    letter-spacing: 0.12em;
    font-variant-numeric: tabular-nums;
    user-select: none;
  }
  .relay {
    align-self: stretch;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px 10px;
    border-radius: var(--radius);
    background: var(--notice-bg);
    /* A long address is never cut: it is what the user checks. */
    overflow-wrap: anywhere;
    user-select: text;
  }
  .history {
    align-self: stretch;
    margin: 0;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  legend {
    font-weight: 600;
    padding: 0 4px;
  }
  .choice {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    cursor: pointer;
  }
  .choice input {
    margin: 3px 0 0;
  }
  .hint {
    display: block;
  }
  .actions {
    margin-top: 4px;
    flex-wrap: wrap;
  }
</style>
