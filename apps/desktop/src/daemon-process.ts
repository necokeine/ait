import { spawnSync } from "node:child_process";
import { join, resolve } from "node:path";

// A macOS bundle launched from Finder inherits launchd's minimal
// `/usr/bin:/bin:/usr/sbin:/sbin`, while `npm run dev` inherits the terminal's
// login shell. The daemon resolves `codex` — and the `node` shim behind it —
// through PATH, so the packaged app has to rebuild the login PATH itself.
const pathMarker = "__ait_path__";
const loginShellTimeoutMs = 5_000;

export interface DaemonSearchPathInput {
  platform: NodeJS.Platform;
  currentPath: string | undefined;
  loginPath: string | undefined;
  home: string;
  user: string | undefined;
}

// The development daemon is built by `cargo run` and must start inside the
// Cargo workspace. The packaged daemon must not start inside the read-only
// application bundle, because Codex inherits this working directory.
export function daemonWorkingDirectory(appRoot: string, home: string, isPackaged: boolean): string {
  return isPackaged ? home : resolve(appRoot, "..", "..");
}

export function daemonSearchPath(input: DaemonSearchPathInput): string {
  const separator = input.platform === "win32" ? ";" : ":";
  const candidates = [input.loginPath, input.currentPath]
    .flatMap((value) => (value ?? "").split(separator))
    .concat(input.platform === "win32" ? [] : fallbackDirectories(input.home, input.user));
  const seen = new Set<string>();
  const ordered: string[] = [];
  for (const candidate of candidates) {
    const directory = candidate.trim();
    if (!directory || seen.has(directory)) continue;
    seen.add(directory);
    ordered.push(directory);
  }
  return ordered.join(separator);
}

// Returns undefined when no invocation of the shell answers: it is unavailable,
// it times out, or its output carries no readable marker.
export function loginShellPath(platform: NodeJS.Platform, shell: string | undefined): string | undefined {
  if (platform === "win32") return undefined;
  const executable = shell?.trim() || "/bin/sh";
  const script = `printf '%s%s%s' '${pathMarker}' "$PATH" '${pathMarker}'`;
  // Interactive login shells expose the most complete PATH, but not every shell
  // accepts those flags, so the plainer invocations are tried in turn.
  for (const flags of [["-i", "-l", "-c"], ["-l", "-c"], ["-c"]]) {
    const result = spawnSync(executable, [...flags, script], {
      encoding: "utf8",
      timeout: loginShellTimeoutMs,
      stdio: ["ignore", "pipe", "ignore"],
    });
    const path = markedPath(result.stdout);
    if (path) return path;
  }
  return undefined;
}

// An interactive shell may wrap its answer in prompts and banners, so the PATH
// is only trusted between the two markers.
export function markedPath(output: string | undefined): string | undefined {
  if (!output) return undefined;
  const start = output.indexOf(pathMarker);
  if (start < 0) return undefined;
  const end = output.indexOf(pathMarker, start + pathMarker.length);
  if (end < 0) return undefined;
  return output.slice(start + pathMarker.length, end).trim() || undefined;
}

function fallbackDirectories(home: string, user: string | undefined): string[] {
  const perUserProfile = user?.trim() ? [join("/etc/profiles/per-user", user.trim(), "bin")] : [];
  return [
    join(home, ".local", "bin"),
    join(home, ".local", "share", "npm", "bin"),
    join(home, ".npm-global", "bin"),
    join(home, ".cargo", "bin"),
    join(home, ".nix-profile", "bin"),
    ...perUserProfile,
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/run/current-system/sw/bin",
    "/nix/var/nix/profiles/default/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
  ];
}
