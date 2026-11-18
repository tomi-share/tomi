<script lang="ts">
  import { api, type RenderedItem, type ItemView } from "./api";
  import { feed } from "./feed.svelte";
  import { msg } from "./i18n";
  import { theme } from "./theme.svelte";

  /**
   * The text of an item that is not hidden, in two modes like Android: "Aperçu" (styled) and
   * "Markdown" (the literal source, only offered when the item has a Markdown representation).
   * HTML is never shown as source, and it is sanitised in Rust before it gets here (it is shown
   * in a sandboxed iframe with no script and no network).
   *
   * `oncut` tells the card whether the body is cut by the card limits, so that it can offer
   * "Lire la suite".
   */
  let {
    item,
    expanded = false,
    oncut,
  }: { item: ItemView; expanded?: boolean; oncut?: (cut: boolean) => void } = $props();

  /** Lines the card shows of a plain text or of the Markdown source before cutting. */
  const PLAIN_LINES = 4;
  const SOURCE_LINES = 8;
  const CARD_DOC_HEIGHT = 220;
  const DETAIL_DOC_HEIGHT = 520;

  let rendered = $state<RenderedItem | null>(null);
  let box: HTMLElement | undefined = $state();
  let docReady = $state(false);
  // The frame is destroyed when the text goes back to the source: the next one starts hidden again.
  $effect(() => {
    if (rendered?.rendered.kind !== "document") docReady = false;
  });

  /** A card of a plain text needs no round trip: the preview is the text. */
  const needsRender = $derived(expanded || item.hasMarkdown || item.rich);
  const modes = $derived(rendered?.modes ?? []);

  // Reads the rendering again when the item, the chosen mode or the theme (document colours) changes.
  // Memoised on the id: a refresh of the feed hands over a new object for the same item, and
  // the document must not be built again (nor flash) for that.
  const itemId = $derived(item.id);

  $effect(() => {
    const id = itemId;
    const mode = feed.preferredMode ?? undefined;
    const dark = theme.dark;
    if (!needsRender) {
      rendered = null;
      return;
    }
    let stale = false;
    api
      .renderItem(id, mode, dark)
      .then((r) => {
        if (!stale) rendered = r;
      })
      .catch((e) => feed.fail(e));
    return () => {
      stale = true;
    };
  });

  /** A rough height for the sandboxed document: its content cannot be measured from outside. */
  const docHeight = $derived.by(() => {
    const cap = expanded ? DETAIL_DOC_HEIGHT : CARD_DOC_HEIGHT;
    const text = item.preview ?? "";
    const lines = text.split("\n").reduce((n, l) => n + Math.max(1, Math.ceil(l.length / 60)), 0);
    return Math.min(cap, Math.max(64, lines * 30 + 24));
  });
  const docCut = $derived(!expanded && (item.previewTruncated || docHeight >= CARD_DOC_HEIGHT));

  // Plain text and Markdown source are clamped by CSS: the DOM says whether something was cut.
  function measure() {
    if (!oncut) return;
    if (rendered?.rendered.kind === "document") return oncut(docCut);
    if (expanded) return oncut(false);
    oncut(!!box && (box.scrollHeight > box.clientHeight + 1 || item.previewTruncated));
  }

  $effect(() => {
    // Re-measured when the text, the mode or the width changes.
    void rendered;
    void item.preview;
    measure();
    if (!box || expanded) return;
    const ro = new ResizeObserver(measure);
    ro.observe(box);
    return () => ro.disconnect();
  });

  const shownMode = $derived(rendered?.mode ?? "preview");
</script>

<div class="rich">
  {#if modes.length > 1}
    <div class="modes" role="group" aria-label={msg.renderModesLabel}>
      {#each modes as m (m)}
        <button
          class="seg"
          type="button"
          aria-pressed={shownMode === m}
          onclick={(e) => {
            e.stopPropagation();
            void feed.chooseMode(m);
          }}>{msg.renderModes[m]}</button
        >
      {/each}
    </div>
  {/if}

  {#if rendered?.rendered.kind === "document"}
    <!-- Shown once loaded (`ready`): no flash of the initial blank document while the srcdoc loads. -->
    <iframe
      class="doc"
      class:ready={docReady}
      onload={() => (docReady = true)}
      sandbox=""
      title={msg.card.details}
      srcdoc={rendered.rendered.srcdoc}
      style:height="{docHeight}px"
    ></iframe>
  {:else if rendered?.rendered.kind === "plain" && shownMode === "source"}
    <pre
      bind:this={box}
      class="source mono selectable"
      class:clamped={!expanded}
      style:--lines={SOURCE_LINES}>{rendered.rendered.text}</pre>
  {:else}
    <div
      bind:this={box}
      class="plain selectable"
      class:clamped={!expanded}
      style:--lines={PLAIN_LINES}>{rendered?.rendered.kind === "plain"
        ? rendered.rendered.text
        : (item.preview ?? "")}</div>
  {/if}
</div>

<style>
  .rich {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }
  /* Segmented control: two joined buttons. */
  .modes {
    display: inline-flex;
    align-self: stretch;
  }
  .seg {
    flex: 1;
    border-radius: 0;
    min-height: 32px;
    font-weight: 500;
  }
  .seg:first-child {
    border-radius: var(--radius-full) 0 0 var(--radius-full);
  }
  .seg:last-child {
    border-radius: 0 var(--radius-full) var(--radius-full) 0;
    margin-left: -1px;
  }
  .seg[aria-pressed="true"] {
    background: var(--brand-container);
    color: var(--brand);
    border-color: var(--brand);
    z-index: 1;
  }
  .plain,
  .source {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: var(--fs-body);
    line-height: var(--lh-body);
  }
  .source {
    font-size: 13px;
    line-height: 19px;
    background: var(--surface-alt);
    border-radius: var(--radius-sm);
    padding: var(--space-2) var(--space-3);
  }
  .clamped {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: var(--lines);
    line-clamp: var(--lines);
    overflow: hidden;
  }
  .doc {
    width: 100%;
    border: 0;
    background: transparent;
    /* Same scheme as the page (inherited, so `dark` in dark mode) and as the document inside
       (`wrap_document`): when the two differ, Chromium paints the frame with an opaque canvas,
       and a fresh frame first shows its white about:blank. */
    color-scheme: inherit;
    opacity: 0;
  }
  .doc.ready {
    opacity: 1;
  }
</style>
