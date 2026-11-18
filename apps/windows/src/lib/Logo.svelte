<script lang="ts">
  import { onMount } from "svelte";

  /**
   * The Tomi symbol, "la boucle" (design/tomi, charter section 2). Shown at 24 CSS px, never
   * animated here and never used as a loading indicator (charter 6.3 and 9). `currentColor` is
   * the text colour of the theme (night on a light background, light on a dark one).
   *
   * Charter 2.4: the pixel-drawn version (logo-24.svg) must never be enlarged. It is exact only
   * when one CSS pixel is one device pixel (display scale 100 %). On a scaled display (125 %,
   * 150 %...) it would be stretched into uneven blocks, so the vector drawing (logo.svg, sharp
   * from 32 device px) is used instead: 24 CSS px = 30 px at 125 %, 36 px at 150 %.
   */
  let pixelPerfect = $state(true);

  onMount(() => {
    // `resolution: 1dppx` matches only at exactly 100 % scale; it changes when the window is
    // dragged to another monitor or the user changes the display scale.
    const query = window.matchMedia("(resolution: 1dppx)");
    const update = () => (pixelPerfect = query.matches);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  });
</script>

{#if pixelPerfect}
  <svg
    class="logo"
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 24 24"
    width="24"
    height="24"
    shape-rendering="crispEdges"
    role="img"
    aria-label="Tomi"
  >
    <path
      fill="currentColor"
      d="M10 3h6v1h-6zM8 4h10v1h-10zM7 5h3v1h-3zM16 5h3v1h-3zM6 6h3v1h-3zM17 6h3v1h-3zM18 7h3v1h-3zM19 8h2v1h-2zM3 9h4v1h-4zM20 9h2v1h-2zM2 10h6v1h-6zM20 10h2v1h-2zM2 11h6v1h-6zM20 11h2v1h-2zM2 12h6v1h-6zM20 12h2v1h-2zM2 13h6v1h-6zM20 13h2v1h-2zM3 14h4v1h-4zM20 14h2v1h-2zM19 15h2v1h-2zM18 16h3v1h-3zM6 17h3v1h-3zM17 17h3v1h-3zM7 18h3v1h-3zM16 18h3v1h-3zM8 19h10v1h-10zM10 20h6v1h-6z"
    />
  </svg>
{:else}
  <svg
    class="logo"
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 256 256"
    width="24"
    height="24"
    role="img"
    aria-label="Tomi"
  >
    <path
      fill="currentColor"
      d="M61.93 70.13A94 94 0 1 1 61.93 185.87L80.84 171.1A70 70 0 1 0 80.84 84.9Z"
    />
    <path fill="currentColor" d="M22 128A32 32 0 1 0 86 128A32 32 0 1 0 22 128Z" />
  </svg>
{/if}

<style>
  .logo {
    flex: none;
    color: var(--logo);
  }
</style>
