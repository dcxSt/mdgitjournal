<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { filterByName } from "../lib/journal";
  import { shortcutLabel } from "../lib/shortcuts";
  import EntryList from "./EntryList.svelte";
  import NewEntryPrompt from "./NewEntryPrompt.svelte";
  import SearchField from "./SearchField.svelte";
  import SearchResults from "./SearchResults.svelte";

  let { app }: { app: AppState } = $props();

  const search = $derived(app.search);
  const fullText = $derived(search.mode === "text" && search.query.trim() !== "");
  const filtered = $derived(filterByName(app.entries, search.query));

  /** Up/Down move between rows; Up from the first row goes back to the search field. */
  function onkeydown(e: KeyboardEvent & { currentTarget: HTMLElement }) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const target = e.target as HTMLElement;
    if (!target.matches("[data-row]")) return;
    const rows = [...e.currentTarget.querySelectorAll<HTMLElement>("[data-row]")];
    const next = rows[rows.indexOf(target) + (e.key === "ArrowDown" ? 1 : -1)];
    e.preventDefault();
    if (next) next.focus();
    else if (e.key === "ArrowUp") app.focusSearch = true;
  }
</script>

<!-- The handler only sees arrow keys bubbling up from the row buttons. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<aside data-nav {onkeydown}>
  <div class="top">
    <SearchField {app} />
    <button type="button" class="new" onclick={() => app.perform("new")}>
      <span>+ New</span>
      <kbd>{shortcutLabel("new", app.mac)}</kbd>
    </button>
    {#if app.newPrompt}<NewEntryPrompt {app} />{/if}
  </div>
  <nav aria-label="Entries">
    {#if fullText}
      <SearchResults {search} selected={app.current.file} onopen={(file) => app.openEntry(file)} />
    {:else if !app.entries.length}
      <p class="note">No entries yet.</p>
    {:else if !filtered.length}
      <p class="note">No entry names match.</p>
    {:else}
      <EntryList entries={filtered} selected={app.current.file} onopen={(file) => app.openEntry(file)} />
    {/if}
  </nav>
</aside>

<style>
  aside {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--color-line);
  }

  .top {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 10px 8px;
  }

  .new {
    display: flex;
    justify-content: space-between;
    padding: 4px 6px;
    color: var(--color-muted);
    text-align: left;
  }

  kbd {
    font: inherit;
    color: var(--color-syntax);
  }

  nav {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding-bottom: 12px;
  }

  .note {
    margin: 8px 12px;
    color: var(--color-muted);
  }
</style>
