<script lang="ts">
  import { dayOfMonth, groupByMonth } from "../lib/journal";
  import type { EntryMeta } from "../lib/types";

  let {
    entries,
    selected,
    onopen,
  }: { entries: EntryMeta[]; selected: string | null; onopen: (file: string) => void } = $props();

  const groups = $derived(groupByMonth(entries));
</script>

{#each groups as group (group.key)}
  <section>
    <h2>{group.label}</h2>
    <ul>
      {#each group.entries as entry (entry.file)}
        <li>
          <button
            type="button"
            data-row
            class:selected={entry.file === selected}
            aria-current={entry.file === selected ? "page" : undefined}
            title={entry.file}
            onclick={() => onopen(entry.file)}
          >
            <span class="day">{dayOfMonth(entry)}</span>
            <span class="title">{entry.title}</span>
          </button>
        </li>
      {/each}
    </ul>
  </section>
{/each}

<style>
  section + section {
    margin-top: 10px;
  }

  h2 {
    margin: 0;
    padding: 4px 12px;
    font-size: 11px;
    font-weight: 500;
    color: var(--color-muted);
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  button {
    display: grid;
    grid-template-columns: 2ch 1fr;
    gap: 10px;
    width: 100%;
    min-height: var(--row-height, 26px);
    padding: 3px 12px;
    border-radius: 0;
    text-align: left;
    align-items: center;
  }

  button.selected {
    background: var(--color-selected);
  }

  /* Inside the row: the scrolling list would clip a ring drawn outside it. */
  button:focus-visible {
    outline-offset: -2px;
  }

  .day {
    color: var(--color-muted);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
