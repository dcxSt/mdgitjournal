import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { AppState } from "./app.svelte";
import { ConnectFlow } from "./connect.svelte";
import { createMockApi } from "./mock";
import type { DevicePoll } from "./types";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

async function setup(polls: DevicePoll["status"][]) {
  const api = createMockApi({ platform: "macos" });
  const app = new AppState(api);
  await app.init();
  const flow = new ConnectFlow(app);
  const pollTimes: number[] = [];
  const start = Date.now();
  // The mock's own flow needs three polls; this one follows the script, then signs in for real.
  const signIn = async () => {
    await api.githubSetPat("token");
    api.authStatus = async () => ({ signedIn: true, kind: "github_app", login: "octocat" });
    return { status: "complete" as const, login: "octocat" };
  };
  api.githubDevicePoll = async () => {
    pollTimes.push(Date.now() - start);
    const status = polls.shift();
    return status === "complete" ? signIn() : { status: status ?? "pending" };
  };
  api.githubListRepos = async () => [];
  return { api, app, flow, pollTimes };
}

it("shows the code, polls at the given interval, slows down when asked, then lists repos", async () => {
  const { app, flow, pollTimes } = await setup(["pending", "slow_down", "pending", "complete"]);
  await flow.signIn();
  expect(flow.step).toEqual({
    name: "code",
    userCode: "WDJB-MJHT",
    verificationUri: "https://github.com/login/device",
  });

  await vi.advanceTimersByTimeAsync(20_000);
  // Interval 1 s; slow_down adds 5 s.
  expect(pollTimes).toEqual([1000, 2000, 8000, 14000]);
  expect(flow.step).toEqual({ name: "repos", repos: [] });
  expect(app.auth.login).toBe("octocat");
  flow.dispose();
});

it("starts over with a message when the code expires or access is denied", async () => {
  const { flow } = await setup(["expired"]);
  await flow.signIn();
  await vi.advanceTimersByTimeAsync(1000);
  expect(flow.step.name).toBe("start");
  expect(flow.error).toMatch(/expired/);

  const denied = await setup(["denied"]);
  await denied.flow.signIn();
  await vi.advanceTimersByTimeAsync(1000);
  expect(denied.flow.error).toMatch(/denied/);
});

it("stops polling when cancelled", async () => {
  const { flow, pollTimes } = await setup([]);
  await flow.signIn();
  flow.cancel();
  await vi.advanceTimersByTimeAsync(10_000);
  expect(pollTimes).toEqual([]);
  expect(flow.step.name).toBe("start");
});

it("only signs in again when the journal is already connected", async () => {
  const { app, flow } = await setup(["complete"]);
  await app.api.githubSetPat("token");
  await app.connect("octocat/journal");
  await flow.signIn();
  await vi.advanceTimersByTimeAsync(1000);
  expect(flow.step).toEqual({ name: "done", fullName: "octocat/journal", result: null });
});

it("checks the token form before calling the backend", async () => {
  const { api, flow } = await setup([]);
  api.mock.calls.length = 0;
  await flow.usePat("github_pat_x", "not a repo");
  expect(flow.error).toBe("Type the repository as owner/repo.");
  await flow.usePat("  ", "octocat/journal");
  expect(flow.error).toBe("Paste a token first.");
  expect(api.mock.calls).toEqual([]);
});

it("connects with a personal access token", async () => {
  const { app, flow } = await setup([]);
  await flow.usePat(" github_pat_x ", " octocat/journal ");
  expect(flow.error).toBeNull();
  expect(flow.step).toMatchObject({ name: "done", fullName: "octocat/journal" });
  expect(app.sync.connected).toBe(true);
  expect(app.settings.remote?.fullName).toBe("octocat/journal");
});
