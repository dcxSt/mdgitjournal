<script lang="ts">
  import type { CurrentEntry } from "../lib/entry.svelte";

  let { current }: { current: CurrentEntry } = $props();
</script>

{#if current.saveError}
  <span class="save failed" title={current.saveError}>Not saved</span>
{:else}
  <span class="save">{current.dirty || current.saving ? "Saving…" : "Saved"}</span>
{/if}

<style>
  .save {
    flex: none;
    color: var(--color-muted);
  }

  .failed {
    color: var(--color-danger);
  }
</style>
