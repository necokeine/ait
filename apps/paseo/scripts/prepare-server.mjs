import { mkdirSync, copyFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { execFileSync } from "node:child_process";
const desktop = fileURLToPath(new URL("..", import.meta.url));
const root = path.resolve(desktop, "../..");
const name = process.platform === "win32" ? "server.exe" : "server";
// Build for the host platform; cross-platform packaging must provide a matching binary.
const binary = process.env.AIT_SERVER_BIN || path.join(root, "target/release", name);
if (!process.env.AIT_SERVER_BIN) {
  execFileSync(
    "cargo",
    [
      "build",
      "--release",
      "--target-dir",
      path.join(root, "target"),
      "-p",
      "server-bin",
      "--bin",
      "server",
    ],
    { cwd: root, stdio: "inherit" },
  );
}
const directory = path.join(desktop, "release-resources/server");
mkdirSync(directory, { recursive: true });
copyFileSync(binary, path.join(directory, name));
