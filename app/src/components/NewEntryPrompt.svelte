<!-- The inline title prompt behind "+ New": Enter creates, an empty title makes "untitled". -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { slugPreview } from "../lib/journal";

  let { app }: { app: AppState } = $props();

  let title = $state("");
  let busy = $state(false);
  const slug = $derived(slugPreview(title));

  async function create(e: SubmitEvent) {
    e.preventDefault();
    if (busy) return;
    busy = true;
    await app.create(title);
    busy = false;
  }
</script>

<form onsubmit={create}>
  <input
    {@attach (node) => node.focus()}
    type="text"
    bind:value={title}
    placeholder="Title, or Enter for untitled"
    aria-label="New entry title"
    autocomplete="off"
    spellcheck="false"
    enterkeyhint="go"
    readonly={busy}
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        app.newPrompt = false;
      }
    }}
    onblur={() => {
      if (!busy) app.newPrompt = false;
    }}
  />
  {#if slug && title.trim()}
    <span class="preview" aria-hidden="true">…-{slug}.md</span>
  {/if}
</form>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  input {
    width: 100%;
  }

  .preview {
    padding: 0 9px;
    color: var(--color-muted);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
