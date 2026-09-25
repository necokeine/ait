#!/usr/bin/env node
import { spawn, spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const token = process.env.AIT_SERVER_TOKEN;
if (!token || !/^[\x21-\x7e]{32,256}$/.test(token)) {
  console.error("Set AIT_SERVER_TOKEN to a 32–256 character access token before starting the app.");
  process.exit(1);
}
const port = Number(process.env.EXPO_PORT ?? "8081");
if (!Number.isInteger(port) || port < 1 || port > 65535) {
  console.error("EXPO_PORT must be a valid TCP port.");
  process.exit(1);
}
const env = { ...process.env };
delete env.AIT_SERVER_TOKEN;
delete env.PASEO_WEB_PLATFORM;
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
for (const [command, args] of [
  [npm, ["run", "build:app-deps"]],
  ...(!process.env.AIT_SERVER_BIN
    ? [
        [
          "cargo",
          [
            "build",
            "--target-dir",
            path.join(root, "target"),
            "-p",
            "server-bin",
            "--bin",
            "server",
          ],
        ],
      ]
    : []),
]) {
  const result = spawnSync(command, args, {
    cwd: root,
    env,
    stdio: "inherit",
    shell: process.platform === "win32" && command === npm,
  });
  if (result.error || result.status !== 0) process.exit(result.status ?? 1);
}

const children = new Set();
let stopping = false;
function stop(code) {
  if (stopping) return;
  stopping = true;
  process.exitCode = code;
  for (const child of children) child.kill("SIGTERM");
  const timer = setTimeout(() => {
    for (const child of children) child.kill("SIGKILL");
  }, 17_000);
  timer.unref();
}
function launch(command, args, options = {}) {
  const child = spawn(command, args, { cwd: root, env, stdio: "inherit", ...options });
  children.add(child);
  child.once("error", (error) => {
    console.error(`Cannot start app process: ${error.message}`);
    children.delete(child);
    stop(1);
  });
  child.once("exit", (code) => {
    children.delete(child);
    stop(code ?? 1);
  });
}
process.once("SIGINT", () => stop(0));
process.once("SIGTERM", () => stop(0));
launch(
  process.env.AIT_SERVER_BIN ??
    path.join(root, "target/debug", process.platform === "win32" ? "server.exe" : "server"),
  [
    "--data-dir",
    process.env.AIT_SERVER_DATA_DIR ?? path.join(root, ".tmp/app/server"),
    "--listen",
    process.env.AIT_SERVER_LISTEN ?? "127.0.0.1:7316",
    "--web-origin",
    `http://localhost:${port}`,
    "--web-origin",
    `http://127.0.0.1:${port}`,
  ],
  { env: { ...env, AIT_SERVER_TOKEN: token } },
);
launch(
  process.execPath,
  [
    path.join(root, "node_modules/expo/bin/cli"),
    "start",
    "--web",
    "--localhost",
    "--port",
    String(port),
  ],
  {
    cwd: path.join(root, "apps/app"),
    env: { ...env, APP_VARIANT: "development", CI: "1" },
  },
);
console.log(
  `App: http://localhost:${port}. Add a direct connection using the server address and your access token.`,
);
