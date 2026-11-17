import { describe, expect, it } from "vitest";
import { EMPTY_VIEW } from "./defaults";
import { commitButton, commitMessageValid, isPushOnly, pullButton } from "./sync";
import type { FileChange, SyncView } from "./types";

const change: FileChange = { path: "2026-11-16-1402-morning-run.md", kind: "modified" };
const view = (patch: Partial<SyncView>): SyncView => ({ ...EMPTY_VIEW, connected: true, repo: "o/r", ...patch });

describe("pull button", () => {
  it("is grey with no badge when up to date", () => {
    expect(pullButton(view({}), [])).toMatchObject({ accent: false, badge: null, busy: false });
  });

  it("is accented with the number of new commits when behind", () => {
    expect(pullButton(view({ behind: 2 }), [])).toMatchObject({
      accent: true,
      badge: 2,
      title: "2 new commits on GitHub",
    });
  });

  it("spins while pulling or fetching", () => {
    expect(pullButton(view({}), ["pull"]).busy).toBe(true);
    expect(pullButton(view({}), ["fetch"]).busy).toBe(true);
    expect(pullButton(view({}), ["commit"]).busy).toBe(false);
  });
});

describe("commit button", () => {
  it("is a disabled, grey Commit & Push with nothing to do", () => {
    expect(commitButton(view({}), [])).toMatchObject({
      label: "Commit & Push",
      accent: false,
      badge: null,
      disabled: true,
      hint: null,
    });
  });

  it("is highlighted with the number of changed files", () => {
    const changes = [change, { path: "b.md", kind: "added" } as const];
    expect(commitButton(view({ changes }), [])).toMatchObject({ accent: true, badge: 2, disabled: false });
  });

  it("is enabled for local commits waiting to be pushed", () => {
    expect(commitButton(view({ ahead: 1 }), [])).toMatchObject({ accent: true, badge: null, disabled: false });
  });

  it("is disabled with the hint “Pull first” when GitHub is ahead", () => {
    const button = commitButton(view({ behind: 1, ahead: 1, changes: [change] }), []);
    expect(button).toMatchObject({ disabled: true, accent: false, hint: "Pull first", badge: 1 });
  });

  it("is just Commit, never blocked by the remote, when not connected", () => {
    const local = { ...EMPTY_VIEW, changes: [change], behind: 3, ahead: 3 };
    expect(commitButton(local, [])).toMatchObject({ label: "Commit", disabled: false, hint: null, badge: 1 });
    expect(commitButton({ ...EMPTY_VIEW, ahead: 2 }, []).disabled).toBe(true);
  });

  it("spins while committing", () => {
    expect(commitButton(view({ changes: [change] }), ["commit"]).busy).toBe(true);
  });
});

describe("commit message", () => {
  it("is required", () => {
    expect(commitMessageValid("")).toBe(false);
    expect(commitMessageValid(" \n\t ")).toBe(false);
    expect(commitMessageValid("Run notes")).toBe(true);
  });

  it("may be left out only when there is nothing to commit but commits to push", () => {
    expect(isPushOnly(view({ ahead: 1 }))).toBe(true);
    expect(isPushOnly(view({ ahead: 1, changes: [change] }))).toBe(false);
    expect(isPushOnly(view({}))).toBe(false);
    expect(isPushOnly({ ...EMPTY_VIEW, ahead: 1 })).toBe(false);
  });
});
