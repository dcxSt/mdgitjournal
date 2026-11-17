// Placeholder values used until the backend has answered.

import type { AuthStatus, Settings, SyncView } from "./types";

export const DEFAULT_SETTINGS: Settings = {
  vim: false,
  modeDesktop: "source",
  modeMobile: "styled",
  fetchIntervalMin: 5,
  imageMaxEdge: 1024,
  imageQuality: 60,
  theme: "system",
  authorName: null,
  authorEmail: null,
  remote: null,
};

export const EMPTY_VIEW: SyncView = {
  connected: false,
  repo: null,
  branch: "main",
  ahead: 0,
  behind: 0,
  changes: [],
  lastFetch: null,
  error: null,
};

export const SIGNED_OUT: AuthStatus = { signedIn: false, kind: null, login: null };
