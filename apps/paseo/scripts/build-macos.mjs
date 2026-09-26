import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const desktop = fileURLToPath(new URL("..", import.meta.url));
const signed = process.argv.includes("--signed");
if (process.platform !== "darwin") throw new Error("DMG builds require macOS.");
if (process.argv.slice(2).some((arg) => arg !== "--signed")) {
  throw new Error("Usage: node scripts/build-macos.mjs [--signed]");
}
if (signed && !process.env.CSC_NAME && !process.env.CSC_LINK) {
  throw new Error(
    "Set CSC_NAME (Developer ID Application identity) or CSC_LINK for release signing.",
  );
}
if (
  signed &&
  !process.env.APPLE_API_KEY &&
  !process.env.APPLE_APP_SPECIFIC_PASSWORD &&
  !process.env.APPLE_KEYCHAIN_PROFILE
) {
  throw new Error(
    "Signed distribution requires Apple notarization credentials; see docs/operations/apple-builds.md.",
  );
}

function run(command, args) {
  execFileSync(command, args, { cwd: desktop, stdio: "inherit" });
}

run("npm", ["--prefix", "../..", "run", "build:desktop-assets"]);
run(process.execPath, ["scripts/prepare-server.mjs"]);
run("npm", ["run", "build:main"]);
run("npm", [
  "exec",
  "--",
  "electron-builder",
  "--config",
  signed ? "electron-builder.yml" : "electron-builder.local.yml",
  "--mac",
  "dmg",
  `--${process.arch}`,
  "--publish",
  "never",
  ...(signed ? ["-c.forceCodeSigning=true"] : []),
]);
