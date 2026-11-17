// Helpers for the settings sheet.

import type { Platform } from "./types";

/** Parses a whole number in [min, max]; null for anything else, so the field reverts. */
export function parseBoundedInt(text: string, min: number, max: number): number | null {
  const trimmed = text.trim();
  if (!/^\d+$/.test(trimmed)) return null;
  const value = Number(trimmed);
  return value >= min && value <= max ? value : null;
}

/** Where the GitHub token is kept, in words, from `app_info.tokenBackend`. */
export function tokenBackendLabel(backend: string, platform: Platform): string {
  switch (backend) {
    case "keychain":
      return platform === "ios" ? "the iOS Keychain" : "the macOS Keychain";
    case "secret-service":
      return "the Secret Service keyring (GNOME Keyring, KWallet)";
    case "file":
      return "a private file (credentials.json, readable only by you)";
    default:
      return backend;
  }
}
