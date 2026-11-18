<script lang="ts">
  import { onMount } from "svelte";
  import CodeViewer from "./CodeViewer.svelte";
  import DepartureDialogs from "./DepartureDialogs.svelte";
  import { departure } from "./departure.svelte";
  import { feed } from "./feed.svelte";
  import ImageViewer from "./ImageViewer.svelte";
  import ItemDetail from "./ItemDetail.svelte";

  // Both windows mount this: it also keeps the departure (banner and dialogs) up to date.
  onMount(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    void departure.start().then((s) => {
      if (gone) s();
      else stop = s;
    });
    return () => {
      gone = true;
      stop?.();
    };
  });

  // Full-window screens, opened from the cards: the detail, the image viewer, the code screen.
  // A viewer opens over the detail when it comes from there.

  // Escape when the focus is nowhere in particular (a screen closed under it, a click on the
  // background): closes the top screen. A screen, a menu or a dialog that has the focus handles
  // Escape itself first.
  function onKey(e: KeyboardEvent) {
    if (e.defaultPrevented) return;
    // Ctrl+Z presses "Annuler" of the banner ("Archivé · Annuler") while it is shown, anywhere
    // except in a text field (where it undoes the typing) or under a dialog.
    if (e.ctrlKey && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "z") {
      const inField = (e.target as HTMLElement | null)?.closest("input, textarea, select, [contenteditable]");
      const action = feed.flash?.kind === "error" ? undefined : feed.flash?.action;
      if (inField || feed.overlays > 0 || !action) return;
      e.preventDefault();
      feed.clearFlash();
      action.run();
      return;
    }
    if (e.key !== "Escape") return;
    const target = e.target as HTMLElement | null;
    if (target?.closest('[role="menu"], [role="alertdialog"]')) return;
    if (feed.overlays > 0) return;
    if (feed.viewer) feed.viewer = null;
    else if (feed.code) feed.code = null;
    else if (feed.detailId !== null) feed.closeDetail();
    else return;
    e.preventDefault();
  }
</script>

<svelte:window onkeydown={onKey} />

{#if feed.detailId !== null}<ItemDetail />{/if}
{#if feed.viewer}<ImageViewer />{/if}
{#if feed.code}<CodeViewer />{/if}
<DepartureDialogs />
