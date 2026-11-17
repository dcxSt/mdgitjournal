<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { shortcutLabel } from "../lib/shortcuts";
  import Icon from "./Icon.svelte";

  let { app }: { app: AppState } = $props();

  let input = $state<HTMLInputElement>();
  const search = $derived(app.search);
  const text = $derived(search.mode === "text");

  $effect(() => {
    if (input && app.focusSearch) {
      app.focusSearch = false;
      input.focus();
      input.select();
    }
  });

  /** First result row in the surrounding list (`data-nav` marks the container). */
  const firstRow = () => input?.closest("[data-nav]")?.querySelector<HTMLButtonElement>("[data-row]");

  function onkeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "Escape") {
      e.preventDefault();
      if (search.query) search.setQuery("");
      else if (text) search.setMode("name");
      else if (!app.mobile && app.current.file !== null) app.focusEditor = true;
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      firstRow()?.focus();
    } else if (e.key === "Enter") {
      e.preventDefault();
      firstRow()?.click();
    }
  }
</script>

<div class="field" class:text>
  <Icon name="search" size={13} />
  <input
    bind:this={input}
    type="search"
    value={search.query}
    oninput={(e) => search.setQuery(e.currentTarget.value)}
    {onkeydown}
    placeholder={text ? "Search inside entries" : "Filter by name"}
    aria-label={text ? "Search inside entries" : "Filter entries by name"}
    autocomplete="off"
    autocapitalize="off"
    spellcheck="false"
  />
  <button
    type="button"
    class="mode"
    aria-pressed={text}
    title={`Search inside entries (${shortcutLabel("search", app.mac)})`}
    onclick={() => search.setMode(text ? "name" : "text")}
  >
    Text
  </button>
</div>

<style>
  .field {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 4px 0 8px;
    border: 1px solid var(--color-line);
    border-radius: var(--radius);
    color: var(--color-muted);
  }

  .field:focus-within {
    border-color: var(--color-accent);
  }

  input {
    flex: 1;
    min-width: 0;
    padding: 5px 0;
    border: none;
    color: var(--color-text);
  }

  input::-webkit-search-cancel-button {
    display: none;
  }

  .mode {
    padding: 1px 6px;
    font-size: 11px;
    color: var(--color-muted);
  }

  .mode[aria-pressed="true"] {
    color: var(--color-accent);
    background: var(--color-accent-soft);
  }
</style>
