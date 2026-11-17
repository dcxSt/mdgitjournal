<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { parseBoundedInt, tokenBackendLabel } from "../lib/settings";
  import type { Mode, Settings, Theme } from "../lib/types";
  import ConnectGitHub from "./ConnectGitHub.svelte";
  import Icon from "./Icon.svelte";
  import Segmented from "./Segmented.svelte";
  import Sheet from "./Sheet.svelte";

  let { app }: { app: AppState } = $props();

  const MODES = [
    { value: "source", label: "Source" },
    { value: "styled", label: "Styled" },
  ] as const satisfies readonly { value: Mode; label: string }[];
  const THEMES = [
    { value: "system", label: "System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ] as const satisfies readonly { value: Theme; label: string }[];
  const LIMITS = {
    fetchIntervalMin: [0, 1440],
    imageMaxEdge: [256, 4096],
    imageQuality: [1, 100],
  } as const;

  let connecting = $state(false);
  let confirmDisconnect = $state(false);
  let notices = $state<string | null>(null);

  const settings = $derived(app.settings);
  const info = $derived(app.info);
  const modeKey = $derived(app.mobile ? "modeMobile" : "modeDesktop");
  const github = $derived(app.auth.signedIn && app.auth.kind === "github_app");
  const signedInAs = $derived(app.auth.login ? `, signed in as @${app.auth.login}` : "");

  function setMode(mode: Mode) {
    app.mode = mode;
    void app.saveSettings({ [modeKey]: mode });
  }

  function saveNumber(key: keyof typeof LIMITS, input: HTMLInputElement) {
    const [min, max] = LIMITS[key];
    const value = parseBoundedInt(input.value, min, max);
    if (value === null) input.value = String(settings[key]);
    else if (value !== settings[key]) void app.saveSettings({ [key]: value });
  }

  function saveText(key: "authorName" | "authorEmail", input: HTMLInputElement) {
    const value = input.value.trim() || null;
    if (value !== settings[key]) void app.saveSettings({ [key]: value } satisfies Partial<Settings>);
  }

  async function reveal() {
    if (!info) return;
    try {
      await app.api.revealPath(info.journalDir);
    } catch (e) {
      app.showError(e);
    }
  }

  /** Loaded when first shown: some 500 KB of licence text. */
  async function showNotices(details: HTMLDetailsElement) {
    if (!details.open || notices !== null) return;
    try {
      notices = await app.api.thirdPartyNotices();
    } catch (e) {
      details.open = false;
      app.showError(e);
    }
  }

  async function disconnect(signOut: boolean) {
    confirmDisconnect = false;
    await app.disconnect(signOut);
  }
</script>

<Sheet label="Settings" wide onclose={() => app.closeOverlay()}>
  <div class="head">
    <h2>Settings</h2>
    <button type="button" class="close" aria-label="Close settings" onclick={() => app.closeOverlay()}>
      <Icon name="close" />
    </button>
  </div>

  <section>
    <h3>Writing</h3>
    <div class="row">
      <span>Opens in</span>
      <Segmented label="Default view" options={MODES} value={settings[modeKey]} onchange={setMode} />
    </div>
    {#if !app.mobile}
      <label class="row">
        <span>Vim keys</span>
        <input
          type="checkbox"
          checked={settings.vim}
          onchange={(e) => app.saveSettings({ vim: e.currentTarget.checked })}
        />
      </label>
      {#if settings.vim && !app.mac}
        <p class="hint">
          In the text, Ctrl+E, Ctrl+N and Ctrl+P are Vim's; outside it they are the app's shortcuts.
        </p>
      {/if}
    {/if}
    <div class="row">
      <span>Theme</span>
      <Segmented
        label="Theme"
        options={THEMES}
        value={settings.theme}
        onchange={(theme) => app.saveSettings({ theme })}
      />
    </div>
  </section>

  <section>
    <h3>Images</h3>
    <label class="row">
      <span>Longest side</span>
      <span class="unit">
        <input
          type="number"
          inputmode="numeric"
          min="256"
          max="4096"
          value={settings.imageMaxEdge}
          onchange={(e) => saveNumber("imageMaxEdge", e.currentTarget)}
        />
        px
      </span>
    </label>
    <label class="row">
      <span>JPEG quality</span>
      <span class="unit">
        <input
          type="number"
          inputmode="numeric"
          min="1"
          max="100"
          value={settings.imageQuality}
          onchange={(e) => saveNumber("imageQuality", e.currentTarget)}
        />
        of 100
      </span>
    </label>
  </section>

  <section>
    <h3>Sync</h3>
    <label class="row">
      <span>Check GitHub every</span>
      <span class="unit">
        <input
          type="number"
          inputmode="numeric"
          min="0"
          max="1440"
          value={settings.fetchIntervalMin}
          onchange={(e) => saveNumber("fetchIntervalMin", e.currentTarget)}
        />
        min
      </span>
    </label>
    <p class="hint">
      0 turns the timer off. The app still checks on launch and when it comes back to the front. It never commits or
      pushes by itself.
    </p>
    <label class="row">
      <span>Author name</span>
      <input
        type="text"
        value={settings.authorName ?? ""}
        placeholder={github ? "Your GitHub name" : "mdgitjournal"}
        autocomplete="name"
        onchange={(e) => saveText("authorName", e.currentTarget)}
      />
    </label>
    <label class="row">
      <span>Author email</span>
      <input
        type="email"
        value={settings.authorEmail ?? ""}
        placeholder={github ? "Your GitHub no-reply address" : "mdgitjournal@localhost"}
        autocomplete="email"
        autocapitalize="off"
        spellcheck="false"
        onchange={(e) => saveText("authorEmail", e.currentTarget)}
      />
    </label>
  </section>

  <section>
    <h3>GitHub</h3>
    {#if connecting}
      <ConnectGitHub {app} ondone={() => (connecting = false)} />
    {:else if app.sync.connected}
      <p>
        Syncing with <strong>{app.sync.repo}</strong> · {app.sync.branch}{signedInAs}.
      </p>
      {#if !app.auth.signedIn}
        <p class="error">Signed out of GitHub. Sign in again to keep syncing.</p>
        <div class="actions">
          <button type="button" class="button primary" onclick={() => (connecting = true)}>Sign in again</button>
        </div>
      {/if}
      {#if confirmDisconnect}
        <p>Remove the GitHub remote from this journal? Your files and their history stay on this device.</p>
        <div class="actions">
          <button type="button" class="button" onclick={() => (confirmDisconnect = false)}>Cancel</button>
          <button type="button" class="button danger" onclick={() => disconnect(false)}>Disconnect</button>
          {#if app.auth.signedIn}
            <button
              type="button"
              class="button danger"
              onclick={() => disconnect(true)}
            >
              Disconnect and sign out
            </button>
          {/if}
        </div>
      {:else}
        <div class="actions">
          <button type="button" class="button" onclick={() => (confirmDisconnect = true)}>Disconnect…</button>
        </div>
      {/if}
    {:else}
      <p class="muted">
        This journal is local{#if app.auth.login} (signed in as @{app.auth.login}){/if}. Connect one GitHub repository to
        sync it between devices.
      </p>
      <div class="actions">
        <button type="button" class="button primary" onclick={() => (connecting = true)}>Connect GitHub</button>
      </div>
    {/if}
    {#if info}
      <p class="hint">The GitHub token is kept in {tokenBackendLabel(info.tokenBackend, info.platform)}.</p>
    {/if}
  </section>

  {#if info}
    <section>
      <h3>Journal folder</h3>
      <div class="row">
        <input class="path" type="text" readonly value={info.journalDir} aria-label="Journal folder" />
        {#if info.platform !== "ios"}
          <button type="button" class="button" onclick={reveal}>Reveal</button>
        {/if}
      </div>
      {#if info.platform === "ios"}
        <p class="hint">Also in the Files app, under On My iPhone → mdgitjournal.</p>
      {:else}
        <p class="hint">An ordinary git repository: the git command line works on it too.</p>
      {/if}
    </section>
    <section class="about">
      <h3>About</h3>
      <p>mdgitjournal {info.version} · MIT licence · © 2026 Stephen Fay</p>
      <p class="hint">This software is based in part on the work of the Independent JPEG Group.</p>
      <details ontoggle={(e) => showNotices(e.currentTarget)}>
        <summary>Third-party notices</summary>
        {#if notices !== null}
          <pre class="notices">{notices}</pre>
        {/if}
      </details>
    </section>
  {/if}
</Sheet>

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 4px;
  }

  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 500;
  }

  .close {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    color: var(--color-muted);
  }

  section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 14px 0;
    border-bottom: 1px solid var(--color-line);
  }

  h3 {
    margin: 0;
    font-size: 11px;
    font-weight: 500;
    color: var(--color-muted);
  }

  p {
    margin: 0;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    min-height: 28px;
  }

  .row > input[type="text"],
  .row > input[type="email"] {
    width: min(280px, 60%);
  }

  .unit {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--color-muted);
  }

  input[type="number"] {
    width: 76px;
    text-align: right;
    color: var(--color-text);
  }

  .path {
    flex: 1;
    min-width: 0;
    font-family: var(--font-source);
    font-size: 12px;
    color: var(--color-muted);
  }

  .hint {
    color: var(--color-muted);
    font-size: 12px;
  }

  .error {
    color: var(--color-danger);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .about {
    border-bottom: none;
  }

  summary {
    color: var(--color-accent);
    cursor: default;
  }

  .notices {
    max-height: 50vh;
    margin: 8px 0 0;
    overflow: auto;
    font-family: var(--font-source);
    font-size: 11px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    color: var(--color-muted);
  }

  :global([data-layout="phone"]) .row,
  :global([data-layout="phone"]) .button {
    min-height: var(--touch);
  }

  :global([data-layout="phone"]) .close {
    width: var(--touch);
    height: var(--touch);
    margin-right: -8px;
  }
</style>
