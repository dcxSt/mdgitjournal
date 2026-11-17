<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { filterByName } from "../lib/journal";
  import EntryList from "./EntryList.svelte";
  import Icon from "./Icon.svelte";
  import NewEntryPrompt from "./NewEntryPrompt.svelte";
  import SearchField from "./SearchField.svelte";
  import SearchResults from "./SearchResults.svelte";
  import SyncButtons from "./SyncButtons.svelte";

  let { app }: { app: AppState } = $props();

  /** How far (px, after damping) the list must be pulled down to refresh. */
  const PULL_TRIGGER = 56;
  const PULL_MAX = 80;

  const search = $derived(app.search);
  const fullText = $derived(search.mode === "text" && search.query.trim() !== "");
  const filtered = $derived(filterByName(app.entries, search.query));

  let startY: number | null = null;
  let pull = $state(0);
  let refreshing = $state(false);

  function touchstart(e: TouchEvent & { currentTarget: HTMLElement }) {
    startY = e.currentTarget.scrollTop <= 0 && !refreshing ? e.touches[0].clientY : null;
  }

  function touchmove(e: TouchEvent) {
    if (startY === null) return;
    pull = Math.min(Math.max(0, (e.touches[0].clientY - startY) / 2), PULL_MAX);
  }

  async function touchend() {
    if (startY === null) return;
    startY = null;
    const go = pull >= PULL_TRIGGER;
    pull = 0;
    if (!go) return;
    refreshing = true;
    await app.refresh();
    await app.fetch(true);
    refreshing = false;
  }

  const pullLabel = $derived(app.sync.connected ? "check GitHub" : "refresh");
</script>

<header class="bar">
  <button type="button" class="icon" aria-label="Settings" onclick={() => (app.overlay = "settings")}>
    <Icon name="settings" size={18} />
  </button>
  <span class="grow"></span>
  <SyncButtons {app} compact />
</header>

<div class="tools">
  <SearchField {app} />
  {#if app.newPrompt}<NewEntryPrompt {app} />{/if}
</div>

<!-- Pull-to-refresh is a touch shortcut; the Pull button does the same for everyone. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="scroll"
  data-nav
  ontouchstart={touchstart}
  ontouchmove={touchmove}
  ontouchend={touchend}
  ontouchcancel={touchend}
>
  {#if pull > 0 || refreshing}
    <p class="pull" style:height="{refreshing ? PULL_TRIGGER : pull}px" aria-live="polite">
      {#if refreshing}
        <span class="spinner" aria-hidden="true"></span>
      {:else}
        {pull >= PULL_TRIGGER ? `Release to ${pullLabel}` : `Pull to ${pullLabel}`}
      {/if}
    </p>
  {/if}
  {#if fullText}
    <SearchResults {search} selected={null} onopen={(file) => app.openEntry(file)} />
  {:else if !app.entries.length}
    <p class="empty">No entries yet — tap + to start writing.</p>
  {:else if !filtered.length}
    <p class="empty">No entry names match.</p>
  {:else}
    <EntryList entries={filtered} selected={null} onopen={(file) => app.openEntry(file)} />
  {/if}
</div>

<button type="button" class="fab" aria-label="New entry" onclick={() => app.perform("new")}>
  <Icon name="plus" size={22} />
</button>

<style>
  .icon {
    display: grid;
    place-items: center;
    width: var(--touch);
    height: var(--touch);
    padding: 0;
    color: var(--color-muted);
  }

  .tools {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px max(12px, env(safe-area-inset-right)) 6px max(12px, env(safe-area-inset-left));
  }

  .tools :global(input) {
    min-height: 36px;
  }

  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior-y: none;
    padding-bottom: calc(88px + var(--safe-bottom));
    font-size: 15px;
  }

  .pull {
    display: grid;
    place-items: center;
    margin: 0;
    overflow: hidden;
    color: var(--color-muted);
  }

  .empty {
    margin: 32px 16px;
    color: var(--color-muted);
    text-align: center;
  }

  .fab {
    position: absolute;
    right: max(16px, env(safe-area-inset-right));
    bottom: calc(16px + var(--safe-bottom));
    display: grid;
    place-items: center;
    width: 52px;
    height: 52px;
    padding: 0;
    border-radius: 50%;
    background: var(--color-accent);
    color: var(--color-bg);
  }

  .fab:hover:not(:disabled) {
    background: var(--color-accent);
  }
</style>
