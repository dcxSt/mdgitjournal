<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { shortcutLabel } from "../lib/shortcuts";
  import { plural } from "../lib/sync";
  import Icon from "./Icon.svelte";
  import SaveState from "./SaveState.svelte";
  import SyncButtons from "./SyncButtons.svelte";

  let { app }: { app: AppState } = $props();

  const where = $derived(
    app.sync.connected ? `${app.sync.repo ?? "GitHub"} · ${app.sync.branch}` : "Local journal",
  );
  const settings = $derived(`Settings (${shortcutLabel("settings", app.mac)})`);
</script>

<footer>
  <button type="button" class="where" title={settings} onclick={() => (app.overlay = "settings")}>
    {where}
  </button>
  <SyncButtons {app} />
  <span class="grow"></span>
  {#if app.current.file !== null}
    <span class="words">{plural(app.current.words, "word")}</span>
    <SaveState current={app.current} />
  {/if}
  <button type="button" class="icon" aria-label="Settings" title={settings} onclick={() => (app.overlay = "settings")}>
    <Icon name="settings" />
  </button>
</footer>

<style>
  footer {
    grid-column: 1 / -1;
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    height: 32px;
    padding: 0 6px;
    border-top: 1px solid var(--color-line);
    color: var(--color-muted);
    white-space: nowrap;
  }

  .where {
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--color-muted);
  }

  .grow {
    flex: 1;
  }

  .words {
    font-variant-numeric: tabular-nums;
  }

  .icon {
    display: grid;
    place-items: center;
    width: 26px;
    height: 24px;
    padding: 0;
    color: var(--color-muted);
  }
</style>
