<!-- Pull and Commit & Push (PLAN §5). Without GitHub there is only Commit. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { shortcutLabel } from "../lib/shortcuts";
  import { commitButton, pullButton } from "../lib/sync";
  import Icon from "./Icon.svelte";

  /** `compact`: the phone's top bar, where the "Pull first" hint replaces the label. */
  let { app, compact = false }: { app: AppState; compact?: boolean } = $props();

  const pull = $derived(pullButton(app.sync, app.ops));
  const commit = $derived(commitButton(app.sync, app.ops));
  const error = $derived(app.lastError);
  const pulling = $derived(app.ops.includes("pull"));
</script>

{#if app.sync.connected}
  <button
    type="button"
    class="sync"
    class:accent={pull.accent}
    disabled={pulling}
    title={error ? `Last sync failed: ${error.message}` : `${pull.title} (${shortcutLabel("pull", app.mac)})`}
    onclick={() => (error ? app.showSyncError() : void app.pull())}
  >
    {#if pull.busy}<span class="spinner" aria-hidden="true"></span>{:else}<Icon name="pull" size={13} />{/if}
    <span>Pull</span>
    {#if pull.badge !== null}<span class="badge" aria-label={pull.title}>{pull.badge}</span>{/if}
    {#if error}<span class="dot" role="img" aria-label="Last sync failed"></span>{/if}
  </button>
{/if}
<button
  type="button"
  class="sync"
  class:accent={commit.accent}
  disabled={commit.disabled || commit.busy}
  title={`${commit.title} (${shortcutLabel("commit", app.mac)})`}
  onclick={() => void app.openCommit()}
>
  {#if commit.busy}<span class="spinner" aria-hidden="true"></span>{:else}<Icon name="push" size={13} />{/if}
  <span>{compact && commit.hint ? commit.hint : commit.label}</span>
  {#if commit.badge !== null}<span class="badge" aria-label={commit.title}>{commit.badge}</span>{/if}
</button>
{#if commit.hint && !compact}
  <span class="hint">{commit.hint}</span>
{/if}

<style>
  .sync {
    display: inline-flex;
    flex: none;
    align-items: center;
    gap: 5px;
    min-height: var(--sync-height, 24px);
    color: var(--color-muted);
    white-space: nowrap;
  }

  .sync.accent {
    color: var(--color-accent);
  }

  .sync:not(.accent) .badge {
    background: var(--color-muted);
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-danger);
  }

  .hint {
    flex: none;
    color: var(--color-muted);
    font-style: italic;
  }
</style>
