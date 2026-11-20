<!-- Confirms Abort merge (`git merge --abort`). -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import Sheet from "./Sheet.svelte";

  let { app }: { app: AppState } = $props();

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
  <p>
    Files git merged go back to how they were before the merge; changes made to them since are discarded.
  </p>
  {#if app.sync.merge?.startedByApp}
    <p class="muted">Edits you had before you chose Resolve with git come back as uncommitted changes.</p>
  {/if}
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
