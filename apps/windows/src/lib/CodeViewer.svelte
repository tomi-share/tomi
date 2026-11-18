<script lang="ts">
  import { tick } from "svelte";
  import { api, asAppError, type ItemView } from "./api";
  import { feed } from "./feed.svelte";
  import { errorMessage, format, fr, plural } from "./i18n/fr";
  import {
    HIGHLIGHT_MAX_BYTES,
    HIGHLIGHT_MAX_LINES,
    highlight,
    languageOf,
    type Token,
    type TokenKind,
  } from "./highlight";
  import Icon from "./Icon.svelte";
  import { extensionOf, isCodeFile } from "./itemLogic";

  /**
   * The code screen (Android's CodeFileScreen): colours, search in the file with next / previous,
   * a "Retour à la ligne" switch that is remembered, several files as tabs, and "Copier le
   * fichier". The text is read by Rust (2 Mo at most, never from a hidden item); the window keeps
   * it in memory, and the core's decrypted copy on disk is cleaned up like those of "Ouvrir".
   */

  const WRAP_KEY = "tomi.codeWrap";
  const MAX_MATCHES = 5000;
  const MAX_LINE_CHARS = 4000;
  const CHUNK = 100;

  interface CodeEntry {
    /** Slot of the attachment in its item (what Rust is asked for), never its position. */
    slot: number;
    name: string;
  }
  interface Run {
    s: number;
    e: number;
    kind: TokenKind;
  }
  interface Hit {
    line: number;
    s: number;
    e: number;
  }
  interface Seg {
    t: string;
    kind?: TokenKind;
    hit?: "match" | "current";
  }

  let target = $derived(feed.code);
  let entries = $state<CodeEntry[]>([]);
  let position = $state(0);
  let status = $state<"loading" | "ready" | "error">("loading");
  let errorText = $state("");
  let original = $state("");
  let lines = $state<string[]>([]);
  let notice = $state<string | null>(null);
  let runs = $state(new Map<number, Run[]>());
  let wrap = $state(readWrap());
  let searching = $state(false);
  let query = $state("");
  let current = $state(0);
  let root: HTMLElement | undefined = $state();
  let searchInput: HTMLInputElement | undefined = $state();

  function readWrap(): boolean {
    try {
      return localStorage.getItem(WRAP_KEY) === "1";
    } catch {
      return false;
    }
  }
  function setWrap(on: boolean) {
    wrap = on;
    try {
      localStorage.setItem(WRAP_KEY, on ? "1" : "0");
    } catch {
      // Remembered for this session only.
    }
  }

  // ---- loading

  async function open(itemId: string, attachmentSlot: number) {
    status = "loading";
    try {
      const view: ItemView = await api.itemView(itemId);
      // By SLOT: the position in the list changes once a file is removed.
      entries = view.attachments.filter((a) => isCodeFile(a)).map((a) => ({ slot: a.slot, name: a.name }));
      if (entries.length === 0) throw { code: "error.nothing_to_open", params: {} };
      position = Math.max(0, entries.findIndex((e) => e.slot === attachmentSlot));
      await load(itemId);
    } catch (e) {
      fail(e);
    }
  }

  async function load(itemId: string) {
    status = "loading";
    query = "";
    current = 0;
    try {
      const file = await api.readCodeFile(itemId, entries[position].slot);
      original = file.text;
      // Same display text as Android: uniform line ends, tabs as 4 spaces.
      const display = file.text.replace(/\r\n?/g, "\n").replaceAll("\t", "    ");
      lines = display.split("\n");
      colour(display, file.name);
      status = "ready";
    } catch (e) {
      fail(e);
    }
  }

  function fail(e: unknown) {
    errorText = errorMessage(asAppError(e));
    status = "error";
  }

  function colour(display: string, name: string) {
    const lang = languageOf(extensionOf(name));
    runs = new Map();
    notice = null;
    if (!lang) {
      notice = fr.code.noColoursLanguage;
      return;
    }
    if (lines.length > HIGHLIGHT_MAX_LINES || new TextEncoder().encode(display).length > HIGHLIGHT_MAX_BYTES) {
      notice = fr.code.noColoursSize;
      return;
    }
    runs = bucket(highlight(display, lang));
  }

  /** Cuts the tokens at line ends and groups them by line. */
  function bucket(tokens: Token[]): Map<number, Run[]> {
    const starts: number[] = [];
    let pos = 0;
    for (const l of lines) {
      starts.push(pos);
      pos += l.length + 1;
    }
    const out = new Map<number, Run[]>();
    let line = 0;
    for (const t of tokens) {
      while (line + 1 < starts.length && starts[line + 1] <= t.start) line++;
      let l = line;
      let from = t.start;
      while (from < t.end && l < starts.length) {
        const lineEnd = starts[l] + lines[l].length;
        const to = Math.min(t.end, lineEnd);
        if (to > from) {
          const list = out.get(l) ?? [];
          list.push({ s: from - starts[l], e: to - starts[l], kind: t.kind });
          out.set(l, list);
        }
        from = lineEnd + 1;
        l++;
      }
    }
    return out;
  }

  $effect(() => {
    const t = target;
    if (!t) return;
    void open(t.itemId, t.slot);
  });

  async function pick(i: number) {
    if (i === position || !target) return;
    position = i;
    await load(target.itemId);
  }

  // ---- search

  const hits = $derived.by((): Hit[] => {
    if (!searching || query === "") return [];
    const re = new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi");
    const out: Hit[] = [];
    for (let i = 0; i < lines.length && out.length < MAX_MATCHES; i++) {
      const text = lines[i].slice(0, MAX_LINE_CHARS);
      re.lastIndex = 0;
      let m: RegExpExecArray | null;
      while ((m = re.exec(text)) && out.length < MAX_MATCHES) {
        out.push({ line: i, s: m.index, e: m.index + m[0].length });
      }
    }
    return out;
  });
  const hitsByLine = $derived.by(() => {
    const map = new Map<number, { s: number; e: number; idx: number }[]>();
    hits.forEach((h, idx) => {
      const list = map.get(h.line) ?? [];
      list.push({ s: h.s, e: h.e, idx });
      map.set(h.line, list);
    });
    return map;
  });
  const counter = $derived(
    hits.length === 0
      ? "0/0"
      : `${Math.min(current, hits.length - 1) + 1}/${hits.length}${hits.length >= MAX_MATCHES ? "+" : ""}`,
  );

  // The first match from the top when the text changes.
  $effect(() => {
    void query;
    current = 0;
  });

  async function go(delta: number) {
    if (hits.length === 0) return;
    current = (current + delta + hits.length) % hits.length;
    await tick();
    root?.querySelector("[data-current]")?.scrollIntoView({ block: "center", inline: "center" });
  }

  $effect(() => {
    // Scroll to the first match as the user types.
    if (hits.length > 0 && current === 0) {
      void tick().then(() => root?.querySelector("[data-current]")?.scrollIntoView({ block: "center" }));
    }
  });

  async function toggleSearch(on = !searching) {
    searching = on;
    if (on) {
      await tick();
      searchInput?.focus();
      searchInput?.select();
    } else {
      query = "";
    }
  }

  // ---- rendering

  /** The pieces of one line: coloured runs and search matches, merged. */
  function segments(i: number): Seg[] | null {
    const text = lines[i].length > MAX_LINE_CHARS ? lines[i].slice(0, MAX_LINE_CHARS) + "…" : lines[i];
    const r = runs.get(i);
    const m = hitsByLine.get(i);
    if (!r && !m) return null;
    const cuts = new Set<number>([0, text.length]);
    for (const x of r ?? []) {
      cuts.add(Math.min(x.s, text.length));
      cuts.add(Math.min(x.e, text.length));
    }
    for (const x of m ?? []) {
      cuts.add(Math.min(x.s, text.length));
      cuts.add(Math.min(x.e, text.length));
    }
    const points = [...cuts].sort((a, b) => a - b);
    const out: Seg[] = [];
    for (let k = 0; k + 1 < points.length; k++) {
      const a = points[k];
      const b = points[k + 1];
      if (b <= a) continue;
      const kind = r?.find((x) => x.s <= a && a < x.e)?.kind;
      const hit = m?.find((x) => x.s <= a && a < x.e);
      out.push({
        t: text.slice(a, b),
        kind,
        hit: hit ? (hit.idx === current ? "current" : "match") : undefined,
      });
    }
    return out;
  }

  const chunks = $derived(Array.from({ length: Math.ceil(lines.length / CHUNK) }, (_, c) => c));
  const digits = $derived(String(lines.length).length);

  async function copyFile() {
    try {
      await navigator.clipboard.writeText(original);
      feed.say(fr.code.copied);
    } catch {
      feed.say(errorMessage("error.clipboard"), "error");
    }
  }

  function close() {
    feed.code = null;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (searching) void toggleSearch(false);
      else close();
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") {
      e.preventDefault();
      void toggleSearch(true);
    } else if (e.key === "F3" || (e.key === "Enter" && document.activeElement === searchInput)) {
      e.preventDefault();
      void go(e.shiftKey ? -1 : 1);
    }
  }

  $effect(() => {
    root?.focus();
  });
