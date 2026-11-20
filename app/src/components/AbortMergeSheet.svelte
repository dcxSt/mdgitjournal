<!-- Confirms Abort merge (`git merge --abort`), naming every file it puts back. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import Sheet from "./Sheet.svelte";

  let { app }: { app: AppState } = $props();

  const merge = $derived(app.sync.merge);

  let aborting = $state(false);
  let error = $state<string | null>(null);

  function close() {
    if (!aborting) app.closeOverlay();
  }

  async function abort() {
    aborting = true;
    error = null;
    const failure = await app.abortMerge();
    aborting = false;
    if (failure) error = failure.message;
    else close();
  }
</script>

<Sheet label="Abort merge" onclose={close}>
  <h2>Abort the merge?</h2>
  {#if merge?.conflicted.length}
    <p>These files go back to how they were before the merge, conflict markers and any resolving with them:</p>
    <ul class="paths">
      {#each merge.conflicted as path (path)}<li class="mono">{path}</li>{/each}
    </ul>
  {/if}
  {#if merge?.merged.length}
    <p>
      So do the {merge.merged.length === 1 ? "file" : "files"} git merged on {merge.merged.length === 1 ? "its" : "their"} own (if you
      edited one since, Abort stops and says so, discarding nothing):
    </p>
    <ul class="paths">
      {#each merge.merged as path (path)}<li class="mono">{path}</li>{/each}
    </ul>
  {/if}
  {#if merge?.startedByApp && merge.precommit}
    <p class="muted">Edits you had before you chose Resolve with git come back as uncommitted changes.</p>
  {/if}
  {#if merge?.autostash}
    <p class="muted">The uncommitted changes git set aside when the merge started (--autostash) come back too.</p>
  {/if}
  <p class="muted">Other files are left as they are.</p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    <button type="button" class="button" data-autofocus disabled={aborting} onclick={close}>Cancel</button>
    <button type="button" class="button danger" disabled={aborting} onclick={abort}>
      {#if aborting}<span class="spinner" aria-hidden="true"></span>{/if}
      Abort merge
    </button>
  </div>
</Sheet>

<style>
  h2 {
    margin: 0 0 8px;
    font-size: 15px;
    font-weight: 500;
  }

  p {
    margin: 0 0 8px;
  }

  .error {
    color: var(--color-danger);
  }

  .paths {
    margin: 0 0 8px;
    padding-left: 18px;
    max-height: 9em;
    overflow: auto;
  }

  .mono {
    font-family: var(--font-source);
    font-size: 12px;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }

  .actions .button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  :global([data-layout="phone"]) .actions .button {
    min-height: var(--touch);
    padding: 0 14px;
  }
</style>
