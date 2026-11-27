<script lang="ts">
  import { api, type AttachmentView, type ItemView } from "./api";
  import { cutWatch, fitName, seenWatch } from "./actions";
  import { clock } from "./clock.svelte";
  import Dialog from "./Dialog.svelte";
  import { feed } from "./feed.svelte";
  import { msg, format, formatBytes, formatRemainingTime, formatTimestamp, plural } from "./i18n";
  import Icon from "./Icon.svelte";
  import { copyItem, deleteItem, openItem, removeAttachment, saveItem } from "./itemActions";
  import {
    NO_OVERFLOW,
    attachmentPlan,
    badgeExtension,
    consumedButShown,
    deliverySummary,
    isCodeFile,
    canArchive,
    canRemoveFromBatch,
    isImage,
    itemActionFlags,
    kindIcon,
    rowAction,
    showsCopyButton,
    showsReadMore,
    tapAction,
    type CardTapAction,
  } from "./itemLogic";
  import Menu, { type MenuEntry } from "./Menu.svelte";
  import RichText from "./RichText.svelte";

  /**
   * An item of the feed. The same card is the body of the detail screen (`expanded`: nothing is
   * cut, and the menu is the toolbar of the screen instead).
   *
   * A click does the natural action of the type (see `tapAction`, the table of Android's
   * CardTap.kt): a link opens, a hidden item is shown, a cut content opens the detail, an image
   * and a code file open in Tomi, another single file opens with its application. Everything the
   * click does is also in the "⋮" menu or on a visible button, for the keyboard.
   */
  let {
    item,
    expanded = false,
    showMenu = true,
  }: { item: ItemView; expanded?: boolean; showMenu?: boolean } = $props();

  let titleCut = $state(false);
  let bodyCut = $state(false);
  let confirmDelete = $state(false);
  let menu: Menu | undefined = $state();
  let el: HTMLElement | undefined = $state();

  const shown = $derived(feed.revealed[item.id] ?? null);
  const revealed = $derived(shown !== null);
  const unread = $derived(!item.read && !item.outgoing);
  /**
   * The "Non lu" dot stays a few seconds after the item is read by being seen, so that the user
   * can tell what was new (the count of the tray is updated at once).
   */
  let lingering = $state(false);
  const showUnread = $derived(unread || lingering);
  $effect(() => {
    if (unread) {
      lingering = true;
    } else if (lingering) {
      const t = setTimeout(() => (lingering = false), UNREAD_LINGER_MS);
      return () => clearTimeout(t);
    }
  });
  /** Reading by looking (see `seenWatch`): never a hidden item, which "Afficher" reads. */
  const readable = $derived(
    !expanded && !item.read && !item.outgoing && !item.secret && !item.unreadable,
  );
  const focused = $derived(feed.focusId === item.id);
  /** The item is already erased in the core but still shown (read once): only "Masquer" makes sense. */
  const gone = $derived(consumedButShown(item, revealed));
  const overflow = $derived(expanded ? NO_OVERFLOW : { title: titleCut, body: bodyCut });
  const tap = $derived(tapAction(item, revealed, overflow));
  const readMore = $derived(!expanded && showsReadMore(item, revealed, overflow));
  const kindLabel = $derived(
    item.unreadable ? msg.card.unreadableShort : item.secret ? msg.card.secret : msg.kinds[item.kind],
  );

  $effect(() => {
    if (focused && el) el.scrollIntoView({ block: "nearest", behavior: "smooth" });
  });

  // What the card measured about a hidden item is only valid while it is shown.
  $effect(() => {
    void revealed;
    titleCut = false;
    bodyCut = false;
  });

  // The countdown ("expire dans…") refreshes by itself; only cards that expire keep the shared clock running.
  $effect(() => {
    if (item.expiresAt === null || item.readPolicy === "afterFirstRead") return;
    return clock.subscribe();
  });

  const footer = $derived.by(() => {
    if (item.unreadable) return null;
    const parts: string[] = [];
    const s = deliverySummary(item);
    let icon: "schedule" | "error" | "done_all" | "check" | null = null;
    let tone: "secondary" | "error" | "success" | "muted" = "muted";
    switch (s.state) {
      case "queued":
        parts.push(msg.card.status.queued);
        icon = "schedule";
        tone = "secondary";
        break;
      case "failed":
        parts.push(msg.card.status.failed);
        icon = "error";
        tone = "error";
        break;
      case "sent":
        parts.push(msg.card.status.sent);
        icon = "check";
        break;
      case "delivered":
        icon = "done_all";
        if (s.delivered === s.total) {
          // Complete delivery says nothing new on a feed card: only the detail screen keeps it
          // (Android: CardFooterLogic). The other states stay.
          if (expanded) {
            parts.push(msg.card.status.deliveredAll);
            tone = "success";
          } else {
            icon = null;
          }
        } else {
          parts.push(format(msg.card.status.deliveredSome, { delivered: s.delivered, total: s.total }));
        }
        break;
      case "expired":
        parts.push(
          format(msg.card.status.expired, { delivered: s.delivered, devices: s.devices.join(", ") }),
        );
        icon = "done_all";
        break;
    }
    if (item.readPolicy === "afterFirstRead") {
      parts.push(msg.card.afterRead);
    } else if (item.expiresAt !== null) {
      parts.push(
        item.expiresAt <= clock.now
          ? msg.card.expired
          : format(msg.card.expiresIn, { when: formatRemainingTime(item.expiresAt - clock.now) }),
      );
    }
    return parts.length > 0 ? { text: parts.join(" · "), icon, tone } : null;
  });

  // ---- actions

  /** Marks an incoming item read, like a tap on the card does (never a hidden one: showing it does). */
  function opened() {
    if (!item.read && !item.outgoing && !item.secret && !item.unreadable) {
      void api.markRead(item.id).catch(() => {});
    }
  }

  const copy = () => copyItem(item);
  const openIt = () => openItem(item);
  const save = () => saveItem(item);

  async function removeLocal() {
    confirmDelete = false;
    if (await deleteItem(item, "local")) {
      if (expanded) feed.closeDetail();
    }
  }

  async function removeEverywhere() {
    confirmDelete = false;
    if (await deleteItem(item, "everywhere")) {
      if (expanded) feed.closeDetail();
    }
  }

  function perform(action: CardTapAction) {
    opened();
    switch (action) {
      case "openLink":
      case "openAttachment":
        return void openIt();
      case "reveal":
        return void feed.reveal(item);
      case "hide":
        return feed.hideValues(item.id);
      case "openCodeFile":
        return feed.openCode(item.id, item.attachments[0]?.slot ?? 0);
      case "openImage":
        return feed.openImage(item.id, item.attachments[0]?.slot ?? 0);
      case "openDetail":
        return void feed.openDetail(item.id);
      case "none":
        return;
    }
  }

  function onContentClick(e: MouseEvent) {
    if (expanded) return;
    const target = e.target as HTMLElement;
    // A button, a link or the preview inside the card has its own action; selecting text is not a tap.
    if (target.closest("button, a, input, iframe")) return;
    if ((window.getSelection()?.toString() ?? "") !== "") return;
    perform(tap);
  }

  function onContext(e: MouseEvent) {
    if (!showMenu || gone || !menu) return;
    e.preventDefault();
    menu.openAt(e.clientX, e.clientY);
  }

  /** A plain Ctrl+C would copy the value without the "sensitive" clipboard formats. */
  function blockCopy(e: ClipboardEvent) {
    e.preventDefault();
  }

  // Drag a file out of the window (Explorer, mail...): a pressed move of a few pixels on its row.
  let dragFrom: { x: number; y: number } | null = null;
  let dragged = false;
  function dragDown(e: MouseEvent) {
    if (e.button !== 0) return;
    dragFrom = { x: e.clientX, y: e.clientY };
    dragged = false;
  }
  function dragMove(e: MouseEvent) {
    if (!dragFrom) return;
    if (e.buttons !== 1) {
      dragFrom = null;
      return;
    }
    if (Math.hypot(e.clientX - dragFrom.x, e.clientY - dragFrom.y) > 8) {
      dragFrom = null;
      dragged = true;
      void api.startDrag(item.id).catch(() => {});
    }
  }

  /** A click on an attachment (tile, frame, mosaic cell): its own viewer, or nothing for an inert file. */
  function onRow(act: ReturnType<typeof rowAction>, slot: number) {
    if (dragged) {
      dragged = false;
      return;
    }
    opened();
    if (act === "openImage") feed.openImage(item.id, slot);
    else if (act === "openCodeFile") feed.openCode(item.id, slot);
  }

  /** "N autres fichiers ›" and the "+N" of a mosaic: the detail (opening it counts as opening the card). */
  function openMore() {
    opened();
    void feed.openDetail(item.id);
  }

  /** The attachments of a revealed hidden item, shaped like the others (no thumbnail, no action). */
  const revealedFiles = $derived<AttachmentView[]>(
    (shown?.attachments ?? []).map((a) => ({ ...a, mime: "", thumbnail: null, available: true })),
  );
  /** A tap on the card opens the single generic file with its application: its tile shows the outgoing arrow. */
  const opensExternally = $derived(!expanded && tap === "openAttachment");
  /** Unread without a title: the names carry the weight the title would (semi-bold). */
  const emphasizeNames = $derived(showUnread && !item.title);

  // ---- removal of ONE file of a batch (the detail only; the item stays)

  /** The rows of the files get a "⋮": a batch of 2 or more, readable, already out of this PC. */
  const rowMenus = $derived(expanded && canRemoveFromBatch(item) && !gone);
  /** Slot of the file whose "partout" removal waits for its confirmation. */
  let confirmFileSlot = $state<number | null>(null);
  /** Slot of the file whose "de cet appareil" removal waits for its confirmation (it cannot be undone either). */
  let confirmLocalFileSlot = $state<number | null>(null);
  let removingFile = false;
  $effect(() => {
    if (expanded) void feed.refreshRemovalSupport();
  });

  function rowEntries(a: AttachmentView): MenuEntry[] {
    const list: MenuEntry[] = [
      { label: msg.card.fileDeleteLocal, icon: "delete", onselect: () => (confirmLocalFileSlot = a.slot) },
    ];
    if (feed.removalEverywhere) {
      list.push({
        label: msg.card.fileDeleteEverywhere,
        icon: "delete_forever",
        danger: true,
        onselect: () => (confirmFileSlot = a.slot),
      });
    }
    return list;
  }

  async function removeFile(slot: number, a: Pick<AttachmentView, "mime">, everywhere: boolean) {
    if (removingFile) return;
    removingFile = true;
    try {
      const removal = await removeAttachment(item, slot, everywhere, isImage(a));
      if (removal === "itemDeleted") {
        if (expanded) feed.closeDetail();
      } else if (removal && item.secret) {
        // What "Afficher" showed lists the old files: masked again, shown anew on request.
        feed.hideValues(item.id);
      }
    } finally {
      removingFile = false;
    }
  }

  function confirmFile() {
    const slot = confirmFileSlot;
    confirmFileSlot = null;
    if (slot === null) return;
    const a = (item.secret ? revealedFiles : item.attachments).find((f) => f.slot === slot);
    if (a) void removeFile(slot, a, true);
  }

  function confirmLocalFile() {
    const slot = confirmLocalFileSlot;
    confirmLocalFileSlot = null;
    if (slot === null) return;
    const a = (item.secret ? revealedFiles : item.attachments).find((f) => f.slot === slot);
    if (a) void removeFile(slot, a, false);
  }

  const confirmFileName = $derived(
    confirmFileSlot === null
      ? ""
      : ((item.secret ? revealedFiles : item.attachments).find((f) => f.slot === confirmFileSlot)?.name ?? ""),
  );

  // ---- menu

  const flags = $derived(itemActionFlags(item, revealed));
  const hasFiles = $derived(flags.hasFiles);
  const canOpen = $derived(flags.canOpen);
  const linkOnly = $derived(flags.linkOnly);
  const canCopy = $derived(flags.canCopy);

  // Same entries and order as Android (`ItemMenuLogic`): what the card does with one tap (its copy icon, the tap that opens a link) is
  // not repeated first, and the two deletions are ONE entry that asks where.
  const entries = $derived.by((): MenuEntry[] => {
    const list: MenuEntry[] = [];
    if (canCopy && !showsCopyButton(item)) {
      list.push({
        label: linkOnly ? msg.card.copyLink : msg.card.copy,
        icon: linkOnly ? "link" : "content_copy",
        onselect: () => void copy(),
      });
    }
    if (canOpen) {
      list.push({ label: msg.card.open, icon: "open_in_new", onselect: () => void openIt() });
    }
    if (flags.canSave) {
      list.push({ label: msg.card.save, icon: "download", onselect: () => void save() });
    }
    list.push({
      label: item.pinned ? msg.card.unpin : msg.card.pin,
      icon: item.pinned ? "push_pin_fill1" : "push_pin",
      onselect: () => void feed.togglePin(item),
    });
    // A send still queued or failed stays visible: no entry.
    if (canArchive(item)) {
      list.push({
        label: item.archived ? msg.card.unarchive : msg.card.archive,
        icon: item.archived ? "unarchive" : "archive",
        onselect: () => void feed.toggleArchive(item),
      });
    }
    if (!expanded) {
      list.push({
        label: msg.card.details,
        icon: "info",
        onselect: () => {
          opened();
          void feed.openDetail(item.id);
        },
      });
    }
    list.push({
      label: msg.detail.deleteMenu,
      icon: "delete",
      separator: true,
      danger: true,
      onselect: () => (confirmDelete = true),
    });
    return list;
  });

  const tapLabel = $derived.by(() => {
    switch (tap) {
      case "openLink":
      case "openAttachment":
        return msg.card.open;
      case "openImage":
        return msg.card.viewImage;
      case "openCodeFile":
        return msg.card.viewCode;
      case "reveal":
        return msg.card.reveal;
      case "hide":
        return msg.card.hide;
      case "openDetail":
        return msg.card.details;
      case "none":
        return undefined;
    }
  });

  const UNREAD_LINGER_MS = 3_000;
  const MASK = "••••••••••";

  function hintOf(act: ReturnType<typeof rowAction>): string {
    return act === "openImage" ? msg.card.viewImage : act === "openCodeFile" ? msg.card.viewCode : msg.card.dragOut;
  }
