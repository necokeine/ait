import assert from "node:assert/strict";
import type { ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";
import test from "node:test";
import { launchDesktopDaemon } from "../src/daemon-launch.js";

const developmentOptions = {
  isPackaged: false,
  platform: "darwin" as const,
  resourcesPath: "/Applications/Ait.app/Contents/Resources",
  appRoot: "/workspace/apps/desktop",
  userData: "/Users/example/Library/Application Support/Ait",
  databaseFilename: "ait-development.sqlite3",
  listen: "127.0.0.1:7315",
  env: {
    HOME: "/Users/example",
    PATH: "/usr/bin:/bin:/usr/sbin:/sbin",
  },
};

test("development startup explains how to install Cargo instead of spawning a missing command", async () => {
  let spawnCalled = false;

  await assert.rejects(
    launchDesktopDaemon(developmentOptions, {
      canExecute: async () => false,
      spawnProcess: () => {
        spawnCalled = true;
        throw new Error("spawn should not be called");
      },
    }),
    /Cargo was not found.*Install Rust.*CARGO/s,
  );
  assert.equal(spawnCalled, false);
});

test("development startup handles an asynchronous Cargo ENOENT as an actionable error", async () => {
  const child = new EventEmitter() as ChildProcess;
  const spawnError = Object.assign(new Error("spawn cargo ENOENT"), { code: "ENOENT" });
  let attemptedExecutable = "";

  const startup = launchDesktopDaemon(developmentOptions, {
    canExecute: async () => true,
    spawnProcess: (executable) => {
      attemptedExecutable = executable;
      queueMicrotask(() => child.emit("error", spawnError));
      return child;
    },
  });

  await assert.rejects(startup, /Cargo.*Install Rust.*CARGO/s);
  assert.equal(attemptedExecutable, "/Users/example/.cargo/bin/cargo");
});

test("packaged startup launches only the bundled daemon sidecar", async () => {
  const child = new EventEmitter() as ChildProcess;
  let invocation: { executable: string; args: string[] } | undefined;

  const launched = launchDesktopDaemon({
    ...developmentOptions,
    isPackaged: true,
    databaseFilename: "ait.sqlite3",
    listen: "127.0.0.1:7314",
  }, {
    canExecute: async (path) => path === "/Applications/Ait.app/Contents/Resources/bin/ait-daemon",
    spawnProcess: (executable, args) => {
      invocation = { executable, args };
      queueMicrotask(() => child.emit("spawn"));
      return child;
    },
  });

  assert.equal(await launched, child);
  assert.deepEqual(invocation, {
    executable: "/Applications/Ait.app/Contents/Resources/bin/ait-daemon",
    args: [
      "--database", "/Users/example/Library/Application Support/Ait/ait.sqlite3",
      "--listen", "127.0.0.1:7314",
    ],
  });
});

test("packaged startup diagnoses a missing bundled daemon without falling back to Cargo", async () => {
  await assert.rejects(
    launchDesktopDaemon({ ...developmentOptions, isPackaged: true }, {
      canExecute: async () => false,
      spawnProcess: () => {
        throw new Error("spawn should not be called");
      },
    }),
    /packaged Ait daemon is missing.*Reinstall Ait/s,
  );
});
