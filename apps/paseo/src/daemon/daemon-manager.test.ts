import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DEFAULT_DESKTOP_SETTINGS } from "../settings/desktop-settings";
import { createDaemonCommandHandlers } from "./daemon-manager";

const mocks = vi.hoisted(() => ({
  paseoHome: "",
  start: vi.fn(),
  stop: vi.fn(),
  restart: vi.fn(),
  status: vi.fn(() => ({ listen: "127.0.0.1:43210", ownedByDesktop: true })),
  authorization: vi.fn(),
  openTransport: vi.fn(),
  settings: {
    releaseChannel: "stable",
    daemon: {
      manageBuiltInDaemon: true,
      keepRunningAfterQuit: true,
    },
  },
  runExternalCliJsonCommand: vi.fn(),
  runExternalCliTextCommand: vi.fn(),
  createNodeEntrypointInvocation: vi.fn(() => ({
    command: "node",
    args: [],
    env: {},
  })),
  spawnProcess: vi.fn(),
  logInfo: vi.fn(),
  logError: vi.fn(),
  appLogPath: "",
  getElectronLogFile: vi.fn(),
}));

vi.mock("electron", () => ({
  app: {
    getPath: vi.fn(() => mocks.paseoHome),
    getVersion: vi.fn(() => "1.2.3"),
    isPackaged: false,
  },
  ipcMain: { handle: vi.fn() },
  powerMonitor: { getSystemIdleTime: vi.fn(() => 0) },
}));

vi.mock("electron-log/main", () => ({
  default: {
    info: mocks.logInfo,
    error: mocks.logError,
    transports: {
      file: {
        getFile: mocks.getElectronLogFile,
      },
    },
  },
}));

vi.mock("./rust-server.js", () => ({
  resolveDesktopServerHome: () => mocks.paseoHome,
  RustServerManager: class {
    start = mocks.start;
    stop = mocks.stop;
    restart = mocks.restart;
    status = mocks.status;
    authorization = mocks.authorization;
  },
}));
vi.mock("./local-transport.js", () => ({
  openLocalTransportSession: mocks.openTransport,
  sendLocalTransportMessage: vi.fn(),
  closeLocalTransportSession: vi.fn(),
}));

vi.mock("../settings/desktop-settings-electron.js", () => ({
  getDesktopSettingsStore: () => ({
    get: async () => mocks.settings,
    patch: vi.fn(),
    migrateLegacyRendererSettings: vi.fn(),
  }),
}));

vi.mock("./runtime-paths.js", () => ({
  createNodeEntrypointInvocation: mocks.createNodeEntrypointInvocation,
  resolveDaemonRunnerEntrypoint: vi.fn(() => ({
    entryPath: path.join(mocks.paseoHome, "daemon.js"),
    execArgv: [],
  })),
}));

vi.mock("./cli/external.js", () => ({
  runExternalCliJsonCommand: mocks.runExternalCliJsonCommand,
  runExternalCliTextCommand: mocks.runExternalCliTextCommand,
}));

describe("daemon-manager commands", () => {
  let fixtureRoot: string;

  beforeEach(() => {
    fixtureRoot = mkdtempSync(path.join(tmpdir(), "paseo daemon manager "));
    mocks.paseoHome = path.join(fixtureRoot, "home");
    mocks.appLogPath = path.join(fixtureRoot, "main.log");
    mocks.settings = DEFAULT_DESKTOP_SETTINGS;
    mocks.runExternalCliJsonCommand.mockReset();
    mocks.runExternalCliTextCommand.mockReset();
    mocks.createNodeEntrypointInvocation.mockReset();
    mocks.createNodeEntrypointInvocation.mockReturnValue({
      command: "node",
      args: [],
      env: {},
    });
    mocks.spawnProcess.mockReset();
    mocks.logInfo.mockReset();
    mocks.logError.mockReset();
    mocks.getElectronLogFile.mockReset();
    mocks.getElectronLogFile.mockReturnValue({ path: mocks.appLogPath });
  });

  afterEach(() => {
    rmSync(fixtureRoot, { recursive: true, force: true });
  });

  it("starts the Rust owner and injects managed credentials only in the main process", async () => {
    const handlers = createDaemonCommandHandlers();
    mocks.start.mockResolvedValue({ status: "running" });
    await expect(handlers.start_desktop_daemon()).resolves.toEqual({ status: "running" });
    mocks.authorization.mockReturnValue("private-token");
    await handlers.open_local_daemon_transport({
      sessionId: "test",
      target: { transportType: "rustTcp", url: "ws://localhost:43210/v1/ws" },
    });
    expect(mocks.openTransport).toHaveBeenLastCalledWith({
      sessionId: "test",
      bearerToken: "private-token",
      target: { transportType: "rustTcp", url: "ws://127.0.0.1:43210/v1/ws" },
    });
    mocks.authorization.mockReturnValue(undefined);
    const external = {
      sessionId: "external",
      target: { transportType: "rustTcp", url: "ws://localhost:43211/v1/ws" },
      bearerToken: "external-token",
    };
    await handlers.open_local_daemon_transport(external);
    expect(mocks.openTransport).toHaveBeenLastCalledWith(external);
  });

  it("honors the built-in management switch", async () => {
    mocks.settings = {
      ...DEFAULT_DESKTOP_SETTINGS,
      daemon: { manageBuiltInDaemon: false, keepRunningAfterQuit: false },
    };
    await expect(createDaemonCommandHandlers().start_desktop_daemon()).rejects.toThrow("disabled");
  });

  it("returns the Electron main-process log tail from electron-log", () => {
    writeFileSync(
      mocks.appLogPath,
      Array.from({ length: 105 }, (_value, index) => `main log line ${index + 1}`).join("\n"),
    );
    const handlers = createDaemonCommandHandlers();

    expect(handlers.desktop_app_logs()).toEqual({
      logPath: mocks.appLogPath,
      contents: Array.from({ length: 100 }, (_value, index) => `main log line ${index + 6}`).join(
        "\n",
      ),
    });
  });

  it("exposes updater diagnostics through the desktop command boundary", () => {
    const diagnostics = createDaemonCommandHandlers().desktop_update_diagnostics();

    expect(diagnostics).toMatchObject({
      platform: process.platform,
      currentVersion: "1.2.3",
    });
  });
});
