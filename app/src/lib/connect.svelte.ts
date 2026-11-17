// Settings → Connect GitHub (PLAN §4): device-flow sign-in or a personal access token, then
// choose a repo, then connect.

import { toApiError } from "./api";
import type { AppState } from "./app.svelte";
import type { DevicePoll, GithubUrls, PullResult, RepoInfo } from "./types";

export type ConnectStep =
  | { name: "start" }
  | { name: "code"; userCode: string; verificationUri: string }
  | { name: "repos"; repos: RepoInfo[] | null }
  | { name: "connecting"; fullName: string }
  /** `result` is null when an already connected journal only signed in again. */
  | { name: "done"; fullName: string; result: PullResult | null };

/** GitHub asks clients to add 5 s to the polling interval on `slow_down`. */
const SLOW_DOWN_S = 5;
const RETRY_S = 10;
const FULL_NAME = /^[\w.-]+\/[\w.-]+$/;

export class ConnectFlow {
  step = $state.raw<ConnectStep>({ name: "start" });
  error = $state<string | null>(null);
  busy = $state(false);
  urls = $state.raw<GithubUrls | null>(null);

  #timer: ReturnType<typeof setTimeout> | undefined;
  #interval = 5;
  #expiresAt = 0;

  constructor(private readonly app: AppState) {}

  /** Already signed in (e.g. an earlier connect failed): go straight to choosing a repo. */
  async begin(): Promise<void> {
    if (this.app.auth.signedIn) await this.showRepos();
  }

  dispose(): void {
    clearTimeout(this.#timer);
  }

  async signIn(): Promise<void> {
    this.error = null;
    this.busy = true;
    try {
      const code = await this.app.api.githubDeviceStart();
      this.#interval = Math.max(1, code.interval);
      this.#expiresAt = Date.now() + code.expiresIn * 1000;
      this.step = { name: "code", userCode: code.userCode, verificationUri: code.verificationUri };
      this.#schedulePoll(this.#interval);
    } catch (e) {
      this.error = toApiError(e).message;
    } finally {
      this.busy = false;
    }
  }

  cancel(): void {
    clearTimeout(this.#timer);
    this.error = null;
    this.step = { name: "start" };
  }

  /** Forgets the GitHub sign-in on this device (the journal is not connected yet). */
  async signOut(): Promise<void> {
    await this.app.disconnect(true);
    this.cancel();
  }

  async showRepos(): Promise<void> {
    this.error = null;
    this.step = { name: "repos", repos: null };
    try {
      const [repos, urls] = await Promise.all([this.app.api.githubListRepos(), this.urls ?? this.app.api.githubUrls()]);
      this.urls = urls;
      if (this.step.name === "repos") this.step = { name: "repos", repos };
    } catch (e) {
      this.error = toApiError(e).message;
      if (this.step.name === "repos") this.step = { name: "repos", repos: [] };
    }
  }

  /** Personal access token path. The token may be blank if one is already stored. */
  async usePat(token: string, fullName: string): Promise<void> {
    const name = fullName.trim();
    this.error = null;
    if (!FULL_NAME.test(name)) {
      this.error = "Type the repository as owner/repo.";
      return;
    }
    if (!token.trim() && this.app.auth.kind !== "pat") {
      this.error = "Paste a token first.";
      return;
    }
    this.busy = true;
    try {
      if (token.trim()) {
        await this.app.api.githubSetPat(token.trim());
        await this.app.refreshAuth();
      }
    } catch (e) {
      this.error = toApiError(e).message;
      return;
    } finally {
      this.busy = false;
    }
    await this.connect(name);
  }

  async connect(fullName: string): Promise<void> {
    const back = this.step;
    this.error = null;
    this.step = { name: "connecting", fullName };
    try {
      const result = await this.app.connect(fullName);
      this.step = { name: "done", fullName, result };
    } catch (e) {
      this.error = toApiError(e).message;
      this.step = back;
    }
  }

  async openUrl(url: string): Promise<void> {
    try {
      await this.app.api.openUrl(url);
    } catch (e) {
      this.error = toApiError(e).message;
    }
  }

  #schedulePoll(seconds: number): void {
    clearTimeout(this.#timer);
    this.#timer = setTimeout(() => void this.#poll(), seconds * 1000);
  }

  async #poll(): Promise<void> {
    if (this.step.name !== "code") return;
    if (Date.now() > this.#expiresAt) {
      this.#restart("The code expired. Start again to get a new one.");
      return;
    }
    let poll: DevicePoll;
    try {
      poll = await this.app.api.githubDevicePoll();
    } catch (e) {
      const error = toApiError(e);
      if (error.kind !== "network") {
        this.#restart(error.message);
        return;
      }
      this.error = `${error.message} Retrying…`;
      this.#schedulePoll(RETRY_S);
      return;
    }
    if (this.step.name !== "code") return;
    this.error = null;
    switch (poll.status) {
      case "pending":
        this.#schedulePoll(this.#interval);
        break;
      case "slow_down":
        this.#interval += SLOW_DOWN_S;
        this.#schedulePoll(this.#interval);
        break;
      case "complete":
        try {
          await this.app.refreshAuth();
        } catch (e) {
          this.#restart(toApiError(e).message);
          return;
        }
        if (this.app.sync.connected) this.step = { name: "done", fullName: this.app.sync.repo ?? "", result: null };
        else await this.showRepos();
        break;
      case "expired":
        this.#restart("The code expired. Start again to get a new one.");
        break;
      case "denied":
        this.#restart("Access was denied on GitHub.");
        break;
    }
  }

  #restart(error: string): void {
    this.step = { name: "start" };
    this.error = error;
  }
}
