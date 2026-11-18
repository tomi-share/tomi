<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import {
    api,
    events,
    type AllExpiryRules,
    type Attachment,
    type DraftInput,
    type ExpiryChoice,
    type PrefillPayload,
    type Rejection,
    type SourceFormat,
  } from "./api";
  import { feed } from "./feed.svelte";
  import Icon from "./Icon.svelte";
  import { errorMessage, format, formatBytes, fr } from "./i18n/fr";

  const MAX_FILE_BYTES = 25 * 1024 * 1024;

  let text = $state("");
  let title = $state("");
  let url = $state("");
  /** Hidden ("masqué"): the text is the value to hide; the title is hidden with it. */
  let secret = $state(false);
  /** Shows what is typed in a hidden item (it is masked by default). */
  let showTyped = $state(false);
  let attachments = $state<Attachment[]>([]);
  let expiry = $state<ExpiryChoice>("never");
  let rules = $state<AllExpiryRules | null>(null);
  /** Rich flavour of the pasted text, kept while the text is untouched. */
  let rich = $state<{ plain: string; html: string | null; format: SourceFormat | null } | null>(null);
  let notes = $state<string[]>([]);
  let dropping = $state(false);
  let secretHint = $state(false);
  let sending = $state(false);
  let area: HTMLTextAreaElement | undefined = $state();

  /** The other devices the item goes to ("Vers Pixel, Tablette"). */
  let targets = $state<string[]>([]);

  async function loadTargets() {
    try {
      const all = await api.devices();
      targets = all.filter((d) => !d.thisDevice && d.state === "verified").map((d) => d.name);
    } catch {
      targets = [];
    }
  }

  const attachBytes = $derived(attachments.reduce((n, a) => n + a.size, 0));
  /** A hidden item with files has no "Après lecture" (the first read would erase the files). */
  const kindRules = $derived(
    rules ? (secret ? (attachments.length > 0 ? rules.secretWithFiles : rules.secret) : rules.normal) : null,
  );
  const noReadOnce = $derived(secret && attachments.length > 0);
  /**
   * "Après lecture" was chosen and a file was added to the hidden item. The choice is KEPT (never
   * silently switched to "Jamais", which would make a read-once secret last forever): the send is
   * blocked and the message is highlighted until the user picks another duration or removes the
   * files. `build_draft` refuses the combination too.
   */
  const readOnceWithFiles = $derived(noReadOnce && expiry === "afterFirstRead");
  /** What the select offers; the conflicting choice stays visible so the user sees what blocks. */
  const options = $derived(
    readOnceWithFiles ? ["afterFirstRead" as ExpiryChoice, ...(kindRules?.options ?? [])] : (kindRules?.options ?? []),
  );
  const canSend = $derived(
    !sending && !readOnceWithFiles && (text.trim() !== "" || url.trim() !== "" || attachments.length > 0),
  );

  // A choice that is not offered for this kind of content falls back to its default, except the
  // read-once-with-files conflict above, which the user resolves.
  $effect(() => {
    if (kindRules && !readOnceWithFiles && !kindRules.options.includes(expiry)) expiry = kindRules.default;
  });

  onMount(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    api.expiryRules().then((r) => {
      rules = r;
      expiry = r.normal.default;
    });
    void loadTargets();
    let stopSpace: (() => void) | undefined;
    events.spaceChanged(() => void loadTargets()).then((u) => (cancelled ? u() : (stopSpace = u)));
    // Files dropped on the window (the webview gives real paths).
    getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "enter") dropping = true;
        else if (p.type === "leave") dropping = false;
        else if (p.type === "drop") {
          dropping = false;
          void addPaths(p.paths);
        }
      })
      .then((u) => (cancelled ? u() : (unlisten = u)));
    // What is typed in a hidden item is shown only while the window is in use: leaving it (blur,
    // popover hidden) puts the mask back, like `feed.hideAllValues()` does for the feed.
    const hideTyped = () => (showTyped = false);
    const onVisibility = () => {
      if (document.hidden) hideTyped();
    };
    window.addEventListener("blur", hideTyped);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      cancelled = true;
      unlisten?.();
      stopSpace?.();
      window.removeEventListener("blur", hideTyped);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  });

  export function focus(): void {
    area?.focus();
  }

  /** Content from a tomi-share:// link or "Envoyer vers": fills the form, sends nothing. */
  export function applyPrefill(p: PrefillPayload): void {
    if (p.error) {
      feed.say(errorMessage(p.error), "error");
      return;
    }
    // Never overwrite what the user already typed: a received link or title only fills
    // an empty field.
    let kept = false;
    if (p.url) {
      if (url.trim() === "") url = p.url;
      else kept = true;
    }
    if (p.title) {
      if (title.trim() === "") title = p.title;
      else kept = true;
    }
    if (kept) feed.say(fr.composer.prefillKept);
    if (p.text) text = text ? `${text}\n${p.text}` : p.text;
    if (p.files.length) attachments = [...attachments, ...p.files];
    if (p.rejected.length) notes = p.rejected.map(rejectionNote);
    if (p.truncated) feed.say(fr.truncatedPrefill);
    focus();
  }

  function rejectionNote(r: Rejection): string {
    return format(fr.rejected[r.reason], { name: r.name });
  }

  function reset(): void {
    text = title = url = "";
    secret = false;
    showTyped = false;
    attachments = [];
    rich = null;
    notes = [];
    secretHint = false;
    if (rules) expiry = rules.normal.default;
  }

  async function addPaths(paths: string[]): Promise<void> {
    try {
      const r = await api.describeFiles(paths, attachBytes, attachments.length);
      attachments = [...attachments, ...r.accepted];
      notes = r.rejected.map(rejectionNote);
    } catch (e) {
      feed.fail(e);
    }
  }

  async function pick(): Promise<void> {
    try {
      const r = await api.pickFiles(attachBytes, attachments.length);
      attachments = [...attachments, ...r.accepted];
      notes = r.rejected.map(rejectionNote);
    } catch (e) {
      feed.fail(e);
    }
  }

  async function stageFiles(files: File[]): Promise<void> {
    const rejected: string[] = [];
    for (const f of files) {
      if (f.size > MAX_FILE_BYTES) {
        rejected.push(format(fr.rejected.tooLarge, { name: f.name || "image" }));
        continue;
      }
      try {
        const name = f.name || (f.type === "image/png" ? "capture.png" : "fichier");
        const a = await api.stageBlob(name, new Uint8Array(await f.arrayBuffer()));
        attachments = [...attachments, a];
      } catch (e) {
        feed.fail(e);
      }
    }
    notes = rejected;
  }

  function onPaste(e: ClipboardEvent): void {
    const cd = e.clipboardData;
    if (!cd) return;
    const files = Array.from(cd.files);
    if (files.length) {
      e.preventDefault();
      void stageFiles(files);
      return;
    }
    const plain = cd.getData("text/plain");
    if (!plain) return;
    e.preventDefault();
    const start = area?.selectionStart ?? text.length;
    const end = area?.selectionEnd ?? text.length;
    void pastedText(plain, cd.getData("text/html") || null, start, end);
  }

  async function pastedText(plain: string, html: string | null, start: number, end: number) {
    let d;
    try {
      d = await api.detectContent(plain, html);
    } catch {
      d = null;
    }
    const wasEmpty = text.trim() === "" && url === "";
    // A link becomes a link item, except in a hidden item where it is just the value.
    if (!secret && d?.kind === "link" && d.url && wasEmpty) {
      url = d.url;
      title = d.title ?? "";
      return;
    }
    text = text.slice(0, start) + plain + text.slice(end);
    if (!secret && wasEmpty && d && (d.kind === "richText" || (d.sourceFormat && d.sourceFormat !== "plain"))) {
      rich = { plain, html, format: d.sourceFormat };
    }
    secretHint = !!d?.looksLikeSecret && !secret;
  }

  /**
   * Hides the item. The choice of expiry is kept when it is still offered (as on Android);
   * turning a hidden item back into a plain one drops "Après lecture" to "Jamais".
   */
  function setSecret(on: boolean): void {
    secret = on;
    secretHint = false;
    showTyped = false;
    if (on) rich = null;
    if (rules) {
      const next = on ? rules.secret : rules.normal;
      if (!next.options.includes(expiry)) expiry = next.default;
    }
  }

  async function removeAttachment(i: number): Promise<void> {
    const [a] = attachments.splice(i, 1);
    if (a?.path) await api.discardStaged(a.path).catch(() => {});
  }

  async function send(): Promise<void> {
    if (!canSend) return;
    sending = true;
    try {
      let linkUrl = url.trim() || null;
      let linkTitle = title.trim() || null;
      let plain = text;
      let htmlFlavour: string | null = null;
      let sourceFormat: SourceFormat | null = null;

      if (plain.trim() && !secret) {
        const same = rich !== null && rich.plain.trim() === plain.trim();
        const d = await api.detectContent(plain, same ? rich!.html : null).catch(() => null);
        if (!linkUrl && d?.kind === "link" && d.url) {
          linkUrl = d.url;
          linkTitle = linkTitle ?? d.title;
          plain = "";
        } else {
          htmlFlavour = same ? rich!.html : null;
          sourceFormat = same ? (rich!.format ?? d?.sourceFormat ?? null) : (d?.sourceFormat ?? null);
        }
      }

      const input: DraftInput = {
        title: linkTitle,
        url: linkUrl,
        text: plain.trim()
          ? {
              plain,
              html: htmlFlavour,
              markdown: sourceFormat === "markdown" ? plain : null,
            }
          : null,
        attachments,
        secret,
        expiry,
        sourceFormat,
      };
      await api.sendDraft(input);
      reset();
    } catch (e) {
      feed.fail(e);
    } finally {
      sending = false;
    }
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void send();
    }
  }
