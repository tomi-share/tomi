<script lang="ts">
  import { untrack } from "svelte";
  import { api, type AppError, type InputRead } from "../api";
  import { errorMessage, fr } from "../i18n/fr";
  import { relayUrlOf } from "./relay";

  /**
   * Where the code of the other device is typed or pasted (spec/pairing-v1.md §4.4): a short
   * code, or the link of its QR code. The code is read by the core before anything is claimed,
   * so a typo consumes nothing.
   *
   * The relay (§4.4) comes from the link, from the "Relais" field, or is the one of this PC. One
   * that was read is not trusted: it is put in the field where the user sees it, and shown again
   * in full with the verification code. A PC that has no relay must be given one (the field is
   * open and required); one that has its relay keeps the field folded under "Autre relais".
   */
  let {
    initialCode = "",
    initialRelay = "",
    fromLink = false,
    relayAuthority = null,
    deviceName,
    busy,
    error,
    onsubmit,
    oncreate,
  }: {
    /** The code of a pairing link (`tomi-share://pair#` or `https://<relay>/l/pair#`): filled in, and nothing more. */
    initialCode?: string;
    /** The relay of that link, `https://host`: put in the "Relais" field. */
    initialRelay?: string;
    fromLink?: boolean;
    /** The relay this PC already has (`host[:port]`), `null` when it has none. */
    relayAuthority?: string | null;
    deviceName: string;
    busy: boolean;
    error: AppError | null;
    onsubmit: (codeOrLink: string, deviceName: string, relayUrl: string | null) => void;
    /** What was pasted is a creation link or a token: offer the creation screen with it. */
    oncreate?: (text: string) => void;
  } = $props();

  let code = $state(untrack(() => initialCode));
  let relay = $state(untrack(() => initialRelay));
  /** The relay was read from a link (opened or pasted): the note asks the user to check it. */
  let relayRead = $state(untrack(() => initialRelay !== ""));
  /** Set by the reading of a pasted link only: a link that goes away takes its relay with it. */
  let relayPasted = $state(false);
  /** The "Autre relais" part stays open once a link filled it. */
  let otherOpen = $state(untrack(() => initialRelay !== ""));
  /** What the pasted text is, once read by the core (links and tokens only). */
  let read = $state<InputRead | null>(null);
  let name = $state(untrack(() => deviceName));
  /** The word that the letters typed last name, offered to complete the code (§4.4). */
  let suggestion = $state<string | null>(null);

  const needsRelay = $derived(relayAuthority === null);
  // A link names its own relay, so the field may stay empty with one pasted.
  const relayKnown = $derived(relay.trim() !== "" || read?.relayUrl != null);
  const ready = $derived(
    code.trim() !== "" && name.trim() !== "" && (!needsRelay || relayKnown) && !busy,
  );

  // A pasted link or token is read by the core (nothing is sent): its relay fills the field.
  $effect(() => {
    const text = code;
    if (!text.includes("://") && text.trim().length < 24) {
      read = null;
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
        read = r;
        if (r.kind === "pairLink" && r.relayUrl) {
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

  /**
   * The word being typed, folded as the core folds a code (accents and case do not count): the
   * last run of letters, once a number and another word came before it, and only while the code
   * is not a link.
   */
  const typing = $derived.by(() => {
    if (code.includes("://")) return "";
    const tokens = code
      .normalize("NFD")
      .replace(/\p{Mn}/gu, "")
      .toLowerCase()
      .split(/[^a-z0-9]+/)
      .filter(Boolean);
    const last = tokens[tokens.length - 1] ?? "";
    return tokens.length >= 2 && /^[a-z]{4,8}$/.test(last) ? last : "";
  });

  $effect(() => {
    const prefix = typing;
    suggestion = null;
    if (prefix === "") return;
    let current = true;
    api
      .pairingCompleteWord(prefix)
      .then((word) => {
        // A word that is already complete needs no offer.
        if (current && word !== prefix) suggestion = word;
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  });

  /**
   * What the offered word adds to the letters already typed, drawn in grey after them, in the
   * field itself. Only when the code ends on those letters (nothing typed after the word) and
   * the word really continues them: otherwise nothing is drawn.
   */
  const ghost = $derived.by(() => {
    const word = suggestion;
    if (!word || typing === "" || !/[\p{L}\p{M}]$/u.test(code)) return "";
    const folded = word.normalize("NFD").replace(/\p{Mn}/gu, "").toLowerCase();
    return folded.startsWith(typing) ? word.slice(typing.length) : "";
  });

  function complete(word: string) {
    // The last run of letters, accents included, becomes the whole word.
    code = code.replace(/[\p{L}\p{M}]+([^\p{L}\p{M}]*)$/u, (_, tail: string) => word + tail);
  }

  function submit() {
    if (ready) onsubmit(code.trim(), name.trim(), relayUrlOf(relay));
  }

  function onCodeKey(e: KeyboardEvent) {
    if (e.key === "Enter") return submit();
    const input = e.currentTarget as HTMLInputElement;
    const atEnd = input.selectionStart === code.length && input.selectionEnd === code.length;
    // Tab, or the right arrow at the end of the text, takes the grey word as typed.
    if (ghost && suggestion && atEnd && (e.key === "Tab" || e.key === "ArrowRight")) {
      e.preventDefault();
      complete(suggestion);
    }
  }
</script>

{#snippet relayField()}
  <div class="field">
    <label for="pair-relay">{fr.pairing.relayLabel}</label>
    <input
      id="pair-relay"
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
  <h3>{fr.pairing.joinTitle}</h3>
  {#if fromLink}
    <div class="note">
      <strong>{fr.pairing.linkReceived}</strong>
      <span class="small">{fr.pairing.linkReceivedHint}</span>
    </div>
  {:else}
    <p class="muted">{fr.pairing.joinIntro}</p>
  {/if}

  <div class="field">
    <label for="pair-code">{fr.pairing.codeLabel}</label>
    <!-- The code is typed or pasted, never saved: no autofill, no spell check. -->
    <div class="codebox">
      <input
        id="pair-code"
        class="mono"
        type="text"
        bind:value={code}
        placeholder={fr.pairing.codePlaceholder}
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        disabled={busy}
        onkeydown={onCodeKey}
      />
      {#if ghost}
        <!-- The typed text is invisible (it only takes the room), the rest of the word is grey. -->
        <span class="ghost mono" aria-hidden="true"><span class="typed">{code}</span>{ghost}</span>
      {/if}
    </div>
    <span class="muted small">{fr.pairing.codeHint}</span>
  </div>

  {#if oncreate && read && (read.kind === "setupLink" || read.kind === "token")}
    <div class="note">
      <span class="small">{fr.pairing.setupLinkPasted}</span>
      <button type="button" disabled={busy} onclick={() => oncreate(code.trim())}>
        {fr.pairing.create}
      </button>
    </div>
  {/if}

  {#if needsRelay}
    {@render relayField()}
  {:else}
    <details bind:open={otherOpen}>
      <summary>{fr.pairing.relayOther}</summary>
      {@render relayField()}
    </details>
  {/if}

  <div class="field">
    <label for="pair-name">{fr.pairing.deviceName}</label>
    <input
      id="pair-name"
      type="text"
      bind:value={name}
      maxlength="64"
      autocomplete="off"
      disabled={busy}
      onkeydown={(e) => e.key === "Enter" && submit()}
    />
    <span class="muted small">{fr.pairing.deviceNameHint}</span>
  </div>

  {#if error}<p class="error" role="alert">{errorMessage(error)}</p>{/if}

  <div class="row">
    <button class="primary" disabled={!ready} onclick={submit}>
      {busy ? fr.pairing.joining : fr.pairing.joinDo}
    </button>
  </div>
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
  .note {
    display: flex;
    flex-direction: column;
    gap: 2px;
    align-self: stretch;
    padding: 6px 10px;
    border-radius: var(--radius);
    background: var(--notice-bg);
  }
  .error {
    color: var(--error);
  }
  details {
    align-self: stretch;
  }
  summary {
    cursor: pointer;
    margin-bottom: 6px;
  }
  .codebox {
    position: relative;
  }
  /* Laid over the field, same box as its text: it never takes any room of its own. */
  .ghost {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    padding: 0 calc(var(--space-3) + var(--border-width));
    white-space: pre;
    overflow: hidden;
    pointer-events: none;
    color: var(--muted);
    opacity: 0.7;
  }
  .typed {
    visibility: hidden;
  }
</style>