</script>

<div
  bind:this={root}
  class="screen"
  role="dialog"
  aria-modal="true"
  aria-label={fr.code.title}
  tabindex="-1"
  onkeydown={onKey}
>
  <header class="bar">
    <button class="ghost icon" type="button" aria-label={fr.code.close} onclick={close}>
      <Icon name="arrow_back" size={24} />
    </button>
    <div class="grow tabs" role="tablist" aria-label={fr.code.title}>
      {#if entries.length > 1}
        {#each entries as f, i (f.slot)}
          <button
            class="chip mono"
            type="button"
            role="tab"
            aria-selected={i === position}
            onclick={() => pick(i)}>{f.name}</button
          >
        {/each}
        <span class="muted small tnum">{format(fr.code.position, { n: position + 1, total: entries.length })}</span>
      {:else if entries[0]}
        <span class="mono ellipsis name">{entries[0].name}</span>
      {/if}
    </div>
    <button
      class="ghost icon"
      class:on={searching}
      type="button"
      title={fr.code.search}
      aria-label={fr.code.search}
      aria-pressed={searching}
      onclick={() => toggleSearch()}
    >
      <Icon name="search" size={20} />
    </button>
    <button
      class="chip"
      type="button"
      aria-pressed={wrap}
      title={fr.code.wrap}
      onclick={() => setWrap(!wrap)}>{fr.code.wrap}</button
    >
    <button
      class="ghost icon"
      type="button"
      title={fr.code.copyFile}
      aria-label={fr.code.copyFile}
      disabled={status !== "ready"}
      onclick={copyFile}
    >
      <Icon name="content_copy" size={20} />
    </button>
  </header>

  {#if searching}
    <div class="searchbar row">
      <input
        bind:this={searchInput}
        type="search"
        bind:value={query}
        placeholder={fr.code.searchPlaceholder}
        aria-label={fr.code.search}
      />
      <span class="muted small tnum counter" aria-live="polite">{counter}</span>
      <button class="ghost icon" type="button" aria-label={fr.code.previous} disabled={hits.length === 0} onclick={() => go(-1)}>
        <span class="up"><Icon name="expand_more" size={20} /></span>
      </button>
      <button class="ghost icon" type="button" aria-label={fr.code.next} disabled={hits.length === 0} onclick={() => go(1)}>
        <Icon name="expand_more" size={20} />
      </button>
    </div>
  {/if}

  {#if status === "loading"}
    <p class="state muted">{fr.code.loading}</p>
  {:else if status === "error"}
    <p class="state" role="alert">{errorText}</p>
  {:else}
    {#if notice}<p class="notice muted small">{notice} · {plural(fr.code.lines, lines.length)}</p>{/if}
    <div class="body" class:wrap>
      {#each chunks as c (c)}
        <div class="chunk">
          {#each lines.slice(c * CHUNK, c * CHUNK + CHUNK) as line, k (c * CHUNK + k)}
            {@const i = c * CHUNK + k}
            {@const segs = segments(i)}
            <div class="ln">
              <span class="no tnum muted" style:min-width="{digits}ch" aria-hidden="true">{i + 1}</span>
              <span class="code selectable"
                >{#if segs}{#each segs as s, j (j)}<span
                      class={s.kind ? `tok-${s.kind}` : undefined}
                      class:hit={s.hit === "match"}
                      class:hit-current={s.hit === "current"}
                      data-current={s.hit === "current" ? "" : undefined}>{s.t}</span
                    >{/each}{:else}{line.length > MAX_LINE_CHARS ? line.slice(0, MAX_LINE_CHARS) + "…" : line}{/if}</span
              >
            </div>
          {/each}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .screen {
    position: fixed;
    inset: 0;
    z-index: 25;
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
    border-bottom: var(--border-width) solid var(--border);
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    overflow-x: auto;
  }
  .name {
    font-weight: 500;
  }
  .tabs .chip {
    flex: none;
  }
  .searchbar {
    padding: var(--space-2) var(--space-3);
    border-bottom: var(--border-width) solid var(--border);
  }
  .counter {
    min-width: 7ch;
    text-align: center;
  }
  .up {
    display: inline-flex;
    transform: rotate(180deg);
  }
  .state {
    padding: var(--space-6);
    text-align: center;
    margin: 0;
  }
  .notice {
    margin: 0;
    padding: var(--space-1) var(--space-3);
    background: var(--surface-alt);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    background: var(--surface);
    font-family: var(--font-mono);
    font-size: 13px;
    line-height: 20px;
    padding: var(--space-2) 0;
  }
  .chunk {
    content-visibility: auto;
    contain-intrinsic-size: auto 2000px;
  }
  .ln {
    display: flex;
    gap: var(--space-3);
    padding: 0 var(--space-4) 0 var(--space-3);
    width: max-content;
    min-width: 100%;
  }
  .wrap .ln {
    width: auto;
  }
  .no {
    flex: none;
    text-align: right;
    user-select: none;
    font-feature-settings: "tnum" 1;
  }
  .code {
    flex: 1;
    white-space: pre;
    min-height: 20px;
  }
  .wrap .code {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .tok-comment {
    color: var(--muted);
    font-style: italic;
  }
  .tok-string {
    color: var(--success);
  }
  .tok-number {
    color: var(--secondary);
  }
  .tok-keyword,
  .tok-tag {
    color: var(--brand);
    font-weight: 500;
  }
  .tok-literal {
    color: var(--secret);
  }
  .tok-attr {
    color: var(--secondary);
  }
  .tok-fn {
    color: var(--text);
    font-weight: 500;
  }
  .hit {
    background: color-mix(in srgb, var(--secondary) 28%, transparent);
    border-radius: 2px;
  }
  .hit-current {
    background: color-mix(in srgb, var(--secondary) 60%, transparent);
    outline: 1px solid var(--secondary);
    border-radius: 2px;
  }
</style>
