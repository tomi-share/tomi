<script lang="ts">
  import { onMount, tick } from "svelte";
  import { api, asAppError, events, type Device, type Transfer } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { feed } from "../lib/feed.svelte";
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuEntry } from "../lib/Menu.svelte";
  import { errorMessage, format, msg, plural, relativeTime } from "../lib/i18n";
  import PairingPanel from "./PairingPanel.svelte";

  /** Finished transfers shown below the running ones. */
  const RECENT_FINISHED = 3;

  let devices = $state<Device[]>([]);
  let transfers = $state<Transfer[]>([]);
  let loaded = $state(false);
  /** Device the user is about to send the history to (the yes / no question). */
  let asking = $state<string | null>(null);
  /** Device being renamed, and what is typed. */
  let renaming = $state<string | null>(null);
  let nameDraft = $state("");
  let nameError = $state("");
  let nameInput: HTMLInputElement | undefined = $state();
  /** Device the user is about to revoke (the confirmation). Never this PC. */
  let revoking = $state<Device | null>(null);
  /** Device an action is running on (rename or revocation: network calls, a revocation takes a moment). */
  let busy = $state<{ id: string; what: "rename" | "revoke" } | null>(null);

  const offers = $derived(transfers.filter((t) => t.phase === "offered"));
  const running = $derived(transfers.filter((t) => t.phase === "running"));
  const finished = $derived(
    transfers
      .filter((t) => !["offered", "running"].includes(t.phase))
      .slice(-RECENT_FINISHED)
      .reverse(),
  );

  async function load() {
    try {
      [devices, transfers] = await Promise.all([api.devices(), api.transfers()]);
    } catch (e) {
      // Not paired is told by the banner of the window, not by an error here.
      if (feed.canSend) feed.fail(e);
    } finally {
      loaded = true;
    }
  }

  onMount(() => {
    void load();
    let gone = false;
    let unlisten: (() => void) | undefined;
    events.spaceChanged(() => void load()).then((u) => (gone ? u() : (unlisten = u)));
    return () => {
      gone = true;
      unlisten?.();
    };
  });

  async function sendHistory(deviceId: string) {
    asking = null;
    if (await feed.run(() => api.sendHistory(deviceId))) await load();
  }

  async function answer(t: Transfer, accept: boolean) {
    if (await feed.run(() => api.answerHistory(t.id, accept))) await load();
  }

  async function cancel(t: Transfer) {
    if (await feed.run(() => api.cancelTransfer(t.id))) await load();
  }

  /** The sentence for a failed device action. Without network nothing is queued, unlike a send. */
  function deviceError(e: unknown): string {
    const err = asAppError(e);
    return err.code === "error.offline" ? msg.devices.offline : errorMessage(err);
  }

  async function startRename(d: Device) {
    asking = null;
    renaming = d.id;
    nameDraft = d.name;
    nameError = "";
    await tick();
    nameInput?.focus();
    nameInput?.select();
  }

  function stopRename() {
    renaming = null;
    nameError = "";
  }

  /** The core and the relay judge the name; only an empty or unchanged one is not even sent. */
  async function saveName(d: Device) {
    const name = nameDraft.trim();
    if (busy || name === "" || name === d.name) return;
    busy = { id: d.id, what: "rename" };
    nameError = "";
    try {
      await api.renameDevice(d.id, name);
      stopRename();
      feed.say(msg.devices.renamed);
    } catch (e) {
      nameError = deviceError(e);
    } finally {
      busy = null;
      await load();
    }
  }

  function onNameKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      stopRename();
    }
  }

  /** The list is read again whatever happened: a refusal often means it was out of date. */
  async function revoke(d: Device) {
    revoking = null;
    // One action at a time: two revocations in parallel would race on the key rotation.
    if (busy) return;
    busy = { id: d.id, what: "revoke" };
    try {
      // Windows Hello is asked in Rust before the core is called (`error.auth_*` otherwise).
      await api.revokeDevice(d.id);
      feed.say(format(msg.devices.revoked, { device: d.name }));
    } catch (e) {
      // A prompt the user closed is not a failure: nothing was done, and the message says so.
      const cancelled = asAppError(e).code === "error.auth_cancelled";
      feed.say(deviceError(e), cancelled ? undefined : "error");
    } finally {
      busy = null;
      await load();
    }
  }

  function menuEntries(d: Device): MenuEntry[] {
    const entries: MenuEntry[] = [{ label: msg.devices.rename, icon: "edit", onselect: () => startRename(d) }];
    // This PC is never offered a revocation: the core refuses it too.
    if (!d.thisDevice) {
      entries.push({
        label: msg.devices.revoke,
        icon: "link_off",
        danger: true,
        separator: true,
        onselect: () => (revoking = d),
      });
    }
    return entries;
  }

  function platformLabel(p: string): string {
    return p === "windows" ? msg.devices.platforms.windows : p === "android" ? msg.devices.platforms.android : msg.devices.platforms.other;
  }

  function peerLabel(t: Transfer): string {
    return format(t.direction === "send" ? msg.transfer.toDevice : msg.transfer.fromDevice, {
      device: t.peer.name,
    });
  }
