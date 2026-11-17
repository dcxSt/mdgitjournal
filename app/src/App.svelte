<script lang="ts">
  import { onMount } from "svelte";
  import CommitSheet from "./components/CommitSheet.svelte";
  import Desktop from "./components/Desktop.svelte";
  import Mobile from "./components/Mobile.svelte";
  import Notices from "./components/Notices.svelte";
  import QuickOpen from "./components/QuickOpen.svelte";
  import SettingsSheet from "./components/SettingsSheet.svelte";
  import type { AppState } from "./lib/app.svelte";
  import { shortcutAction } from "./lib/shortcuts";

  let { app }: { app: AppState } = $props();

  onMount(() => {
    const scheme = matchMedia("(prefers-color-scheme: dark)");
    const onScheme = () => (app.systemDark = scheme.matches);
    onScheme();
    scheme.addEventListener("change", onScheme);
    app.width = window.innerWidth;
    void app.init();
    return () => {
      scheme.removeEventListener("change", onScheme);
      app.dispose();
    };
  });

  $effect(() => {
    const root = document.documentElement;
    if (app.settings.theme === "system") delete root.dataset.theme;
    else root.dataset.theme = app.settings.theme;
  });

  // Capture phase: app shortcuts win over the editor's own keymaps.
  function onkeydown(e: KeyboardEvent) {
    if (!app.ready) return;
    const action = shortcutAction(e, app.mac);
    if (!action) return;
    e.preventDefault();
    e.stopPropagation();
    app.perform(action);
  }
</script>

<svelte:window onkeydowncapture={onkeydown} onresize={() => (app.width = window.innerWidth)} />

{#if app.initError}
  <p class="fatal" role="alert">Couldn't open the journal: {app.initError}</p>
{:else if app.ready}
  {#if app.mobile}
    <Mobile {app} />
  {:else}
    <Desktop {app} />
  {/if}
  <Notices {app} />
  {#if app.overlay === "commit"}
    <CommitSheet {app} />
  {:else if app.overlay === "settings"}
    <SettingsSheet {app} />
  {:else if app.overlay === "quickOpen"}
    <QuickOpen {app} />
  {/if}
{/if}

<style>
  .fatal {
    max-width: 40em;
    margin: 20vh auto 0;
    padding: 0 24px;
    color: var(--color-danger);
  }
</style>
