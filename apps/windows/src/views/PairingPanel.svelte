<script lang="ts">
  import { onMount } from "svelte";
  import { api, asAppError, type AppError } from "../lib/api";
  import { feed } from "../lib/feed.svelte";
  import { errorMessage, fr } from "../lib/i18n/fr";
  import { pairing } from "../lib/pairing.svelte";
  import Create from "../lib/pairing/Create.svelte";
  import Invite from "../lib/pairing/Invite.svelte";
  import Join from "../lib/pairing/Join.svelte";
  import JoinForm from "../lib/pairing/JoinForm.svelte";

  /**
   * The pairing screens (spec/pairing-v1.md): "Ajouter un appareil" on a PC that belongs to a
   * space; "Créer mon espace" or "Rejoindre un espace" on one that does not. The pairing runs
   * in the core; this reads it and carries the decisions of the user.
   */

  /**
   * A PC that is not paired always offers both ways, "Rejoindre" (the default) and "Créer": the
   * relay may hold several spaces (one per person), so `GET /v1/hello` says nothing about the
   * space of whoever installs this PC. The install token is the real gate; a wrong one
   * (`install_token_invalid`) or a space already made for it (`space_exists`) is told after the
   * attempt.
   */
  type Mode = "join" | "create";
  let mode = $state<Mode>("join");
  /** A command that starts something is in flight (the code is being made, the claim is made...). */
  let busy = $state(false);
  /** Why the last start was refused (a code that is not one, a wrong token...). */
  let failure = $state<AppError | null>(null);
  /**
   * What fills the creation form: the token of a creation link that opened the app, or a text
   * pasted in the join form that turned out to be one. A secret, dropped when the form is left.
   * The relay it names is not trusted: the form shows it in full before "Créer l'espace".
   */
  let createSeed = $state<{ token: string; relay: string } | null>(null);

  const view = $derived(pairing.view);
  const options = $derived(pairing.options);

  onMount(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    void pairing.start().then((s) => (gone ? s() : (stop = s)));
    return () => {
      gone = true;
      stop?.();
      // Leaving the screen abandons a code on show or codes being compared (§11.2): the code
      // does not stay valid for a screen that is gone. A pairing both users confirmed goes on,
      // and a finished one is closed; the core decides which is which. Always told, even when
      // the view still reads "none": a start may be in flight (reserved, not yet installed),
      // and the core must get the chance to drop it.
      void api.pairingLeave().catch(() => {});
      // Drop the secrets this window held (code, link, verification code) from JS memory; a
      // reopened window reads them again from the core.
      pairing.view = { role: "none" };
      pairing.link = null;
      pairing.setup = null;
    };
  });

  // A pairing link fills the form and does nothing more (§4.4); a PC that already belongs to
  // a space ignores it, with a word.
  $effect(() => {
    if (pairing.link === null || options === null) return;
    if (options.paired) {
      feed.say(fr.pairing.linkIgnored);
      pairing.link = null;
    } else {
      mode = "join";
    }
  });

  // A creation link fills the creation form and does nothing more (§10); same for a PC that is paired.
  $effect(() => {
    if (pairing.setup === null || options === null) return;
    if (options.paired) {
      feed.say(fr.pairing.linkIgnored);
    } else {
      createSeed = { token: pairing.setup.token, relay: pairing.setup.relayUrl };
      mode = "create";
    }
    pairing.setup = null;
  });

  // While the relay does not answer, look again so the "relais injoignable" note goes away once it does.
  $effect(() => {
    if (!options || options.paired || options.spaceInitialized !== null || pairing.active) return;
    const t = setInterval(() => void pairing.refreshOptions(), 8000);
    return () => clearInterval(t);
  });

  function goTo(next: Mode) {
    failure = null;
    // The seed is a secret: it does not outlive the form it filled.
    if (next === "join") createSeed = null;
    mode = next;
  }

  /** What was pasted in the join form is a creation link or a token: the creation form takes it. */
  function createWith(text: string) {
    failure = null;
    createSeed = { token: text, relay: "" };
    mode = "create";
  }

  async function invite() {
    failure = null;
    busy = true;
    try {
      pairing.view = await api.pairingInvite();
    } catch (e) {
      failure = asAppError(e);
    } finally {
      busy = false;
    }
  }

  async function join(codeOrLink: string, deviceName: string, relayUrl: string | null) {
    failure = null;
    busy = true;
    try {
      pairing.view = await api.pairingJoin(codeOrLink, deviceName, relayUrl);
      pairing.link = null;
    } catch (e) {
      failure = asAppError(e);
    } finally {
      busy = false;
    }
  }

  async function create(installToken: string, deviceName: string, relayUrl: string | null) {
    failure = null;
    busy = true;
    try {
      await api.pairingCreateSpace(installToken, deviceName, relayUrl);
      feed.say(fr.pairing.created);
      createSeed = null;
      mode = "join";
    } catch (e) {
      failure = asAppError(e);
    } finally {
      busy = false;
    }
    // Paired now, or told that a space exists after all: the screen follows.
    await Promise.all([pairing.refreshOptions(), feed.refreshStatus()]);
  }

  /** The codes are the same. `true` once the decision is taken. */
  async function confirm(importHistory: boolean): Promise<boolean> {
    try {
      await api.pairingConfirm(importHistory);
      return true;
    } catch (e) {
      feed.fail(e);
      return false;
    }
  }

  /** Refuses, cancels, or closes a pairing that is over: back to where it started. */
  async function leave() {
    await feed.run(() => api.pairingCancel());
    await pairing.refreshView();
  }

  async function finish() {
    await feed.run(() => api.pairingDismiss());
    await pairing.refreshView();
  }

  async function again() {
    await finish();
    await invite();
  }

  async function acknowledge() {
    await feed.run(() => api.pairingDismiss());
    await pairing.refreshOptions();
  }
