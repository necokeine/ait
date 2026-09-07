import { realpath, stat } from "node:fs/promises";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

function localPath(reference: string): string {
  if (!reference.startsWith("file:")) return reference;
  try {
    return fileURLToPath(reference);
  } catch {
    throw new Error("The file reference is invalid.");
  }
}

export async function resolveProjectPath(projectRoot: string, reference: string): Promise<string> {
  if (!reference.trim() || reference.includes("\0")) throw new Error("The file reference is invalid.");
  const root = await realpath(projectRoot);
  const requested = localPath(reference.trim());
  const candidate = await realpath(isAbsolute(requested) ? requested : resolve(root, requested));
  const withinRoot = relative(root, candidate);
  if (withinRoot === ".." || withinRoot.startsWith(`..${sep}`) || isAbsolute(withinRoot)) {
    throw new Error("The file reference is outside the current Project.");
  }
  await stat(candidate);
  return candidate;
}

export function vscodeFileUrl(path: string, line: number, column = 1): string {
  const encodedPath = pathToFileURL(path).pathname;
  return `vscode://file${encodedPath}:${line}:${column}`;
}