</script>

<section class="composer" class:dropping aria-label={fr.composer.label}>
  {#if dropping}<div class="drop">{fr.composer.drop}</div>{/if}

  <textarea
    bind:this={area}
    bind:value={text}
    class:masked={secret && !showTyped}
    class:secret
    rows="3"
    placeholder={secret ? fr.composer.placeholderSecret : fr.composer.placeholder}
    aria-label={fr.composer.label}
    autocomplete="off"
    spellcheck={secret ? false : undefined}
    onpaste={onPaste}
    onkeydown={onKey}
    oninput={() => {
      if (rich && rich.plain.trim() !== text.trim()) rich = null;
    }}
  ></textarea>

  {#if secretHint}
    <div class="hint row">
      <Icon name="visibility_off" size={20} />
      <span class="grow">{fr.composer.looksSecret}</span>
      <button onclick={() => setSecret(true)}>{fr.composer.markSecret}</button>
      <button class="ghost icon" onclick={() => (secretHint = false)} aria-label={fr.common.close}>
        <Icon name="close" size={20} />
      </button>
    </div>
  {/if}

  {#if url || secret}
    <div class="row">
      <input
        type="text"
        bind:value={title}
        placeholder={secret ? fr.composer.titleSecret : fr.composer.title}
        aria-label={secret ? fr.composer.titleSecret : fr.composer.title}
        maxlength="300"
        autocomplete="off"
        spellcheck={secret ? false : undefined}
      />
      {#if secret}
        <button class="ghost" onclick={() => (showTyped = !showTyped)} aria-pressed={showTyped}>
          <Icon name={showTyped ? "visibility_off" : "visibility"} size={20} />
          {showTyped ? fr.composer.hideInput : fr.composer.showInput}
        </button>
      {/if}
    </div>
  {/if}
  {#if url}
    <div class="row">
      <input
        type="text"
        class="mono"
        bind:value={url}
        placeholder={fr.composer.url}
        aria-label={fr.composer.url}
        autocomplete="off"
        spellcheck={secret ? false : undefined}
      />
      <button class="ghost icon" onclick={() => (url = "")} aria-label={fr.composer.removeField}>
        <Icon name="close" size={20} />
      </button>
    </div>
  {/if}

  {#if attachments.length}
    <ul class="chips">
      {#each attachments as a, i (`${i}:${a.name}`)}
        <li class="file row">
          {#if a.thumbnail}<img src={a.thumbnail} alt="" />{:else}<Icon name="draft" size={16} />{/if}
          <span class="grow nm">{a.name}</span>
          <span class="muted small tnum">{formatBytes(a.size)}</span>
          <button
            class="ghost icon"
            onclick={() => removeAttachment(i)}
            aria-label={format(fr.composer.removeFile, { name: a.name })}
          >
            <Icon name="close" size={20} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  {#each notes as n (n)}
    <div class="note small" role="alert">{n}</div>
  {/each}

  <div class="row bar">
    <label class="switch row">
      <input type="checkbox" checked={secret} onchange={(e) => setSecret(e.currentTarget.checked)} />
      <span>
        {fr.composer.secret}
        <span class="muted small block">{secret ? fr.composer.secretHint : fr.composer.secretOff}</span>
      </span>
    </label>
  </div>

  <div class="row bar">
    <button class="ghost icon" onclick={pick} title={fr.composer.addFiles} aria-label={fr.composer.addFiles}>
      <Icon name="attach_file" size={20} />
    </button>
    <span class="grow"></span>
    <label class="row small" title={fr.composer.expires}>
      <span class="muted">{fr.composer.expires}</span>
      <select bind:value={expiry} aria-label={fr.composer.expires} aria-invalid={readOnceWithFiles}>
        {#each options as o (o)}
          <option value={o}>{fr.expiry[o]}</option>
        {/each}
      </select>
    </label>
    <button class="primary" disabled={!canSend} onclick={send}>
      <Icon name="send" size={20} />
      {fr.composer.send}
    </button>
  </div>
  {#if expiry === "afterFirstRead" && !readOnceWithFiles}
    <div class="muted small">{fr.composer.afterReadHint}</div>
  {/if}
  {#if readOnceWithFiles}
    <div class="note small" role="alert">{fr.composer.afterReadNoFiles}</div>
  {:else if noReadOnce}
    <div class="muted small">{fr.composer.afterReadNoFiles}</div>
  {/if}
  {#if targets.length > 0}
    <div class="muted small">{format(fr.composer.to, { devices: targets.join(", ") })}</div>
  {/if}
</section>

<style>
  .composer {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
    background: var(--surface);
    border: var(--border-width) solid var(--border);
    border-radius: var(--radius-md);
  }
  .composer.dropping {
    border-color: var(--brand);
    border-style: dashed;
  }
  .drop {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--brand) 14%, var(--surface));
    border-radius: var(--radius-md);
    font-weight: 600;
    z-index: 2;
  }
  textarea {
    min-height: 76px;
  }
  /* The value of a hidden item is masked while it is typed (Chromium-based WebView2). */
  textarea.masked {
    -webkit-text-security: disc;
  }
  textarea.secret {
    font-family: var(--font-mono);
    font-feature-settings: "zero" 1;
    background: var(--secret-container);
  }
  .hint {
    background: var(--notice-bg);
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-md);
    color: var(--text);
  }
  .note {
    color: var(--error);
  }
  .chips {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .file {
    background: var(--surface-alt);
    border-radius: var(--radius-sm);
    padding: 0 var(--space-1) 0 var(--space-2);
    color: var(--muted);
  }
  /* The whole name, in the proportional font like the tiles of the cards: it wraps, it is never cut. */
  .file .nm {
    min-width: 0;
    padding: var(--space-1) 0;
    color: var(--text);
    font-size: var(--fs-label);
    line-height: var(--lh-label);
    font-weight: 500;
    overflow-wrap: anywhere;
  }
  .file img {
    width: 24px;
    height: 24px;
    object-fit: cover;
    border-radius: var(--radius-sm);
  }
  .file button {
    min-height: 32px;
    width: 32px;
  }
  .switch {
    align-items: flex-start;
    cursor: pointer;
    font-weight: 500;
    font-size: var(--fs-label);
  }
  .switch input {
    margin-top: 2px;
  }
  .block {
    display: block;
    font-weight: 400;
  }
  .bar {
    flex-wrap: wrap;
  }
  .bar select {
    width: auto;
    min-height: 32px;
  }
</style>