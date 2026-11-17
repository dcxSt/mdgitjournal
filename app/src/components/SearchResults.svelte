<script lang="ts">
  import { groupHits, snippetParts } from "../lib/journal";
  import type { Search } from "../lib/search.svelte";

  let {
    search,
    selected,
    onopen,
  }: { search: Search; selected: string | null; onopen: (file: string) => void } = $props();

  const files = $derived(groupHits(search.hits));
  /** Snippets shown per file; the entry itself has the rest. */
  const LINES = 3;
</script>

{#if search.error}
  <p class="note failed">{search.error}</p>
{:else if !files.length}
  <p class="note">{search.pending ? "Searching…" : "No matches."}</p>
{:else}
  <ul aria-busy={search.pending}>
    {#each files as hit (hit.file)}
      <li>
        <button
          type="button"
          data-row
          class:selected={hit.file === selected}
          title={hit.file}
          onclick={() => onopen(hit.file)}
        >
          <span class="title">{hit.title}</span>
          {#each hit.lines.slice(0, LINES) as line}
            {@const part = snippetParts(line)}
            <span class="snippet">{part.before}<mark>{part.match}</mark>{part.after}</span>
          {/each}
        </button>
      </li>
    {/each}
  </ul>
{/if}

<style>
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  button {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    padding: 6px 12px;
    border-radius: 0;
    text-align: left;
  }

  button.selected {
    background: var(--color-selected);
  }

  .title,
  .snippet {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }

  .snippet {
    color: var(--color-muted);
  }

  mark {
    background: var(--color-accent-soft);
    color: var(--color-text);
    border-radius: 2px;
  }

  .note {
    margin: 8px 12px;
    color: var(--color-muted);
  }

  .failed {
    color: var(--color-danger);
  }
</style>
