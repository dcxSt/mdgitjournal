<!-- Mod-P: open an entry by fuzzy filename. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { entryTimestamp, quickOpen } from "../lib/journal";
  import Sheet from "./Sheet.svelte";

  let { app }: { app: AppState } = $props();

  let query = $state("");
  let index = $state(0);
  let list = $state<HTMLUListElement>();
  const matches = $derived(quickOpen(app.entries, query));

  $effect(() => {
    list?.children[index]?.scrollIntoView({ block: "nearest" });
  });

  function choose(file: string) {
    app.overlay = null;
    void app.openEntry(file);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      index = Math.min(Math.max(index + step, 0), Math.max(matches.length - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const match = matches[index];
      if (match) choose(match.file);
    }
  }
</script>

<Sheet label="Open entry" onclose={() => (app.overlay = null)}>
  <input
    data-autofocus
    type="text"
    role="combobox"
    aria-label="Open entry"
    aria-expanded="true"
    aria-controls="quick-open"
    aria-activedescendant={matches.length ? `quick-open-${index}` : undefined}
    placeholder="Open entry…"
    autocomplete="off"
    autocapitalize="off"
    spellcheck="false"
    value={query}
    oninput={(e) => {
      query = e.currentTarget.value;
      index = 0;
    }}
    {onkeydown}
  />
  <ul id="quick-open" role="listbox" aria-label="Entries" bind:this={list}>
    {#each matches as entry, i (entry.file)}
      <li id="quick-open-{i}" role="option" aria-selected={i === index}>
        <button type="button" tabindex="-1" onclick={() => choose(entry.file)} onpointermove={() => (index = i)}>
          <span class="title">{entry.title}</span>
          <span class="when">{entryTimestamp(entry) ?? "Other"}</span>
        </button>
      </li>
    {:else}
      <li class="none">No entry names match.</li>
    {/each}
  </ul>
</Sheet>

<style>
  input {
    width: 100%;
    margin-bottom: 8px;
  }

  ul {
    max-height: 50vh;
    margin: 0 -8px;
    padding: 0;
    overflow-y: auto;
    list-style: none;
  }

  button {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
    padding: 5px 8px;
    text-align: left;
  }

  [aria-selected="true"] button {
    background: var(--color-selected);
  }

  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .when {
    flex: none;
    color: var(--color-muted);
    font-variant-numeric: tabular-nums;
  }

  .none {
    padding: 5px 8px;
    color: var(--color-muted);
  }
</style>