</script>

<section class="panel scroll">
 <div class="inner">
  <h1>{msg.devices.title}</h1>

  <div class="layout">
  <div class="side">
  <PairingPanel />

  {#each offers as t (t.id)}
    <div class="card offer">
      <strong>{format(msg.transfer.offer, { device: t.peer.name })}</strong>
      <p class="muted small">{msg.transfer.offerBody}</p>
      <div class="row">
        <button class="primary" onclick={() => answer(t, true)}>{msg.transfer.accept}</button>
        <button onclick={() => answer(t, false)}>{msg.transfer.decline}</button>
      </div>
    </div>
  {/each}
  </div>

  <div class="main">
  <ul class="list cards">
    {#each devices as d (d.id)}
      <li class="card">
        <div class="row">
          <span class="avatar" aria-hidden="true">
            <Icon name={d.platform === "android" ? "smartphone" : "computer"} size={24} />
          </span>
          <div class="grow">
            <div class="name">
              <strong>{d.name}</strong>
              {#if d.thisDevice}<span class="badge">{msg.devices.thisDevice}</span>{/if}
              {#if d.state === "unverified"}<span class="badge warn">{msg.devices.unverified}</span>{/if}
            </div>
            <div class="muted small">
              {platformLabel(d.platform)} ·
              {d.lastSeen ? format(msg.devices.lastSeen, { when: relativeTime(d.lastSeen) }) : msg.devices.never}
            </div>
          </div>
          {#if !d.thisDevice && d.state === "verified" && asking !== d.id && busy?.id !== d.id}
            <button onclick={() => (asking = d.id)}>{msg.transfer.send}</button>
          {/if}
          {#if busy === null}
            <Menu entries={menuEntries(d)} label={format(msg.devices.more, { device: d.name })} />
          {/if}
        </div>
        {#if busy?.id === d.id && busy.what === "revoke"}
          <p class="small muted" role="status">{msg.devices.revoking}</p>
        {:else if d.inactiveDays !== null && !d.thisDevice && d.state !== "revoked"}
          <!-- A hint, never an action taken: the usual confirmation follows. -->
          <div class="ask">
            <p class="small">
              {format(msg.devices.inactive, { duration: plural(msg.devices.inactiveDuration, d.inactiveDays) })}
            </p>
            <div class="row">
              <button disabled={busy !== null} onclick={() => (revoking = d)}>{msg.devices.revoke}</button>
            </div>
          </div>
        {/if}
        {#if renaming === d.id}
          <form
            class="ask"
            onsubmit={(e) => {
              e.preventDefault();
              void saveName(d);
            }}
          >
            <label class="small" for="rename-{d.id}">{msg.devices.renameLabel}</label>
            <input
              id="rename-{d.id}"
              type="text"
              bind:this={nameInput}
              bind:value={nameDraft}
              onkeydown={onNameKey}
              autocomplete="off"
              spellcheck="false"
              aria-invalid={nameError !== ""}
              aria-describedby="rename-help-{d.id}"
              disabled={busy?.id === d.id}
            />
            <p id="rename-help-{d.id}" class="small" class:muted={nameError === ""} class:error={nameError !== ""} role={nameError ? "alert" : undefined}>
              {nameError || msg.devices.renameHint}
            </p>
            <div class="row">
              <button class="primary" type="submit" disabled={busy !== null || nameDraft.trim() === "" || nameDraft.trim() === d.name}>
                {busy?.id === d.id ? msg.devices.renameSaving : msg.devices.renameSave}
              </button>
              <button type="button" onclick={stopRename} disabled={busy?.id === d.id}>{msg.common.cancel}</button>
            </div>
          </form>
        {/if}
        {#if asking === d.id}
          <!-- One question, yes or no: the whole history, no choice per type. -->
          <div class="ask">
            <p class="small">{format(msg.transfer.sendAsk, { device: d.name })}</p>
            <div class="row">
              <button class="primary" onclick={() => sendHistory(d.id)}>{msg.transfer.sendYes}</button>
              <button onclick={() => (asking = null)}>{msg.transfer.sendNo}</button>
            </div>
          </div>
        {/if}
      </li>
    {:else}
      {#if loaded}
        <li class="muted empty">
          {feed.status && !feed.status.paired ? msg.devices.notPaired : msg.devices.empty}
        </li>
      {/if}
    {/each}
  </ul>

  {#if revoking}
    {@const target = revoking}
    <Dialog
      title={format(msg.devices.revokeTitle, { device: target.name })}
      message={msg.devices.revokeMessage}
      confirmLabel={msg.devices.revoke}
      onconfirm={() => revoke(target)}
      oncancel={() => (revoking = null)}
    />
  {/if}

  {#if running.length > 0 || finished.length > 0}
    <h2>{msg.transfer.title}</h2>
    <ul class="list cards">
      {#each [...running, ...finished] as t (t.id)}
        <li class="card">
          <div class="row">
            <div class="grow">
              <strong>{peerLabel(t)}</strong>
              <span class="badge" class:warn={t.phase === "failed"}>{msg.transfer.phases[t.phase]}</span>
              <div class="muted small tnum">
                {format(msg.transfer.progress, { done: t.itemsDone, total: t.itemsTotal })}
                {#if t.filesTotal > 0}
                  · {format(msg.transfer.files, { done: t.filesDone, total: t.filesTotal })}
                {/if}
              </div>
              {#if t.phase === "running" && t.itemsTotal > 0}
                <progress value={t.itemsDone} max={t.itemsTotal} aria-label={peerLabel(t)}></progress>
              {/if}
            </div>
            {#if t.phase === "running"}
              <button class="ghost" onclick={() => cancel(t)}>{msg.transfer.cancel}</button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
  </div>
  </div>
 </div>
</section>

<style>
  /* The section scrolls over the whole width (its scrollbar sits at the window edge). */
  .inner {
    width: 100%;
    max-width: 1100px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  /* Narrow: one column, in the source order. Wide: pairing and offers in a side column,
     devices and history next to it, the cards on as many columns as fit. */
  .layout,
  .side,
  .main {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .main {
    min-width: 0;
  }
  @media (min-width: 900px) {
    .layout {
      display: grid;
      grid-template-columns: minmax(300px, 380px) minmax(0, 1fr);
      align-items: start;
      gap: var(--space-5);
    }
    .cards {
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
      align-items: start;
    }
  }
  h2 {
    margin-top: var(--space-2);
  }
  p {
    margin: var(--space-1) 0;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .card {
    background: var(--surface);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
  }
  .offer {
    border-color: var(--brand);
  }
  .empty {
    padding: var(--space-4);
    text-align: center;
  }
  .row {
    gap: var(--space-3);
  }
  /* Avatar of a device: a round chip with the icon of its platform. */
  .avatar {
    flex: none;
    width: 40px;
    height: 40px;
    display: grid;
    place-items: center;
    border-radius: var(--radius-full);
    background: var(--brand-container);
    color: var(--brand);
  }
  .name {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .ask {
    margin-top: var(--space-2);
    padding-top: var(--space-2);
    border-top: var(--border-width) solid var(--border);
  }
  .badge {
    font-size: var(--fs-caption);
    line-height: var(--lh-caption);
    background: var(--surface-alt);
    border-radius: var(--radius-full);
    padding: 0 var(--space-2);
  }
  .badge.warn {
    color: var(--secondary);
  }
  .ask input {
    margin-top: var(--space-1);
  }
  .error {
    color: var(--error);
  }
  progress {
    width: 100%;
    margin-top: var(--space-2);
  }
</style>