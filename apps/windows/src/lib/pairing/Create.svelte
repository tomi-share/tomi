<script lang="ts">
  import { untrack } from "svelte";
  import { api, type AppError } from "../api";
  import { errorMessage, format, fr } from "../i18n/fr";
  import { relayShown, relayUrlOf } from "./relay";

  /**
   * First device: creates the space with the install token of the relay
   * (spec/pairing-v1.md §10). There is nobody to compare codes with yet.
   *
   * The token may be pasted as a creation link (`https://<relay>/create#<token>`), which names
   * the relay. A relay that was read is not trusted: it is shown in full, here, before "Créer
   * l'espace", and nothing is kept before the creation succeeds. A PC that has no relay must be
   * given one; one that has its relay keeps the field folded under "Autre relais".
   */
  let {
    deviceName,
    relayAuthority = null,
    initialToken = "",
    initialRelay = "",
    busy,
    error,
    onsubmit,
    onback,
  }: {
    deviceName: string;
    /** The relay this PC already has (`host[:port]`), `null` when it has none. */
    relayAuthority?: string | null;
    /** The token of a creation link that opened the app. */
    initialToken?: string;
    /** The relay of that link, `https://host`. */
    initialRelay?: string;
    busy: boolean;
    error: AppError | null;
    onsubmit: (installTokenOrLink: string, deviceName: string, relayUrl: string | null) => void;
    onback: () => void;
  } = $props();

  // A secret: typed or pasted, masked unless asked, used once, never kept or logged.
  let token = $state(untrack(() => initialToken));
  let relay = $state(untrack(() => initialRelay));
  let relayRead = $state(untrack(() => initialRelay !== ""));
  /** Set by the reading of a pasted creation link only: a link that goes away takes its relay. */
  let relayPasted = $state(false);
  let otherOpen = $state(untrack(() => initialRelay !== ""));
  let name = $state(untrack(() => deviceName));
  let shown = $state(false);

  const introParts = fr.pairing.createIntro.split("{command}");
  const needsRelay = $derived(relayAuthority === null);
  /** The relay the space will be created at, as the user is shown it, in full. */
  const retained = $derived(relay.trim() !== "" ? relayShown(relay) : relayAuthority);
  const ready = $derived(
    token.trim() !== "" && name.trim() !== "" && retained !== null && !busy,
  );

  // A pasted creation link is read by the core (nothing is sent): its relay fills the field.
  $effect(() => {
    const text = token;
    if (!text.includes("://")) {
      if (untrack(() => relayPasted)) {
        relay = "";
        relayRead = false;
        relayPasted = false;
      }
      return;
    }
    let current = true;
    api
      .pairingReadInput(text)
      .then((r) => {
        if (!current) return;
        if (r.kind === "setupLink" && r.relayUrl) {
          relay = r.relayUrl;
          relayRead = true;
          relayPasted = true;
          otherOpen = true;
        } else if (relayPasted) {
          relay = "";
          relayRead = false;
          relayPasted = false;
        }
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  });

  function submit() {
    if (ready) onsubmit(token.trim(), name.trim(), relayUrlOf(relay));
  }
</script>

{#snippet relayField()}
  <div class="field">
    <label for="pair-create-relay">{fr.pairing.relayLabel}</label>
    <input
      id="pair-create-relay"
      class="mono"
      type="text"
      bind:value={relay}
      placeholder={fr.pairing.relayPlaceholder}
      autocomplete="off"
      autocapitalize="off"
      spellcheck="false"
      disabled={busy}
      required={needsRelay}
      oninput={() => {
        relayPasted = false;
        relayRead = false;
      }}
      onkeydown={(e) => e.key === "Enter" && submit()}
    />
    <span class="muted small">
      {#if relayRead}
        {fr.pairing.relayFromLink}
      {:else if needsRelay}
        {fr.pairing.relayHintRequired}
      {:else}
        {fr.pairing.relayHint}
      {/if}
    </span>
  </div>
{/snippet}

<div class="form">
  <h3>{fr.pairing.createTitle}</h3>
  {#if initialToken !== ""}
    <div class="note">
      <strong>{fr.pairing.setupLinkReceived}</strong>
      <span class="small">{fr.pairing.setupLinkReceivedHint}</span>
    </div>
  {/if}
  <!-- The command is exact text: monospace, no ligature. -->
  <p class="muted">
    {introParts[0]}<code class="mono">tomi-relay admin invite --new</code>{introParts[1]}
  </p>

  <div class="field">
    <label for="pair-token">{fr.pairing.tokenLabel}</label>
    <div class="row">
      <input
        id="pair-token"
        class="grow mono"
        type={shown ? "text" : "password"}
        bind:value={token}
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        disabled={busy}
        onkeydown={(e) => e.key === "Enter" && submit()}
      />
      <button class="ghost" type="button" onclick={() => (shown = !shown)}>
        {shown ? fr.pairing.hideInput : fr.pairing.showInput}
      </button>
    </div>
    <span class="muted small">{fr.pairing.tokenHint}</span>
  </div>

  {#if needsRelay}
    {@render relayField()}
  {:else}
    <details bind:open={otherOpen}>
      <summary>{fr.pairing.relayOther}</summary>
      {@render relayField()}
    </details>
  {/if}

  <div class="field">
    <label for="pair-create-name">{fr.pairing.deviceName}</label>
    <input
      id="pair-create-name"
      type="text"
      bind:value={name}
      maxlength="64"
      autocomplete="off"
      disabled={busy}
      onkeydown={(e) => e.key === "Enter" && submit()}
    />
    <span class="muted small">{fr.pairing.deviceNameHint}</span>
  </div>

  <!-- The relay that will be used, in full, before anything is sent (§4.4). -->
  <p class="relay-used" class:warn={retained === null}>
    {#if retained !== null}
      <strong class="mono">{format(fr.pairing.relayUsed, { relay: retained })}</strong>
      {#if relayRead}<span class="muted small">{fr.pairing.relayVerify}</span>{/if}
    {:else}
      {fr.pairing.relayNone}
    {/if}
  </p>

  {#if error}<p class="error" role="alert">{errorMessage(error)}</p>{/if}

  <div class="row">
    <button class="primary" disabled={!ready} onclick={submit}>
      {busy ? fr.pairing.creating : fr.pairing.createDo}
    </button>
    <button disabled={busy} onclick={onback}>{fr.pairing.back}</button>
  </div>
  {#if busy}<progress aria-label={fr.pairing.creating}></progress>{/if}
</div>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: 10px;
    align-items: flex-start;
  }
  p {
    margin: 0;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    align-self: stretch;
  }
  .error {
    color: var(--error);
  }
  .note {
    display: flex;
    flex-direction: column;
    gap: 2px;
    align-self: stretch;
    padding: 6px 10px;
    border-radius: var(--radius);
    background: var(--notice-bg);
  }
  .relay-used {
    display: flex;
    flex-direction: column;
    gap: 2px;
    align-self: stretch;
    overflow-wrap: anywhere;
  }
  .warn {
    color: var(--muted);
  }
  details {
    align-self: stretch;
  }
  summary {
    cursor: pointer;
    margin-bottom: 6px;
  }
  progress {
    width: 100%;
  }
</style>
