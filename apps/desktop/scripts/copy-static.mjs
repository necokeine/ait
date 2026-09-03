import { cp, mkdir, rm } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
await rm(resolve(root, "dist/src"), { recursive: true, force: true });
await mkdir(resolve(root, "dist"), { recursive: true });
await Promise.all([
  cp(resolve(root, "src/index.html"), resolve(root, "dist/index.html")),
  cp(resolve(root, "src/styles.css"), resolve(root, "dist/styles.css")),
]);