</script>

<!-- Announced as it changes: a screen reader reads the verification code when it appears. -->
<section class="pairing" aria-live="polite">
  {#if options?.resumed}
    <div class="card notice row">
      <span class="grow">
        {options.resumed === "provisioned"
          ? fr.pairing.resumedProvisioned
          : fr.pairing.resumedDiscarded}
      </span>
      <button onclick={acknowledge}>{fr.pairing.ok}</button>
    </div>
  {/if}

  {#if view.role === "invite"}
    <div class="card">
      <Invite
        session={view}
        onconfirm={confirm}
        onrefuse={leave}
        onfinish={finish}
        onagain={again}
      />
    </div>
  {:else if view.role === "join"}
    <div class="card">
      <Join session={view} onconfirm={confirm} onrefuse={leave} onfinish={finish} />
    </div>
  {:else if options === null}
    <p class="muted">{fr.common.loading}</p>
  {:else if options.paired}
    <div class="row">
      <button class="primary" disabled={busy} onclick={invite}>
        {busy ? fr.pairing.preparing : fr.pairing.addDevice}
      </button>
    </div>
    {#if failure}<p class="error" role="alert">{errorMessage(failure)}</p>{/if}
  {:else}
    <div class="card">
      {#if options.pending}<p class="muted">{fr.pairing.pending}</p>{/if}
      {#if mode === "create"}
        {#key createSeed}
          <Create
            deviceName={options.deviceName}
            relayAuthority={options.relayAuthority}
            initialToken={createSeed?.token ?? ""}
            initialRelay={createSeed?.relay ?? ""}
            {busy}
            error={failure}
            onsubmit={create}
            onback={() => goTo("join")}
          />
        {/key}
      {:else}
        {#if options.relayAuthority !== null && options.spaceInitialized === null}
          <p class="muted small">{fr.pairing.relayUnknown}</p>
        {/if}
        <!-- A link that arrives while the form is open replaces what is typed. -->
        {#key pairing.link}
          <JoinForm
            initialCode={pairing.link?.code ?? ""}
            initialRelay={pairing.link?.relayUrl ?? ""}
            fromLink={pairing.link !== null}
            relayAuthority={options.relayAuthority}
            deviceName={options.deviceName}
            {busy}
            error={failure}
            onsubmit={join}
            oncreate={createWith}
          />
        {/key}
        <!-- Always offered: the first device of a person has nobody to ask for a code. -->
        <div class="create-entry">
          <span class="muted small">{fr.pairing.createHint}</span>
          <button disabled={busy} onclick={() => goTo("create")}>{fr.pairing.create}</button>
        </div>
      {/if}
    </div>
  {/if}
</section>

<style>
  .pairing {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  p {
    margin: 0;
  }
  .card {
    background: var(--surface);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .notice {
    border-color: var(--brand);
  }
  .error {
    color: var(--error);
  }
  .create-entry {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    padding-top: 8px;
    border-top: 1px solid var(--border);
  }
</style>
