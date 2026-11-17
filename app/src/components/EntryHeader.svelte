<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { entryTimestamp } from "../lib/journal";
  import { shortcutLabel } from "../lib/shortcuts";
  import type { Mode } from "../lib/types";
  import EntryMenu from "./EntryMenu.svelte";
  import Segmented from "./Segmented.svelte";
  import TitleField from "./TitleField.svelte";

  let { app }: { app: AppState } = $props();

  const MODES = [
    { value: "source", label: "Source" },
    { value: "styled", label: "Styled" },
  ] as const satisfies readonly { value: Mode; label: string }[];

  const stamp = $derived(app.currentMeta && entryTimestamp(app.currentMeta));
</script>

<header>
  {#if stamp}<span class="when">{stamp} ·</span>{/if}
  <TitleField {app} />
  <Segmented
    label="Editor mode"
    options={MODES}
    value={app.mode}
    onchange={(mode) => (app.mode = mode)}
    title={`Toggle Source/Styled (${shortcutLabel("toggleMode", app.mac)})`}
  />
  <EntryMenu {app} />
</header>

<style>
  header {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 40px;
    padding: 6px 10px 6px 16px;
    border-bottom: 1px solid var(--color-line);
  }

  .when {
    flex: none;
    color: var(--color-muted);
    font-variant-numeric: tabular-nums;
  }
</style>
