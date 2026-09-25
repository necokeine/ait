import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const app = fileURLToPath(new URL("..", import.meta.url));
const mode = process.argv[2] ?? "--unsigned";
if (process.platform !== "darwin") throw new Error("Local iOS builds require macOS and Xcode.");
if (process.argv.length > 3 || !["--simulator", "--unsigned", "--export"].includes(mode)) {
  throw new Error("Usage: node scripts/build-ios.mjs [--simulator|--unsigned|--export]");
}
const signed = mode === "--export";
const simulator = mode === "--simulator";
const exportOptions = process.env.IOS_EXPORT_OPTIONS_PLIST
  ? path.resolve(process.env.IOS_EXPORT_OPTIONS_PLIST)
  : undefined;
if (
  signed &&
  (!process.env.APPLE_TEAM_ID || !process.env.IOS_BUNDLE_IDENTIFIER || !exportOptions)
) {
  throw new Error(
    "IPA export requires APPLE_TEAM_ID, IOS_BUNDLE_IDENTIFIER and IOS_EXPORT_OPTIONS_PLIST. See docs/operations/apple-builds.md.",
  );
}
if (signed && !existsSync(exportOptions))
  throw new Error(`Missing export options: ${exportOptions}`);
if (signed) {
  const options = JSON.parse(
    execFileSync("plutil", ["-convert", "json", "-o", "-", exportOptions], { encoding: "utf8" }),
  );
  if (options.destination && options.destination !== "export") {
    throw new Error(
      "ExportOptions.plist destination must be export; this command does not upload apps.",
    );
  }
}

const output = path.join(
  app,
  "release/ios",
  simulator ? "simulator" : signed ? "signed" : "unsigned",
);
const env = {
  ...process.env,
  CI: "1",
  EXPO_NO_TELEMETRY: "1",
  EXTRA_PACKAGER_ARGS: process.env.EXTRA_PACKAGER_ARGS ?? "--max-workers 2",
  APP_VARIANT: process.env.APP_VARIANT ?? "production",
};
// A mobile bundle must never inherit the desktop Metro platform switch.
delete env.PASEO_WEB_PLATFORM;
function run(command, args, cwd = app) {
  execFileSync(command, args, { cwd, env, stdio: "inherit" });
}

run("xcodebuild", ["-version"]);
run("pod", ["--version"]);
run("npm", ["--prefix", "../..", "run", "build:app-deps"]);
run("npm", ["run", "build:terminal-webview"]);
run("npm", ["exec", "--", "expo", "prebuild", "--platform", "ios", "--no-install"]);
run("pod", ["install"], path.join(app, "ios"));
const workspaces = readdirSync(path.join(app, "ios")).filter((name) =>
  name.endsWith(".xcworkspace"),
);
if (workspaces.length !== 1) throw new Error("Expected exactly one generated iOS workspace.");
const scheme = workspaces[0].replace(/\.xcworkspace$/, "");
const archive = path.join(output, `${scheme}.xcarchive`);
mkdirSync(output, { recursive: true });
const provisioning =
  signed && process.env.IOS_ALLOW_PROVISIONING_UPDATES === "1" ? ["-allowProvisioningUpdates"] : [];
run("xcodebuild", [
  "-hideShellScriptEnvironment",
  "-workspace",
  path.join(app, "ios", workspaces[0]),
  "-scheme",
  scheme,
  "-configuration",
  "Release",
  "-destination",
  simulator ? "generic/platform=iOS Simulator" : "generic/platform=iOS",
  "-derivedDataPath",
  path.join(output, "DerivedData"),
  "-jobs",
  process.env.IOS_BUILD_JOBS ?? "4",
  ...(simulator
    ? ["build", `ARCHS=${process.arch === "arm64" ? "arm64" : "x86_64"}`, "ONLY_ACTIVE_ARCH=YES"]
    : ["archive", "-archivePath", archive]),
  ...(signed
    ? [
        `DEVELOPMENT_TEAM=${process.env.APPLE_TEAM_ID}`,
        "CODE_SIGN_STYLE=Automatic",
        ...provisioning,
      ]
    : ["CODE_SIGNING_ALLOWED=NO"]),
]);
if (signed) {
  run("xcodebuild", [
    "-exportArchive",
    "-archivePath",
    archive,
    "-exportPath",
    output,
    "-exportOptionsPlist",
    exportOptions,
    ...provisioning,
  ]);
}
console.log(
  simulator
    ? `Simulator app: ${path.join(output, "DerivedData/Build/Products/Release-iphonesimulator", `${scheme}.app`)}`
    : `${signed ? "Signed archive and IPA" : "Unsigned device archive (requires signing before installation)"}: ${output}`,
);
