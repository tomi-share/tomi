// Small Svelte actions shared by the cards and the viewers.

import { fitFileName } from "./itemLogic";

/**
 * Tells whether the content of a node is cut by its CSS limits (a line clamp or a max height):
 * the card then offers "Lire la suite". Measured again when the node is resized.
 */
export function cutWatch(node: HTMLElement, report: (cut: boolean) => void) {
  const check = () => report(node.scrollHeight > node.clientHeight + 1);
  const observer = new ResizeObserver(check);
  observer.observe(node);
  check();
  return {
    destroy() {
      observer.disconnect();
      report(false);
    },
  };
}

/**
 * Cell of a masonry grid (`grid-auto-rows: 1px` + `grid-auto-flow: row dense`): the cell spans as many
 * 1 px rows as its own height, so a short card lets the next one come up under it instead of waiting
 * for the tallest card of its row. The cell must size to its content (`align-self: start`), and any
 * gap below it is its own bottom padding (the border box is measured). Measured again on every
 * resize (a card that expands, an image that loads).
 */
export function masonryCell(node: HTMLElement) {
  const apply = (height: number) => {
    node.style.gridRowEnd = `span ${Math.max(1, Math.ceil(height))}`;
  };
  const observer = new ResizeObserver((entries) => {
    const box = entries[entries.length - 1].borderBoxSize?.[0];
    apply(box ? box.blockSize : node.offsetHeight);
  });
  observer.observe(node);
  apply(node.offsetHeight);
  return {
    destroy() {
      observer.disconnect();
    },
  };
}

/** Share of a card that must be on screen to count as seen (or of the window, for a tall card). */
const SEEN_SHARE = 0.6;
/** How long a card must stay on screen before it counts as read: a quick scroll past reads nothing. */
export const SEEN_DWELL_MS = 1_500;

/**
 * Reads a card by looking at it: `onseen` is called once the card has stayed on screen (at least
 * 60 % of it, or of the window when it is taller) for `SEEN_DWELL_MS` while `ready()` holds (the
 * window is visible, focused and nothing covers it). While the window is not attended the card is
 * looked at again later, so that it is read as soon as the user comes back. `wanted()` says
 * whether the card still needs reading.
 */
export function seenWatch(
  node: HTMLElement,
  opts: { wanted: () => boolean; ready: () => boolean; onseen: () => void },
) {
  let visible = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const stop = () => {
    clearTimeout(timer);
    timer = undefined;
  };
  const arm = () => {
    if (timer !== undefined || !visible) return;
    timer = setTimeout(() => {
      timer = undefined;
      if (!visible || !opts.wanted()) return;
      if (opts.ready()) opts.onseen();
      else arm();
    }, SEEN_DWELL_MS);
  };
  const observer = new IntersectionObserver(
    (entries) => {
      const e = entries[entries.length - 1];
      const view = e.rootBounds?.height ?? window.innerHeight;
      visible =
        e.isIntersecting &&
        (e.intersectionRatio >= SEEN_SHARE || e.intersectionRect.height >= view * SEEN_SHARE);
      if (visible) arm();
      else stop();
    },
    { threshold: [0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1] },
  );
  observer.observe(node);
  return {
    destroy() {
      stop();
      observer.disconnect();
    },
  };
}

/** Moves the focus into a node when it appears (a viewer or a screen that opens on top). */
export function focusOnMount(node: HTMLElement) {
  node.focus();
}

/**
 * Writes a file name into a node, cut BEFORE its extension so that it fits the node's CSS height
 * limit (`max-height` of N lines, `overflow: hidden`): "Courrier banque fonds de rési….pdf". The
 * name is measured where it is drawn (`itemLogic.fitFileName` searches, the DOM answers), again
 * when the width of the node changes or the fonts arrive.
 */
export function fitName(node: HTMLElement, opts: { name: string }) {
  let name = opts.name;
  let width = -1;
  const fits = (text: string) => {
    node.textContent = text;
    return node.scrollHeight <= node.clientHeight + 1;
  };
  const run = () => {
    node.textContent = fitFileName(name, fits);
  };
  // Deferred to the next frame: rewriting the text inside the observer callback would raise
  // "ResizeObserver loop completed with undelivered notifications".
  let frame = 0;
  const observer = new ResizeObserver(() => {
    // Only a change of width: writing the text changes the height, never the width.
    if (node.clientWidth === width) return;
    width = node.clientWidth;
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(run);
  });
  observer.observe(node);
  run();
  void document.fonts?.ready.then(run);
  return {
    update(next: { name: string }) {
      if (next.name === name) return;
      name = next.name;
      run();
    },
    destroy() {
      cancelAnimationFrame(frame);
      observer.disconnect();
    },
  };
}