</script>

<!--
  Attachments of a card (feed and detail), as on Android: one attachment = ONE object with ONE click target (a tile, or an image frame), plus the
  mosaic of an image batch. What a click does is unchanged (`rowAction`): only the presentation.
-->

<!-- A file name: whole (detail, wraps even without a space) or cut before its extension (feed). -->
{#snippet nameText(name: string, lines: 1 | 2, strong: boolean)}
  {#if expanded}
    <span class="nm" class:strong>{name}</span>
  {:else}
    <span class="nm" class:strong>
      <span class="sr-only">{name}</span>
      <span class="cut" style:--lines={lines} aria-hidden="true" use:fitName={{ name }}></span>
    </span>
  {/if}
{/snippet}

<!-- File tile: type badge 40x40, name, size below, action on the right (chevron: Tomi's viewer; arrow: the application). -->
{#snippet tile(a: AttachmentView, index: number, act: ReturnType<typeof rowAction>, onAlt: boolean)}
  {@const img = isImage(a)}
  {@const thumb = !onAlt && img ? a.thumbnail : null}
  {@const ext = img ? null : badgeExtension(a.name)}
  {@const tonal = act !== "none"}
  <svelte:element
    this={act === "none" ? "div" : "button"}
    class="tile"
    class:alt={onAlt}
    class:openable={tonal}
    type={act === "none" ? undefined : "button"}
    title={onAlt ? a.name : expanded ? hintOf(act) : `${a.name} — ${hintOf(act)}`}
    onmousedown={onAlt ? undefined : dragDown}
    onmousemove={onAlt ? undefined : dragMove}
    onclick={onAlt ? undefined : () => onRow(act, a.slot)}
    role={act === "none" ? undefined : "button"}
  >
    <span class="badge" class:tonal class:gen={!tonal} class:th={!!thumb} aria-hidden="true">
      {#if thumb}
        <img src={thumb} alt="" draggable="false" />
      {:else if ext}
        <span class="mono">{ext}</span>
      {:else}
        <Icon name={img ? "image" : isCodeFile(a) ? "code" : "draft"} size={20} />
      {/if}
    </span>
    <span class="tx">
      {@render nameText(a.name, 2, emphasizeNames)}
      <span class="muted small tnum sz">
        {formatBytes(a.size)}{#if !a.available} · {msg.card.fileGone}{/if}
      </span>
    </span>
    {#if tonal}
      <span class="tr"><Icon name="chevron_right" size={20} /></span>
    {:else if opensExternally}
      <span class="tr"><Icon name="open_in_new" size={20} /></span>
    {/if}
  </svelte:element>
{/snippet}

<!-- Image alone: the thumbnail and its caption in a single frame. Feed: 180 px at most; detail: whole when alone, 200 px when several. -->
{#snippet frame(a: AttachmentView, index: number, act: ReturnType<typeof rowAction>, count: number)}
  <svelte:element
    this={act === "none" ? "div" : "button"}
    class="frame"
    class:openable={act !== "none"}
    type={act === "none" ? undefined : "button"}
    title={expanded ? hintOf(act) : `${a.name} — ${hintOf(act)}`}
    onmousedown={dragDown}
    onmousemove={dragMove}
    onclick={() => onRow(act, a.slot)}
    role={act === "none" ? undefined : "button"}
  >
    <img
      class="shot"
      class:feed={!expanded}
      class:whole={expanded && count === 1}
      class:multi={expanded && count > 1}
      src={a.thumbnail}
      alt=""
      draggable="false"
    />
    <span class="cap">
      {@render nameText(a.name, 1, emphasizeNames)}
      <span class="muted small tnum sz">
        {formatBytes(a.size)}{#if !a.available} · {msg.card.fileGone}{/if}
      </span>
      {#if act !== "none"}<span class="tr"><Icon name="chevron_right" size={20} /></span>{/if}
    </span>
  </svelte:element>
{/snippet}

<!-- Images only (2 or more): 2 square cells, or a large one (2/3) and two small ones; "+N" on the last, which opens the detail. -->
{#snippet mosaic(list: AttachmentView[], shownCount: number, more: number)}
  <div class="mosaic" class:two={shownCount === 2}>
    {#each list.slice(0, shownCount) as a, i (`${i}:${a.name}`)}
      {@const plus = i === shownCount - 1 && more > 0}
      {@const act = rowAction(a, false)}
      <svelte:element
        this={plus || act !== "none" ? "button" : "div"}
        class="cell"
        class:openable={plus || act !== "none"}
        type={plus || act !== "none" ? "button" : undefined}
        title={plus ? plural(msg.card.moreFiles, more) : `${a.name} — ${hintOf(act)}`}
        aria-label={plus ? plural(msg.card.moreFiles, more) : a.name}
        onmousedown={plus ? undefined : dragDown}
        onmousemove={plus ? undefined : dragMove}
        onclick={plus ? openMore : () => onRow(act, a.slot)}
        role={plus || act !== "none" ? "button" : undefined}
      >
        <img src={a.thumbnail} alt="" draggable="false" />
        {#if plus}<span class="plus" aria-hidden="true">+{more}</span>{/if}
      </svelte:element>
    {/each}
  </div>
{/snippet}

<!-- One file of the list: a frame (image with a thumbnail) or a tile. -->
{#snippet oneFile(list: AttachmentView[], e: { index: number; look: "frame" | "tile" }, secretShown: boolean)}
  {@const a = list[e.index]}
  {@const act = rowAction(a, secretShown)}
  {#if e.look === "frame"}
    {@render frame(a, e.index, act, list.length)}
  {:else}
    {@render tile(a, e.index, act, secretShown)}
  {/if}
{/snippet}

<!--
  `secretShown`: the files of a revealed hidden item: no thumbnail, no action (they open with "Ouvrir"),
  tiles on `surface` (the block is already `surface-alt`).
-->
{#snippet fileList(list: AttachmentView[], secretShown: boolean)}
  {@const plan = attachmentPlan(list, expanded, !secretShown)}
  {@const allImages = list.every(isImage)}
  {@const summary =
    list.length >= 2
      ? format(allImages ? msg.card.summaryImages : msg.card.summaryFiles, {
          n: list.length,
          size: formatBytes(list.reduce((n, a) => n + a.size, 0)),
        })
      : null}
  <div class="files">
    {#if plan.kind === "mosaic"}
      {@render mosaic(list, plan.shown, plan.more)}
      {#if summary}<div class="muted small">{summary}</div>{/if}
    {:else}
      {#if expanded && summary}<div class="group muted small">{summary}</div>{/if}
      {#each plan.entries as e (`${list[e.index].slot}:${list[e.index].name}`)}
        {#if rowMenus}
          <div class="frow" class:top={e.look === "frame"}>
            <div class="fgrow">{@render oneFile(list, e, secretShown)}</div>
            <Menu entries={rowEntries(list[e.index])} label={msg.card.fileActions} />
          </div>
        {:else}
          {@render oneFile(list, e, secretShown)}
        {/if}
      {/each}
      {#if plan.more > 0}
        {#if readMore}
          <div class="muted small">{plural(msg.card.moreFilesCaption, plan.more)}</div>
        {:else}
          <div class="more">
            <button class="ghost" type="button" onclick={openMore}>
              {plural(msg.card.moreFiles, plan.more)}
              <Icon name="chevron_right" size={20} />
            </button>
          </div>
        {/if}
      {/if}
    {/if}
  </div>
{/snippet}

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<article
  class="card"
  class:secret={item.secret}
  class:focused
  class:expanded
  data-item-id={item.id}
  tabindex="-1"
  bind:this={el}
  oncontextmenu={onContext}
  use:seenWatch={{
    wanted: () => readable,
    ready: () => feed.feedAttended,
    onseen: () => feed.markSeen(item.id),
  }}
>
  <header class="head">
    <span class="kind" class:secret-kind={item.secret} class:error-kind={item.unreadable}>
      <Icon name={kindIcon(item)} size={20} label={kindLabel} />
    </span>
    <span class="meta grow ellipsis">
      {item.outgoing ? msg.card.thisPc : format(msg.card.from, { device: item.sourceDevice.name })}
      · <span class="tnum">{formatTimestamp(item.createdAt)}</span>
    </span>
    {#if item.pinned}
      <span class="pin"><Icon name="push_pin_fill1" size={16} label={msg.card.pinned} /></span>
    {/if}
    {#if item.archived}
      <span class="archived"><Icon name="archive" size={16} />{msg.card.archived}</span>
    {/if}
    {#if showUnread}<span class="dot" role="img" aria-label={msg.card.new}></span>{/if}
    {#if showMenu && !gone}
      {#if showsCopyButton(item)}
        <button class="ghost icon" type="button" title={msg.card.copy} aria-label={msg.card.copy} onclick={copy}>
          <Icon name="content_copy" size={20} />
        </button>
      {/if}
      <Menu bind:this={menu} {entries} label={msg.card.more} />
    {/if}
  </header>

  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="content"
    class:tappable={!expanded && tap !== "none"}
    title={!expanded ? tapLabel : undefined}
    onclick={onContentClick}
  >
    {#if item.unreadable}
      <p class="muted">
        {item.unreadableReason === "updateRequired" ? msg.card.unreadableUpdate : msg.card.unreadable}
      </p>
    {:else if item.secret}
      <div class="secret-label">{msg.card.secret}</div>
      {#if !revealed}
        <div class="mask">
          <span class="dots mono" aria-hidden="true">{MASK}</span>
          <button class="ghost reveal" type="button" onclick={() => feed.reveal(item)}>
            <Icon name="visibility" size={20} />
            {msg.card.reveal}
          </button>
        </div>
        <!-- Name, size, thumbnail and type of a hidden file stay hidden: only the count, with a paperclip. -->
        {#if item.attachmentCount > 0}
          <div class="hidden-files muted small">
            <Icon name="attach_file" size={16} />
            <span>
              {item.attachmentCount === 1
                ? msg.card.fileHidden
                : format(msg.card.filesHidden, { n: item.attachmentCount })}
            </span>
          </div>
        {/if}
      {:else if shown}
        {#if shown.title}
          <div class="title" class:clamp={!expanded} use:cutWatch={(c) => (titleCut = c)} oncopy={blockCopy}>{shown.title}</div>
        {/if}
        <div class="revealed" oncopy={blockCopy}>
          {#if shown.text}
            <pre
              class="value mono password"
              class:clamp6={!expanded}
              use:cutWatch={(c) => (bodyCut = c)}>{shown.text}</pre>
          {/if}
          {#if shown.url}<div class="value mono">{shown.url}</div>{/if}
          {#if revealedFiles.length > 0}
            {@render fileList(revealedFiles, true)}
          {/if}
          <div class="reveal-actions row">
            {#if shown.text}
              <span class="muted small">{plural(msg.card.characters, shown.text.length)}</span>
            {/if}
            <span class="grow"></span>
            <!-- In the detail, "Ouvrir" is in the toolbar of the screen. -->
            {#if hasFiles && !gone && !expanded && item.attachmentCount === 1}
              <button class="ghost" type="button" onclick={openIt}>
                <Icon name="open_in_new" size={20} />{msg.card.open}
              </button>
            {/if}
            <button class="ghost" type="button" onclick={() => feed.hideValues(item.id)}>
              <Icon name="visibility_off" size={20} />{msg.card.hide}
            </button>
          </div>
        </div>
        <div class="muted small">{msg.card.revealedHint}</div>
      {/if}
    {:else}
      {#if item.title}
        <div
          class="title selectable"
          class:strong={showUnread}
          class:clamp={!expanded}
          use:cutWatch={(c) => (titleCut = c)}>{item.title}</div>
      {/if}
      {#if item.preview !== null}
        <RichText {item} {expanded} oncut={(c) => (bodyCut = c)} />
      {/if}
      {#if item.url}
        <div class="url mono selectable" class:ellipsis={!expanded}>{item.url}</div>
      {/if}
      {#if item.attachments.length > 0}
        {@render fileList(item.attachments, false)}
      {/if}
    {/if}
  </div>

  {#if readMore}
    <div class="more">
      <button class="ghost" type="button" onclick={openMore}>
        {msg.card.readMore}
        <Icon name="chevron_right" size={20} />
      </button>
    </div>
  {/if}

  {#if footer}
    <footer class="foot small" class:tone-secondary={footer.tone === "secondary"} class:tone-error={footer.tone === "error"} class:tone-success={footer.tone === "success"}>
      {#if footer.icon}<Icon name={footer.icon} size={16} />{/if}
      <span>{footer.text}</span>
    </footer>
  {/if}
</article>

{#if confirmDelete}
  <Dialog
    title={msg.detail.deleteTitle}
    message={item.secret ? msg.detail.deleteSecretMessage : msg.detail.deleteMessage}
    secondaryLabel={msg.card.deleteLocal}
    onsecondary={removeLocal}
    confirmLabel={msg.card.deleteEverywhere}
    onconfirm={removeEverywhere}
    oncancel={() => (confirmDelete = false)}
  />
{/if}

{#if confirmLocalFileSlot !== null}
  <Dialog
    title={msg.card.deleteLocalTitle}
    message={msg.card.deleteLocalMessage}
    confirmLabel={msg.card.fileDeleteLocal}
    onconfirm={confirmLocalFile}
    oncancel={() => (confirmLocalFileSlot = null)}
  />
{/if}

{#if confirmFileSlot !== null}
  <Dialog
    title={msg.card.fileDeleteTitle}
    message={item.secret ? msg.card.fileDeleteSecretMessage : format(msg.card.fileDeleteMessage, { name: confirmFileName })}
    confirmLabel={msg.card.fileDeleteEverywhere}
    onconfirm={confirmFile}
    oncancel={() => (confirmFileSlot = null)}
  />
{/if}

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-2) var(--space-3) var(--space-4);
    background: var(--surface);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-md);
    min-width: 0;
  }
  /*
   * Hidden items have a plum edge on the left (border.width.secret-accent), DRAWN behind the
   * content (a background clipped by the rounded corners): it takes no room, so the text lines
   * up with the one of a normal card.
   */
  .card.secret {
    border-left-color: transparent;
    background-color: var(--surface);
    background-image: linear-gradient(var(--secret), var(--secret));
    background-origin: border-box;
    background-repeat: no-repeat;
    background-size: var(--secret-accent-width) 100%;
  }
  .card.focused {
    outline: var(--focus-width) solid var(--brand);
    outline-offset: 1px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 36px;
  }
  .kind {
    display: inline-flex;
    color: var(--muted);
  }
  .kind.secret-kind {
    color: var(--secret);
  }
  .kind.error-kind {
    color: var(--error);
  }
  .meta {
    color: var(--muted);
    font-size: var(--fs-caption);
    line-height: var(--lh-caption);
  }
  .pin {
    display: inline-flex;
    color: var(--muted);
  }
  /* Badge "Archivé": a search under "Tous" also finds archived items. */
  .archived {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    color: var(--muted);
    font-size: var(--fs-caption);
    line-height: var(--lh-caption);
    white-space: nowrap;
  }
  /* Non-lu: a filled dot in brand (FILL 1 is reserved for the selected state and this dot). */
  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: var(--radius-full);
    background: var(--brand);
  }
  .content {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding-right: var(--space-2);
    min-width: 0;
  }
  .content.tappable {
    cursor: pointer;
  }
  .content p {
    margin: 0;
  }
  .title {
    font-weight: 500;
    overflow-wrap: anywhere;
  }
  .title.strong {
    font-weight: 600;
  }
  .clamp {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
  }
  .url {
    color: var(--brand);
    overflow-wrap: anywhere;
  }
  .secret-label {
    color: var(--secret);
    font-size: var(--fs-label);
    line-height: var(--lh-label);
    font-weight: 500;
  }
  .mask {
    display: flex;
    align-items: center;
    background: var(--secret-container);
    border-radius: var(--radius-md);
    padding-left: var(--space-4);
    min-height: 48px;
  }
  .dots {
    flex: 1;
    font-size: var(--fs-subtitle);
    letter-spacing: 0.1em;
  }
  .reveal {
    min-height: 48px;
    color: var(--text);
  }
  .revealed {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    background: var(--surface-alt);
    /* Explicit text colour: the revealed value stays very readable. */
    color: var(--text);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
  }
  .value {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    /* Not selectable on purpose (see blockCopy): copying goes through "Copier". */
    user-select: none;
    -webkit-user-select: none;
  }
  .clamp6 {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 6;
    line-clamp: 6;
    overflow: hidden;
  }
  .reveal-actions {
    flex-wrap: wrap;
    gap: var(--space-1);
  }
  .reveal-actions button {
    min-height: 32px;
    color: var(--muted);
  }
  /* ---- attachments ---- */
  .files {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  /* A file with its "⋮" next to it (the detail of a batch): the menu is a sibling, a button never holds a button. */
  .frow {
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }
  .frow.top {
    align-items: flex-start;
  }
  .fgrow {
    flex: 1;
    min-width: 0;
  }
  .group {
    padding-top: var(--space-1);
    font-weight: 500;
  }
  .hidden-files {
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }
  /* Tile: 56 px at least, surface-alt (surface inside the block of a revealed hidden item), radius 8, padding 8. */
  .tile,
  .frame {
    display: flex;
    justify-content: flex-start;
    width: 100%;
    min-height: 0;
    padding: 0;
    text-align: left;
    white-space: normal;
    font-weight: 400;
    color: var(--text);
    background-color: var(--surface-alt);
    border: 0;
    border-radius: var(--radius-md);
  }
  .tile {
    flex-direction: row;
    align-items: center;
    gap: var(--space-3);
    min-height: 56px;
    padding: var(--space-2);
  }
  .tile.alt {
    background-color: var(--surface);
  }
  .frame {
    flex-direction: column;
    align-items: stretch;
    overflow: hidden;
  }
  /* Only what does something reacts to the pointer (the global button:hover would repaint the background). */
  .tile.openable,
  .frame.openable {
    cursor: pointer;
  }
  .tile.openable:hover,
  .frame.openable:hover {
    background-color: var(--surface-alt);
    background-image: linear-gradient(var(--hover), var(--hover));
  }
  .tile.alt.openable:hover {
    background-color: var(--surface);
  }
  .tile.openable:active,
  .frame.openable:active {
    background-color: var(--surface-alt);
    background-image: linear-gradient(var(--pressed), var(--pressed));
  }
  /* Type badge 40 x 40, radius 4: tonal (brand) when the file opens in Tomi, neutral otherwise. */
  .badge {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 40px;
    overflow: hidden;
    border-radius: var(--radius-sm);
    font-size: var(--fs-caption);
    line-height: var(--lh-caption);
    font-weight: 600;
    letter-spacing: 0.04em;
  }
  .badge .mono {
    font-size: var(--fs-caption);
    line-height: var(--lh-caption);
  }
  .badge.gen {
    background: var(--surface);
    color: var(--text);
    box-shadow: inset 0 0 0 var(--border-width) var(--border);
  }
  .tile.alt .badge.gen {
    background: var(--surface-alt);
  }
  .badge.tonal {
    background: var(--brand-container);
    color: var(--brand);
  }
  .badge.th img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .tx {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  /* Name: Inter 14/20 medium (semi-bold when unread without title). Feed: 2 lines (1 under an image), cut before the extension. */
  .nm {
    display: block;
    min-width: 0;
    font-size: var(--fs-label);
    line-height: var(--lh-label);
    font-weight: 500;
    overflow-wrap: anywhere;
  }
  .nm.strong {
    font-weight: 600;
  }
  .cut {
    display: block;
    max-height: calc(var(--lh-label) * var(--lines));
    overflow: hidden;
    overflow-wrap: anywhere;
  }
  .sz {
    white-space: nowrap;
  }
  .tr {
    flex: none;
    display: inline-flex;
    margin-right: var(--space-1);
    color: var(--muted);
  }
  /* Image: the thumbnail and its caption in one frame. */
  .shot {
    display: block;
    width: 100%;
    object-fit: cover;
    background: var(--surface-alt);
  }
  .shot.feed {
    max-height: 180px;
  }
  .shot.multi {
    max-height: 200px;
  }
  .shot.whole {
    height: auto;
    object-fit: contain;
  }
  .cap {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 40px;
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-3);
  }
  .cap .nm {
    flex: 1;
  }
  /* Mosaic: a large cell on the left (2/3) and two small ones; two images: two square cells. "+N" on a dark veil. */
  .mosaic {
    display: grid;
    grid-template-columns: 2fr 1fr;
    grid-template-rows: 1fr 1fr;
    gap: 4px;
    aspect-ratio: 5 / 3;
    max-height: 240px;
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .mosaic.two {
    grid-template-columns: 1fr 1fr;
    grid-template-rows: 1fr;
    aspect-ratio: 2.2 / 1;
  }
  .mosaic:not(.two) .cell:first-child {
    grid-row: 1 / span 2;
  }
  .cell {
    position: relative;
    display: block;
    min-width: 0;
    min-height: 0;
    padding: 0;
    border: 0;
    border-radius: 0;
    background: var(--surface-alt);
    overflow: hidden;
  }
  .cell img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .cell.openable {
    cursor: pointer;
  }
  .cell.openable:hover img {
    opacity: 0.88;
  }
  .plus {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    /* viewer-chip of the charter (black at 55 %), as the image viewer. */
    background: rgba(0, 0, 0, 0.55);
    color: #ffffff;
    font-size: var(--fs-title);
    line-height: var(--lh-title);
    font-weight: 600;
  }
  .more {
    display: flex;
    justify-content: flex-end;
  }
  .more button {
    color: var(--brand);
    padding-right: var(--space-1);
  }
  .foot {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    color: var(--muted);
  }
  .foot.tone-secondary {
    color: var(--secondary);
  }
  .foot.tone-error {
    color: var(--error);
  }
  .foot.tone-success {
    color: var(--success);
  }
</style>
