<!-- The entry title, edited in place. Only the "-title" part of the file name changes. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { slugPreview } from "../lib/journal";

  let { app }: { app: AppState } = $props();

  const meta = $derived(app.currentMeta);
  // A string, so re-listing entries (a new but equal meta) doesn't reset the draft.
  const title = $derived(meta?.title ?? "");
  // Follows the file's title, and holds the user's text while they type.
  let draft = $derived(title);
  let editing = $state(false);
  const slug = $derived(slugPreview(draft));
  // Only text the user changed renames: a file whose name isn't in slug form (a Keep both copy,
  // "My_Notes") keeps it when the field merely gains and loses focus.
  const changed = $derived(meta !== null && draft !== meta.title && draft.trim() !== "" && slug !== meta.title);

  async function commit() {
    editing = false;
    if (!meta) return;
    if (!changed || !(await app.rename(draft))) draft = meta.title;
  }
</script>

{#if meta?.date}
  <span class="title-field">
    <input
      type="text"
      value={draft}
      oninput={(e) => (draft = e.currentTarget.value)}
      onfocus={() => (editing = true)}
      onblur={commit}
      onkeydown={(e) => {
        if (e.isComposing) return;
        if (e.key === "Enter") {
          e.preventDefault();
          e.currentTarget.blur();
        } else if (e.key === "Escape") {
          e.preventDefault();
          draft = meta.title;
          e.currentTarget.blur();
        }
      }}
      aria-label="Title"
      title="Rename: only the title part of the file name changes"
      autocomplete="off"
      spellcheck="false"
      enterkeyhint="done"
    />
    {#if editing && changed && slug}
      <span class="preview" aria-hidden="true">→ {slug}</span>
    {/if}
  </span>
{:else if meta}
  <span class="title-field fixed" title="Only entries named yyyy-mm-dd-hhmm-title.md can be renamed here">
    {meta.file}
  </span>
{/if}

<style>
  .title-field {
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }

  input {
    flex: 1;
    min-width: 4ch;
    padding: 2px 4px;
    margin-left: -4px;
    border-color: transparent;
    color: var(--color-text);
  }

  input:hover {
    border-color: var(--color-line);
  }

  input:focus {
    border-color: var(--color-accent);
  }

  .preview {
    flex: none;
    color: var(--color-muted);
  }

  .fixed {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
