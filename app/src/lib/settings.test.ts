import { expect, it } from "vitest";
import { parseBoundedInt, tokenBackendLabel } from "./settings";

it("accepts only whole numbers in range", () => {
  expect(parseBoundedInt(" 5 ", 0, 1440)).toBe(5);
  expect(parseBoundedInt("0", 0, 1440)).toBe(0);
  expect(parseBoundedInt("1441", 0, 1440)).toBeNull();
  expect(parseBoundedInt("2.5", 0, 10)).toBeNull();
  expect(parseBoundedInt("-1", 0, 10)).toBeNull();
  expect(parseBoundedInt("", 0, 10)).toBeNull();
  expect(parseBoundedInt("1e3", 0, 4096)).toBeNull();
});

it("names the token store", () => {
  expect(tokenBackendLabel("keychain", "macos")).toBe("the macOS Keychain");
  expect(tokenBackendLabel("keychain", "ios")).toBe("the iOS Keychain");
  expect(tokenBackendLabel("something-new", "linux")).toBe("something-new");
});
