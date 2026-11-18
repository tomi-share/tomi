<script lang="ts">
  import { api, asAppError } from "./api";
  import { feed } from "./feed.svelte";
  import { errorMessage, format, fr } from "./i18n/fr";
  import Icon from "./Icon.svelte";
  import { isImage } from "./itemLogic";

  /**
   * The internal image viewer (Android's ImageViewerScreen): full window, zoom with the wheel
   * or the buttons, drag to move, arrows to go through the images of the item. The bytes come
   * from Rust as a `data:` URL (never for a hidden item, 16 Mo at most); the orientation of a photo
   * (EXIF) is applied by the webview. The image is always shown on a dark ground.
   */

  const MIN = 1;
  const MAX = 8;

  interface Entry {
    index: number;
    name: string;
  }

  const target = $derived(feed.viewer);
  let entries = $state<Entry[]>([]);
  let position = $state(0);
  let src = $state<string | null>(null);
  let name = $state("");
  let status = $state<"loading" | "ready" | "error">("loading");
  let errorText = $state("");
  let scale = $state(1);
  let tx = $state(0);
  let ty = $state(0);
  let root: HTMLElement | undefined = $state();
  let stage: HTMLElement | undefined = $state();
  let drag: { x: number; y: number; tx: number; ty: number } | null = null;

  async function open(itemId: string, attachmentIndex: number) {
    status = "loading";
    try {
      const view = await api.itemView(itemId);
      entries = view.attachments
        .map((a, index) => ({ a, index }))
        .filter(({ a }) => isImage(a) && a.available)
        .map(({ a, index }) => ({ index, name: a.name }));
      if (entries.length === 0) throw { code: "error.nothing_to_open", params: {} };
      position = Math.max(0, entries.findIndex((e) => e.index === attachmentIndex));
      await load(itemId);
    } catch (e) {
      fail(e);
    }
  }

  async function load(itemId: string) {
    status = "loading";
    src = null;
    reset();
    const entry = entries[position];
    name = entry.name;
    try {
      const img = await api.readImage(itemId, entry.index);
      src = img.dataUrl;
      status = "ready";
    } catch (e) {
      fail(e);
    }
  }

  function fail(e: unknown) {
    const err = asAppError(e);
    errorText = err.code === "error.image_too_large" ? fr.viewer.tooLarge : errorMessage(err);
    status = "error";
  }

  $effect(() => {
    const t = target;
    if (!t) return;
    void open(t.itemId, t.index);
  });

  async function go(delta: number) {
    if (!target || entries.length < 2) return;
    position = (position + delta + entries.length) % entries.length;
    await load(target.itemId);
  }

  function reset() {
    scale = 1;
    tx = 0;
    ty = 0;
  }

  function zoom(factor: number) {
    const next = Math.min(MAX, Math.max(MIN, scale * factor));
    scale = next;
    if (next === MIN) {
      tx = 0;
      ty = 0;
    }
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    zoom(Math.exp(-e.deltaY / 400));
  }

  function onPointerDown(e: PointerEvent) {
    if (scale <= MIN) return;
    drag = { x: e.clientX, y: e.clientY, tx, ty };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onPointerMove(e: PointerEvent) {
    if (!drag) return;
    tx = drag.tx + (e.clientX - drag.x);
    ty = drag.ty + (e.clientY - drag.y);
  }
  function onPointerUp() {
    drag = null;
  }

  function onDouble() {
    if (scale > MIN) reset();
    else scale = 2.5;
  }

  function close() {
    feed.viewer = null;
  }

  async function save() {
    if (!target) return;
    try {
      const r = await api.saveItem(target.itemId);
      feed.say(format(fr.card.saved, { dir: r.dir }));
    } catch (e) {
      feed.fail(e);
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      void go(1);
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      void go(-1);
    } else if (e.key === "+" || e.key === "=") {
      zoom(1.25);
    } else if (e.key === "-") {
      zoom(1 / 1.25);
    } else if (e.key === "0") {
      reset();
    }
  }

  $effect(() => {
    root?.focus();
  });

  // The wheel must not scroll anything behind the viewer: listen non-passively.
  $effect(() => {
    const node = stage;
    if (!node) return;
    node.addEventListener("wheel", onWheel, { passive: false });
    return () => node.removeEventListener("wheel", onWheel);
  });
</script>

<div
  bind:this={root}
  class="viewer"
  role="dialog"
  aria-modal="true"
  aria-label={fr.card.viewImage}
  tabindex="-1"
  onkeydown={onKey}
>
  <header class="bar">
    <button class="ghost icon" type="button" aria-label={fr.viewer.close} onclick={close}>
      <Icon name="close" size={24} />
    </button>
    <span class="mono ellipsis grow name">{name}</span>
    {#if entries.length > 1}
      <button class="ghost icon" type="button" aria-label={fr.viewer.previous} onclick={() => go(-1)}>
        <span class="flip"><Icon name="chevron_right" size={24} /></span>
      </button>
      <span class="tnum small" aria-live="polite">{format(fr.viewer.position, { n: position + 1, total: entries.length })}</span>
      <button class="ghost icon" type="button" aria-label={fr.viewer.next} onclick={() => go(1)}>
        <Icon name="chevron_right" size={24} />
      </button>
    {/if}
    <button class="ghost icon" type="button" aria-label={fr.viewer.zoomOut} disabled={status !== "ready" || scale <= MIN} onclick={() => zoom(1 / 1.25)}>
      <span aria-hidden="true">−</span>
    </button>
    <button class="ghost" type="button" disabled={status !== "ready"} title={fr.viewer.fit} onclick={reset}>
      <span class="tnum">{Math.round(scale * 100)} %</span>
    </button>
    <button class="ghost icon" type="button" aria-label={fr.viewer.zoomIn} disabled={status !== "ready" || scale >= MAX} onclick={() => zoom(1.25)}>
      <span aria-hidden="true">+</span>
    </button>
    <button class="ghost icon" type="button" title={fr.card.save} aria-label={fr.card.save} onclick={save}>
      <Icon name="download" size={24} />
    </button>
  </header>

  <div
    bind:this={stage}
    class="stage"
    class:zoomed={scale > MIN}
    role="presentation"
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerUp}
    ondblclick={onDouble}
  >
    {#if status === "ready" && src}
      <img
        {src}
        alt={format(fr.viewer.description, { n: position + 1, total: entries.length })}
        draggable="false"
        style:transform="translate({tx}px, {ty}px) scale({scale})"
      />
    {:else if status === "loading"}
      <p class="state">{fr.viewer.loading}</p>
    {:else}
      <p class="state" role="alert">{errorText}</p>
    {/if}
  </div>

  <footer class="hint small">{fr.viewer.hint}</footer>
</div>

<style>
  /* The ground of the viewer is dark in both themes (the dark tokens of the charter). */
  .viewer {
    --text: #e9e7e1;
    --muted: #a6abb8;
    --surface: #1a1f2b;
    --surface-alt: #242a38;
    --border: #383e4c;
    --border-strong: #6f7587;
    --brand: #a9bfea;
    --hover: color-mix(in srgb, #e9e7e1 10%, transparent);
    --pressed: color-mix(in srgb, #e9e7e1 18%, transparent);
    color-scheme: dark;
    position: fixed;
    inset: 0;
    z-index: 25;
    display: flex;
    flex-direction: column;
    background: color-mix(in srgb, #121620 96%, black);
    color: var(--text);
    outline: none;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-2) var(--space-3);
  }
  .bar button {
    color: var(--text);
  }
  .name {
    padding: 0 var(--space-2);
  }
  .flip {
    display: inline-flex;
    transform: scaleX(-1);
  }
  /* The image is positioned against the stage, whose size is definite: in a grid with an
     auto row, `max-height: 100%` resolved against the image itself and a tall photo was cut. */
  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
    display: grid;
    place-items: center;
    overflow: hidden;
    touch-action: none;
  }
  .stage.zoomed {
    cursor: grab;
  }
  .stage.zoomed:active {
    cursor: grabbing;
  }
  img {
    position: absolute;
    inset: 0;
    margin: auto;
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    user-select: none;
    transform-origin: center;
  }
  .state {
    margin: 0;
    padding: var(--space-6);
    text-align: center;
    max-width: 420px;
  }
  .hint {
    text-align: center;
    color: var(--muted);
    padding: var(--space-2);
  }
</style>
