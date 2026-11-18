<script lang="ts">
  import { api, type JobView } from "./api";
  import { feed } from "./feed.svelte";
  import Icon from "./Icon.svelte";
  import { errorMessage, formatBytes, msg } from "./i18n";

  let now = $state(Date.now());

  // Countdown for the cancellation window.
  $effect(() => {
    if (!feed.jobs.some((j) => j.state === "pending")) return;
    const t = setInterval(() => (now = Date.now()), 250);
    return () => clearInterval(t);
  });

  // A finished job disappears by itself.
  $effect(() => {
    const done = feed.jobs.filter((j) => j.state === "done" || j.state === "queued");
    // "Enregistré : il partira dès que le réseau revient" is longer to read.
    const timers = done.map((j) =>
      setTimeout(() => feed.dropJob(j.id), j.state === "queued" ? 6000 : 2500),
    );
    return () => timers.forEach(clearTimeout);
  });

  function seconds(job: JobView): number {
    return job.state === "pending" ? Math.max(0, Math.ceil((job.sendAt - now) / 1000)) : 0;
  }

  function progress(job: JobView): number {
    return job.state === "sending" && job.totalBytes > 0 ? job.sentBytes / job.totalBytes : 0;
  }

  async function cancel(id: string) {
    await feed.run(() => api.cancelJob(id));
  }
</script>

{#if feed.jobs.length > 0}
  <!-- Snackbar anchored at the bottom of the window (like Flash.svelte): it never moves the content. -->
  <div class="jobs" class:raised={feed.flash !== null}>
    {#each feed.jobs as job (job.id)}
      <div class="job" class:failed={job.state === "failed"} role="status" aria-live="polite">
        {#if job.state === "pending"}
          <!-- "Envoi dans 3 s · Annuler": the undo window. -->
          <span class="text">{msg.job.pending} <span class="tnum count">{seconds(job)} s</span></span>
          <button class="action" type="button" onclick={() => cancel(job.id)}>{msg.job.cancel}</button>
        {:else if job.state === "sending"}
          <span class="text">
            {msg.job.sending}
            {#if job.totalBytes > 0}<span class="tnum count">{formatBytes(job.sentBytes)} / {formatBytes(job.totalBytes)}</span>{/if}
          </span>
          {#if job.totalBytes > 0}<progress value={progress(job)} max="1" aria-label={msg.job.sending}></progress>{/if}
        {:else if job.state === "done"}
          <Icon name="check" size={20} />
          <span class="text">{msg.job.done}</span>
        {:else if job.state === "queued"}
          <Icon name="schedule" size={20} />
          <span class="text wrap">{msg.job.queued}</span>
        {:else if job.state === "failed"}
          <Icon name="error" size={20} />
          <span class="text wrap">{msg.job.failed} : {errorMessage(job.error)}</span>
          <button class="action" type="button" onclick={() => feed.run(() => api.retryJob(job.id))}>{msg.job.retry}</button>
          <button
            class="action quiet"
            type="button"
            onclick={() => feed.run(() => api.dismissJob(job.id)).then(() => feed.dropJob(job.id))}
            >{msg.job.dismiss}</button
          >
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  /* Snackbar: inverse surface, same family as Flash.svelte; a failure is a filled error surface. */
  .jobs {
    position: fixed;
    left: var(--space-3);
    right: var(--space-3);
    bottom: var(--space-3);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    z-index: 49;
    pointer-events: none;
  }
  /* A flash notice (Archivé · Annuler...) sits at the very bottom: the job bar rises above it. */
  .jobs.raised {
    bottom: calc(var(--space-3) + 52px);
  }
  .job {
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 44px;
    padding: var(--space-1) var(--space-2) var(--space-1) var(--space-3);
    border-radius: var(--radius-md);
    background: var(--text);
    color: var(--bg);
    box-shadow: 0 4px 16px color-mix(in srgb, var(--text) 24%, transparent);
    animation: rise var(--dur-medium) ease-out;
  }
  .job.failed {
    background: var(--error);
    color: var(--on-error);
  }
  .text {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Long messages (offline queue, failure with its reason) may take two lines. */
  .text.wrap {
    white-space: normal;
  }
  .count {
    margin-left: var(--space-1);
    opacity: 0.8;
  }
  /* Button in the colour of the bar, clearly readable (Android: the snackbar action). */
  .action {
    flex: none;
    min-height: 36px;
    padding: 0 var(--space-3);
    border: 0;
    border-radius: var(--radius-sm);
    background: color-mix(in srgb, var(--bg) 18%, transparent);
    color: inherit;
    font-weight: 600;
    white-space: nowrap;
  }
  .action:hover {
    background: color-mix(in srgb, var(--bg) 28%, transparent);
  }
  .action.quiet {
    background: transparent;
    text-decoration: underline;
  }
  .action.quiet:hover {
    background: color-mix(in srgb, var(--bg) 16%, transparent);
  }
  progress {
    flex: none;
    width: 96px;
  }
  @keyframes rise {
    from {
      transform: translateY(8px);
      opacity: 0;
    }
  }
</style>
