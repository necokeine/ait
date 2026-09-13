import { cp, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { runIconsTool } from "app-builder-lib/out/toolsets/icons.js";

// Use the icon converter shipped with our pinned electron-builder version.
const source = fileURLToPath(new URL("../../../logo.svg", import.meta.url));
const output = fileURLToPath(new URL("../release-resources/icons/", import.meta.url));
await mkdir(output, { recursive: true });
await runIconsTool({ inputFile: source, outputFormat: "set", outDir: output });
await cp(`${output}/512x512.png`, new URL("../../../logo.png", import.meta.url));
