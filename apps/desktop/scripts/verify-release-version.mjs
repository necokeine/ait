import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const desktopRoot = resolve(here, "..");
const repositoryRoot = resolve(desktopRoot, "../..");

const [packageText, cargoText] = await Promise.all([
  readFile(resolve(desktopRoot, "package.json"), "utf8"),
  readFile(resolve(repositoryRoot, "Cargo.toml"), "utf8"),
]);

const desktopVersion = JSON.parse(packageText).version;
const workspacePackage = cargoText.match(/\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/)?.[1];
const rustVersion = workspacePackage?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!rustVersion) {
  throw new Error("Could not read workspace.package.version from Cargo.toml");
}

const tag = process.argv[2] ?? `v${desktopVersion}`;
if (!/^v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/.test(tag)) {
  throw new Error(`Release tag must use semantic version form vX.Y.Z: ${tag}`);
}

const tagVersion = tag.slice(1);
if (desktopVersion !== rustVersion || desktopVersion !== tagVersion) {
  throw new Error(
    `Release versions differ: tag=${tagVersion}, desktop=${desktopVersion}, rust=${rustVersion}`,
  );
}

console.log(`Release version ${tagVersion} is consistent across the tag, desktop, and Rust workspace.`);
