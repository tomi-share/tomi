<script lang="ts">
  import { ICONS, type IconName } from "./icons";

  /**
   * A Material Symbols icon (charter section 6). `size` is 16 (metadata), 20 (card type, buttons) or
   * 24. The colour is `currentColor`: set it with `color` on the parent. Decorative unless `label` is set.
   */
  let {
    name,
    size = 20,
    label,
  }: { name: IconName; size?: 16 | 20 | 24; label?: string } = $props();

  const def = $derived(ICONS[name]);
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox={def.vb}
  fill="currentColor"
  role={label ? "img" : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : "true"}
  focusable="false"
>
  <path d={def.d} />
</svg>

<style>
  .icon {
    flex: none;
    display: inline-block;
    vertical-align: middle;
  }
</style>
