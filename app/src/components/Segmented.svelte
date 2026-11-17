<script lang="ts" generics="T extends string">
  let {
    label,
    options,
    value,
    onchange,
    title,
  }: {
    label: string;
    options: readonly { value: T; label: string }[];
    value: T;
    onchange: (value: T) => void;
    title?: string;
  } = $props();
</script>

<div class="segmented" role="group" aria-label={label} {title}>
  {#each options as option (option.value)}
    <button type="button" aria-pressed={option.value === value} onclick={() => onchange(option.value)}>
      {option.label}
    </button>
  {/each}
</div>

<style>
  .segmented {
    display: inline-flex;
    flex: none;
    padding: 1px;
    border: 1px solid var(--color-line);
    border-radius: var(--radius);
  }

  button {
    padding: 2px 8px;
    border-radius: 4px;
    color: var(--color-muted);
  }

  button[aria-pressed="true"] {
    background: var(--color-selected);
    color: var(--color-text);
  }
</style>
