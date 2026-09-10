import assert from "node:assert/strict";
import test from "node:test";

import {
  daemonSearchPath,
  daemonWorkingDirectory,
  loginShellPath,
  markedPath,
} from "../src/daemon-process.js";

const launchdPath = "/usr/bin:/bin:/usr/sbin:/sbin";

const searchPath = (overrides: {
  currentPath?: string | undefined;
  loginPath?: string | undefined;
  user?: string | undefined;
  platform?: NodeJS.Platform;
} = {}): string => daemonSearchPath({
  platform: overrides.platform ?? "darwin",
  currentPath: overrides.currentPath ?? launchdPath,
  loginPath: overrides.loginPath,
  home: "/Users/dev",
  user: overrides.user,
});

test("the packaged daemon starts outside the application bundle", () => {
  const appRoot = "/Applications/Ait desktop.app/Contents/Resources/app.asar";
  assert.equal(daemonWorkingDirectory(appRoot, "/Users/dev", true), "/Users/dev");
});

test("the development daemon starts in the Cargo workspace", () => {
  assert.equal(daemonWorkingDirectory("/repo/apps/desktop", "/Users/dev", false), "/repo");
});

test("the launchd PATH is extended with the login shell PATH", () => {
  const merged = searchPath({ loginPath: "/Users/dev/.local/share/npm/bin:/opt/homebrew/bin" });
  assert.deepEqual(merged.split(":").slice(0, 2), [
    "/Users/dev/.local/share/npm/bin",
    "/opt/homebrew/bin",
  ]);
  assert.ok(merged.split(":").includes("/usr/bin"));
});

test("well-known tool directories survive an unavailable login shell", () => {
  const directories = searchPath().split(":");
  for (const directory of [
    "/Users/dev/.local/bin",
    "/Users/dev/.local/share/npm/bin",
    "/Users/dev/.cargo/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
  ]) {
    assert.ok(directories.includes(directory), `${directory} is missing`);
  }
});

test("the nix-darwin per-user profile is only added for a known user", () => {
  assert.ok(searchPath({ user: "dev" }).split(":").includes("/etc/profiles/per-user/dev/bin"));
  assert.ok(!searchPath({ user: "  " }).includes("/etc/profiles/per-user"));
});

test("duplicate and blank PATH entries are collapsed", () => {
  const directories = searchPath({ loginPath: "/usr/bin::/opt/homebrew/bin: /usr/bin " }).split(":");
  assert.equal(directories.filter((directory) => directory === "/usr/bin").length, 1);
  assert.equal(directories.filter((directory) => directory === "/opt/homebrew/bin").length, 1);
  assert.ok(!directories.includes(""));
});

test("Windows keeps its inherited PATH and separator", () => {
  const merged = searchPath({ platform: "win32", currentPath: "C:\\Windows;C:\\Tools;C:\\Windows" });
  assert.equal(merged, "C:\\Windows;C:\\Tools");
});

test("a marked PATH is read past interactive shell noise", () => {
  assert.equal(markedPath("dev% __ait_path__/opt/homebrew/bin:/usr/bin__ait_path__"), "/opt/homebrew/bin:/usr/bin");
  assert.equal(markedPath("__ait_path__  __ait_path__"), undefined);
  assert.equal(markedPath("__ait_path__/usr/bin"), undefined);
  assert.equal(markedPath(undefined), undefined);
});

test("a shell that cannot answer yields no login PATH", () => {
  assert.equal(loginShellPath("darwin", "/nonexistent/shell"), undefined);
  assert.equal(loginShellPath("win32", "/bin/sh"), undefined);
});

// Shells disagree on which of `-i`, `-l`, and `-c` they accept, so this covers
// the fallback chain against whichever `/bin/sh` the host provides.
test("a POSIX shell reports its PATH", () => {
  const path = loginShellPath(process.platform, "/bin/sh");
  assert.ok(path && path.split(":").includes("/usr/bin"), `unexpected shell PATH: ${path}`);
});
