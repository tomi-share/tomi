<script lang="ts">
  import type { Qr } from "../api";

  /** The QR code of the pairing link, drawn from the modules the core's link was encoded to. */
  let { qr, label }: { qr: Qr; label: string } = $props();

  /** Modules across, quiet zone included. */
  const side = $derived(qr.size + 2 * qr.quiet);
  /** CSS pixels a module: a whole number, so that the modules are even at 100 % and 200 %. */
  const MODULE_PX = 5;
</script>

<!-- Dark modules on a light ground that covers the quiet zone, in any theme: a scanner reads
     a QR code that way round. Not the colours of the symbol, which are reserved to it. -->
<svg
  class="qr"
  viewBox="{-qr.quiet} {-qr.quiet} {side} {side}"
  style:width="{side * MODULE_PX}px"
  style:height="{side * MODULE_PX}px"
  role="img"
  aria-label={label}
  shape-rendering="crispEdges"
>
  <rect x={-qr.quiet} y={-qr.quiet} width={side} height={side} fill="#ffffff" />
  <path d={qr.path} fill="#000000" />
</svg>

<style>
  .qr {
    flex: none;
    display: block;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
</style>
