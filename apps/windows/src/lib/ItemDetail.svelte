<script lang="ts">
  import { api } from "./api";
  import Dialog from "./Dialog.svelte";
  import { feed } from "./feed.svelte";
  import { format, formatDateTime, formatPreciseDateTime, fr } from "./i18n/fr";
  import Icon from "./Icon.svelte";
  import { copyItem, deleteItem, openItem, saveItem } from "./itemActions";
  import { canArchive, consumedButShown, deliverySummary, itemActionFlags } from "./itemLogic";
  import ItemCard from "./ItemCard.svelte";

  /**
   * The detail screen (Android's ItemDetailScreen): the card in full, then the type, the expiry,
   * the pin and the status. Reached from the "⋮" menu, from a click on a card whose content is
   * cut ("Lire la suite"), or from a notification. Nothing more is shown of a hidden item than on its card.
   *
   * The actions are a visible toolbar in the header, not a
   * "⋮" menu: pin, archive | open, save, copy. Archiving or unarchiving goes back to the list.
   * "Supprimer…" is kept apart, under the information. Wide window: labelled buttons and the
   * information in a column on the right; narrow window (the popover): icons only, information below.
   */
  const item = $derived(feed.detail);
  let root: HTMLElement | undefined = $state();
  let confirmDelete = $state(false);

  $effect(() => {
    root?.focus();
  });

  // Opening a received item reads it (never a hidden one: showing it does).
  let marked: string | null = null;
  $effect(() => {
    const i = item;
    if (!i || marked === i.id) return;
    marked = i.id;
    if (!i.read && !i.outgoing && !i.secret && !i.unreadable) void api.markRead(i.id).catch(() => {});
  });

  // The item left (deleted here or elsewhere): back to the list once it was seen.
  let existed = false;
  $effect(() => {
    if (item) existed = true;
    else if (existed) feed.closeDetail();
  });

  const revealed = $derived(item ? feed.revealed[item.id] != null : false);
  const flags = $derived(item ? itemActionFlags(item, revealed) : null);
  /** Read once and already erased in the core, still shown: only "Masquer" makes sense. */
  const gone = $derived(item ? consumedButShown(item, revealed) : false);

  function close() {
    feed.closeDetail();
  }

  /** Archive / unarchive, then back to the list (the banner "Archivé · Annuler" is shown there). */
  async function archive() {
    if (!item) return;
    const id = item.id;
    const next = feed.neighborOf(id);
    // The user may have left (Escape) and opened another item while the core answered.
    if ((await feed.toggleArchive(item)) && feed.detailId === id) backToList(next);
  }

  /** Back to the list, the keyboard on the card that takes the place of the item. */
  function backToList(next: string | null) {
    close();
    void feed.focusCard(next);
  }

  /** Pinning an archived item unarchives it: it leaves the archives, so back to the list too. */
  async function pin() {
    if (!item) return;
    const id = item.id;
    const unarchives = item.archived && !item.pinned;
    const next = feed.neighborOf(id);
    if ((await feed.togglePin(item)) && unarchives && feed.detailId === id) backToList(next);
  }

  let deleting = false;
  async function remove(scope: "local" | "everywhere") {
    confirmDelete = false;
    if (!item || deleting) return;
    const id = item.id;
    deleting = true;
    try {
      if ((await deleteItem(item, scope)) && feed.detailId === id) close();
    } finally {
      deleting = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape" || (e.altKey && e.key === "ArrowLeft")) {
      e.preventDefault();
      e.stopPropagation();
      close();
      return;
    }
    const inField = e.target instanceof HTMLElement && e.target.closest("input, textarea, select");
    if (inField || !item) return;
    const ctrl = e.ctrlKey && !e.altKey && !e.shiftKey;
    const key = e.key.toLowerCase();
    // Ctrl+S and Ctrl+O would open the browser's own dialogs: always taken, done only when offered.
    if (ctrl && key === "s") {
      e.preventDefault();
      if (flags?.canSave && !gone) void saveItem(item);
    } else if (ctrl && key === "o") {
      e.preventDefault();
      if (flags?.canOpen && !gone) void openItem(item);
    } else if (ctrl && key === "c") {
      // With a selection, the plain copy stays (a hidden value blocks it on its own).
      if ((window.getSelection()?.toString() ?? "") !== "") return;
      if (flags?.canCopy && !gone) {
        e.preventDefault();
        void copyItem(item);
      }
    } else if (e.key === "Delete" && !e.ctrlKey && !e.altKey && !e.shiftKey && !gone) {
      e.preventDefault();
      confirmDelete = true;
    }
  }

  /** The "Précédent" button of the mouse goes back, like Alt+←. */
  function onMouseUp(e: MouseEvent) {
    if (e.button === 3) {
      e.preventDefault();
      close();
    } else if (e.button === 4) {
      e.preventDefault();
    }
  }

  /**
   * A toolbar is ONE Tab stop; ← / → (and Home / End) move between its buttons (roving tabindex).
   * The buttons come and go with the item: the stops are recomputed when the content changes.
   */
  function roving(node: HTMLElement) {
    let current: HTMLElement | null = null;
    const buttons = () => Array.from(node.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
    function sync() {
      const list = buttons();
      const lost = current !== null && !node.contains(current);
      if (!current || !list.includes(current as HTMLButtonElement)) current = list[0] ?? null;
      for (const b of list) b.tabIndex = b === current ? 0 : -1;
      // The focused button left (e.g. "Ouvrir" of a value that was masked again): the keyboard
      // would be left on <body>, where the shortcuts of the screen no longer answer.
      if (lost && document.activeElement === document.body) {
        (current ?? node.closest<HTMLElement>('[role="dialog"]'))?.focus();
      }
    }
    function onFocusIn(e: FocusEvent) {
      if (e.target instanceof HTMLButtonElement && buttons().includes(e.target)) {
        current = e.target;
        sync();
      }
    }
    function onKeyDown(e: KeyboardEvent) {
      if (e.ctrlKey || e.altKey || e.metaKey) return;
      const list = buttons();
      const at = list.findIndex((b) => b === document.activeElement);
      if (at < 0) return;
      let to = -1;
      if (e.key === "ArrowRight") to = (at + 1) % list.length;
      else if (e.key === "ArrowLeft") to = (at - 1 + list.length) % list.length;
      else if (e.key === "Home") to = 0;
      else if (e.key === "End") to = list.length - 1;
      if (to < 0) return;
      e.preventDefault();
      e.stopPropagation();
      list[to].focus();
    }
    const observer = new MutationObserver(sync);
    observer.observe(node, { childList: true, subtree: true, attributes: true, attributeFilter: ["disabled"] });
    node.addEventListener("focusin", onFocusIn);
    node.addEventListener("keydown", onKeyDown);
    sync();
    return {
      destroy() {
        observer.disconnect();
        node.removeEventListener("focusin", onFocusIn);
        node.removeEventListener("keydown", onKeyDown);
      },
    };
  }

  const rows = $derived.by(() => {
    const i = item;
    if (!i) return [];
    const type = i.unreadable
      ? fr.card.unreadableShort
      : i.secret
        ? fr.card.secret
        : fr.kinds[i.kind];
    const expiry =
      i.readPolicy === "afterFirstRead"
        ? fr.detail.meta.afterRead
        : i.expiresAt !== null
          ? formatDateTime(i.expiresAt)
          : fr.detail.meta.never;
    const s = deliverySummary(i);
    const status =
      s.state === "received"
        ? fr.detail.meta.received
        : s.state === "queued"
          ? fr.card.status.queued
          : s.state === "failed"
            ? fr.card.status.failed
            : s.state === "sent"
              ? fr.card.status.sent
              : s.state === "delivered"
                ? s.delivered === s.total
                  ? fr.card.status.deliveredAll
                  : format(fr.card.status.deliveredSome, { delivered: s.delivered, total: s.total })
                : format(fr.card.status.expired, { delivered: s.delivered, devices: s.devices.join(", ") });
    return [
      { key: fr.detail.meta.type, value: type, pin: false },
      { key: fr.detail.meta.sentAt, value: formatPreciseDateTime(i.createdAt), pin: false },
      { key: fr.detail.meta.expiry, value: expiry, pin: false },
      { key: fr.detail.meta.pinned, value: i.pinned ? fr.detail.meta.yes : fr.detail.meta.no, pin: i.pinned },
      { key: fr.detail.meta.status, value: status, pin: false },
    ];
  });

  const copyLabel = $derived(flags?.linkOnly ? fr.card.copyLink : fr.card.copy);
  const archiveLabel = $derived(item?.archived ? fr.card.unarchive : fr.card.archive);
  const pinLabel = $derived(item?.pinned ? fr.card.unpin : fr.card.pin);
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  bind:this={root}
  class="screen"
  role="dialog"
  aria-modal="true"
  aria-label={fr.detail.title}
  tabindex="-1"
  onkeydown={onKey}
  onmouseup={onMouseUp}
>
  <header class="bar">
    <button
      class="ghost back"
      type="button"
      title="{fr.detail.back} ({fr.detail.keys.back})"
      aria-label={fr.detail.back}
      onclick={close}
    >
      <Icon name="arrow_back" size={24} />
    </button>
    <span class="esc muted small" aria-hidden="true">{fr.detail.keys.back}</span>
    <h2 class="grow ellipsis">{fr.detail.title}</h2>
    {#if item && flags && !gone}
      <div class="tools" role="toolbar" aria-label={fr.detail.actions} use:roving>
        <button
          class="ghost tool"
          class:on={item.pinned}
          type="button"
          title={pinLabel}
          aria-label={pinLabel}
          aria-pressed={item.pinned}
          onclick={() => void pin()}
        >
          <Icon name={item.pinned ? "push_pin_fill1" : "push_pin"} size={20} />
          <span class="lbl">{pinLabel}</span>
        </button>
        {#if canArchive(item)}
          <button
            class="ghost tool"
            type="button"
            title={archiveLabel}
            aria-label={archiveLabel}
            onclick={() => void archive()}
          >
            <Icon name={item.archived ? "unarchive" : "archive"} size={20} />
            <span class="lbl">{archiveLabel}</span>
          </button>
        {/if}
        {#if flags.canOpen || flags.canSave}<span class="sep" aria-hidden="true"></span>{/if}
        {#if flags.canOpen}
          <button
            class="ghost tool"
            type="button"
            title="{fr.card.open} ({fr.detail.keys.open})"
            aria-label={fr.card.open}
            onclick={() => void openItem(item)}
          >
            <Icon name="open_in_new" size={20} />
            <span class="lbl">{fr.card.open}</span>
          </button>
        {/if}
        {#if flags.canSave}
          <button
            class="ghost tool"
            type="button"
            title="{fr.card.save} ({fr.detail.keys.save})"
            aria-label={fr.card.save}
            onclick={() => void saveItem(item)}
          >
            <Icon name="download" size={20} />
            <span class="lbl">{fr.card.save}</span>
          </button>
        {/if}
        {#if flags.canCopy}
          <button
            class="primary tool copy"
            type="button"
            title="{copyLabel} ({fr.detail.keys.copy})"
            onclick={() => void copyItem(item)}
          >
            <Icon name={flags.linkOnly ? "link" : "content_copy"} size={20} />
            {copyLabel}
          </button>
        {/if}
      </div>
    {/if}
  </header>

  <div class="scroll body">
    {#if item}
      <div class="layout">
        <div class="main">
          <ItemCard {item} expanded showMenu={false} />
          {#if item.secret && !revealed}
            <p class="muted small hint">{fr.detail.maskedHint}</p>
          {/if}
        </div>
        <aside class="side" aria-labelledby="detail-info">
          <h3 id="detail-info" class="muted small">{fr.detail.info}</h3>
          <dl class="meta">
            {#each rows as r (r.key)}
              <div class="row-meta">
                <dt class="muted">{r.key}</dt>
                <dd>
                  {#if r.pin}<Icon name="push_pin_fill1" size={16} />{/if}
                  {r.value}
                </dd>
              </div>
            {/each}
          </dl>
          {#if !gone}
            <button
              class="ghost danger delete"
              type="button"
              title="{fr.detail.deleteMenu} ({fr.detail.keys.delete})"
              onclick={() => (confirmDelete = true)}
            >
              <Icon name="delete" size={20} />
              {fr.detail.deleteMenu}
            </button>
          {/if}
        </aside>
      </div>
    {:else if feed.detailId !== null}
      <p class="not-found">{fr.detail.notFound}</p>
    {/if}
  </div>
</div>

{#if confirmDelete && item}
  <Dialog
    title={fr.detail.deleteTitle}
    message={item.secret ? fr.detail.deleteSecretMessage : fr.detail.deleteMessage}
    secondaryLabel={fr.card.deleteLocal}
    onsecondary={() => void remove("local")}
    confirmLabel={fr.card.deleteEverywhere}
    onconfirm={() => void remove("everywhere")}
    oncancel={() => (confirmDelete = false)}
  />
{/if}

<style>
  .screen {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    outline: none;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
  }
  .bar h2 {
    min-width: 0;
  }
  .back {
    width: 36px;
    padding: 0;
    color: var(--muted);
  }
  /* "Échap" next to "Retour": only where there is room (the popover keeps the tooltip). */
  .esc {
    display: none;
    padding: 1px var(--space-2);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-sm, 4px);
  }
  .body {
    flex: 1;
    min-height: 0;
    padding: var(--space-1) var(--space-4) var(--space-5);
  }

  /* The toolbar: icons only until the window is wide, "Copier" always keeps its word. */
  .tools {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }
  .tool {
    width: 36px;
    padding: 0;
    color: var(--muted);
  }
  .tool.on {
    color: var(--brand);
  }
  .tool.copy {
    width: auto;
    padding: 0 var(--space-3);
    color: var(--on-brand);
  }
  .tool:hover:not(:disabled):not(.copy) {
    color: var(--text);
  }
  .lbl {
    display: none;
  }
  .sep {
    width: var(--border-width);
    height: 20px;
    margin: 0 var(--space-1);
    background: var(--border);
  }

  .layout {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: 720px;
    margin: 0 auto;
  }
  .main {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    min-width: 0;
  }
  .side {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    min-width: 0;
  }
  .side h3 {
    margin: 0;
    font-weight: 500;
  }
  .delete {
    align-self: flex-start;
  }
  .hint,
  .not-found {
    margin: 0;
  }
  .not-found {
    padding: var(--space-6) var(--space-4);
    text-align: center;
  }
  .meta {
    margin: 0;
    padding: 0 var(--space-4);
    background: var(--surface);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-md);
  }
  .row-meta {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) 0;
  }
  .row-meta + .row-meta {
    border-top: var(--border-width) solid var(--border);
  }
  dt,
  dd {
    margin: 0;
  }
  dd {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    text-align: right;
  }

  @media (min-width: 760px) {
    .tool {
      width: auto;
      padding: 0 var(--space-3);
    }
    .lbl {
      display: inline;
    }
    .esc {
      display: inline;
    }
    .layout {
      display: grid;
      grid-template-columns: minmax(0, 1fr) 272px;
      align-items: start;
      gap: var(--space-5);
      max-width: 1040px;
    }
    .side {
      position: sticky;
      top: 0;
    }
  }
</style>
