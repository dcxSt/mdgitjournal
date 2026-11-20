<script lang="ts">
  import { onMount } from "svelte";
  import AbortMergeSheet from "./components/AbortMergeSheet.svelte";
  import CommitSheet from "./components/CommitSheet.svelte";
  import DeleteSheet from "./components/DeleteSheet.svelte";
  import Desktop from "./components/Desktop.svelte";
  import MergeView from "./components/MergeView.svelte";
  import Mobile from "./components/Mobile.svelte";
  import Notices from "./components/Notices.svelte";
  import QuickOpen from "./components/QuickOpen.svelte";
  import SettingsSheet from "./components/SettingsSheet.svelte";
  import SyncReview from "./components/SyncReview.svelte";
  import SyncSummary from "./components/SyncSummary.svelte";
  import type { AppState } from "./lib/app.svelte";
  import { shortcutAction, yieldsToVim } from "./lib/shortcuts";

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

  // Phone-sized sheets and fields follow the layout, not the window width (styles/app.css).
  $effect(() => {
    document.documentElement.dataset.layout = app.mobile ? "phone" : "desktop";
  });

  // Capture phase: app shortcuts win over the editor's own keymaps, except the few Vim needs.
  function onkeydown(e: KeyboardEvent) {
    if (!app.ready) return;
    const action = shortcutAction(e, app.mac);
    if (!action || yieldsToVim(action, e.target, app.mac, app.settings.vim && !app.mobile)) return;
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
  {:else if app.overlay === "delete"}
    <DeleteSheet {app} />
  {:else if app.overlay === "summary" && app.summary}
    <SyncSummary {app} summary={app.summary} />
  {:else if app.overlay === "abortMerge"}
    <AbortMergeSheet {app} />
  {/if}
  <!-- The review stays open under the merge view, as it was. -->
  {#if app.review && (app.overlay === "review" || app.overlay === "merge")}
    {#key app.review}
      <SyncReview {app} review={app.review} />
    {/key}
    {#if app.overlay === "merge" && app.review.merging}
      <MergeView {app} review={app.review} />
    {/if}
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
