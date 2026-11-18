<script lang="ts" module>
  import type { IconName } from "./icons";

  export interface MenuEntry {
    label: string;
    icon?: IconName;
    onselect: () => void;
    /** Red entry (deletion). */
    danger?: boolean;
    /** A separator is drawn above this entry. */
    separator?: boolean;
  }
</script>

<script lang="ts">
  import { tick } from "svelte";
  import Icon from "./Icon.svelte";

  /**
   * A menu opened by a button (the "⋮" of a card) or at a point (right click on a card, the
   * desktop counterpart of Android's long press). Keyboard: arrows move, Home / End jump,
   * Escape closes and gives the focus back, Tab leaves. It is `position: fixed` so that a
   * scrolling list never cuts it.
   */
  let {
    entries,
    label,
    icon = "more_vert",
    trigger = true,
  }: { entries: MenuEntry[]; label: string; icon?: IconName; trigger?: boolean } = $props();

  let open = $state(false);
  let button: HTMLButtonElement | undefined = $state();
  let panel: HTMLDivElement | undefined = $state();
  let pos = $state({ x: 0, y: 0 });
  /** Where the menu was asked for: the button, or a point. */
  let anchor: { x: number; y: number; right: boolean } = { x: 0, y: 0, right: true };
  let restoreFocus: HTMLElement | null = null;

  async function show(x: number, y: number, right: boolean) {
    anchor = { x, y, right };
    restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    open = true;
    pos = { x, y };
    await tick();
    place();
    items()[0]?.focus();
  }

  /** Keeps the panel inside the window: flips left / up when it would run out. */
  function place() {
    if (!panel) return;
    const r = panel.getBoundingClientRect();
    const margin = 8;
    let x = anchor.right ? anchor.x - r.width : anchor.x;
    let y = anchor.y;
    if (x + r.width > window.innerWidth - margin) x = window.innerWidth - margin - r.width;
    if (x < margin) x = margin;
    if (y + r.height > window.innerHeight - margin) y = Math.max(margin, anchor.y - r.height - 40);
    pos = { x, y };
  }

  export function openAt(x: number, y: number): void {
    void show(x, y, false);
  }

  function toggle() {
    if (open) return close();
    const r = button?.getBoundingClientRect();
    if (r) void show(r.right, r.bottom + 4, true);
  }

  function close(refocus = true) {
    if (!open) return;
    open = false;
    if (refocus) restoreFocus?.focus();
  }

  function items(): HTMLButtonElement[] {
    return panel ? Array.from(panel.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')) : [];
  }

  function onKey(e: KeyboardEvent) {
    const list = items();
    const i = list.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      list[(i + 1) % list.length]?.focus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      list[(i - 1 + list.length) % list.length]?.focus();
    } else if (e.key === "Home") {
      e.preventDefault();
      list[0]?.focus();
    } else if (e.key === "End") {
      e.preventDefault();
      list[list.length - 1]?.focus();
    } else if (e.key === "Tab") {
      close(false);
    }
  }

  function choose(entry: MenuEntry) {
    close();
    entry.onselect();
  }

  $effect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (panel && !panel.contains(e.target as Node) && !button?.contains(e.target as Node)) close(false);
    };
    const dismiss = () => close(false);
    window.addEventListener("pointerdown", away, true);
    window.addEventListener("blur", dismiss);
    window.addEventListener("resize", dismiss);
    return () => {
      window.removeEventListener("pointerdown", away, true);
      window.removeEventListener("blur", dismiss);
      window.removeEventListener("resize", dismiss);
    };
  });
</script>

{#if trigger}
  <button
    bind:this={button}
    class="ghost icon"
    type="button"
    aria-label={label}
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={toggle}
  >
    <Icon name={icon} size={20} />
  </button>
{/if}

{#if open}
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div
    bind:this={panel}
    class="menu"
    role="menu"
    aria-label={label}
    style:left="{pos.x}px"
    style:top="{pos.y}px"
    onkeydown={onKey}
  >
    {#each entries as entry (entry.label)}
      {#if entry.separator}<div class="sep" role="separator"></div>{/if}
      <button
        class="ghost item"
        class:danger={entry.danger}
        role="menuitem"
        type="button"
        tabindex="-1"
        onclick={() => choose(entry)}
      >
        {#if entry.icon}<Icon name={entry.icon} size={20} />{:else}<span class="blank"></span>{/if}
        <span class="grow">{entry.label}</span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .menu {
    position: fixed;
    z-index: 30;
    min-width: 200px;
    max-width: 280px;
    padding: var(--space-1) 0;
    background: var(--surface);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: 0 4px 16px color-mix(in srgb, var(--text) 18%, transparent);
  }
  .item {
    width: 100%;
    justify-content: flex-start;
    border-radius: 0;
    min-height: 40px;
    padding: 0 var(--space-3);
    font-weight: 400;
    text-align: left;
  }
  .item:focus-visible {
    outline-offset: -2px;
    background: var(--hover);
  }
  .blank {
    width: 20px;
    flex: none;
  }
  .sep {
    height: 1px;
    margin: var(--space-1) 0;
    background: var(--border);
  }
</style>
