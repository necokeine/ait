import { chmod, copyFile, mkdir, stat } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const sourceArgument = process.argv[2];
if (!sourceArgument) {
  throw new Error("Usage: npm run stage:daemon -- <path-to-ait-daemon>");
}

const source = resolve(process.cwd(), sourceArgument);
const sourceMetadata = await stat(source).catch(() => null);
if (!sourceMetadata?.isFile()) {
  throw new Error(`Ait daemon binary does not exist: ${source}`);
}

const here = dirname(fileURLToPath(import.meta.url));
const destination = resolve(here, "..", "release-resources", "bin", "ait-daemon");
await mkdir(dirname(destination), { recursive: true });
await copyFile(source, destination);
await chmod(destination, 0o755);

console.log(`Staged ${source} at ${destination}`);
