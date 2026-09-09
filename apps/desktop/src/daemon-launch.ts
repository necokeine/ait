import { spawn, type ChildProcess, type SpawnOptions } from "node:child_process";
import { constants } from "node:fs";
import { access, stat } from "node:fs/promises";
import { delimiter, isAbsolute, join, resolve } from "node:path";

export interface DesktopDaemonLaunchOptions {
  isPackaged: boolean;
  platform: NodeJS.Platform;
  resourcesPath: string;
  appRoot: string;
  userData: string;
  databaseFilename: string;
  listen: string;
  env?: NodeJS.ProcessEnv;
}

type CanExecute = (path: string) => Promise<boolean>;
type SpawnProcess = (executable: string, args: string[], options: SpawnOptions) => ChildProcess;

export interface DesktopDaemonLaunchDependencies {
  canExecute?: CanExecute;
  spawnProcess?: SpawnProcess;
}

interface DaemonLaunchCommand {
  executable: string;
  args: string[];
  cwd: string;
}

export async function launchDesktopDaemon(
  options: DesktopDaemonLaunchOptions,
  dependencies: DesktopDaemonLaunchDependencies = {},
): Promise<ChildProcess> {
  const command = await daemonLaunchCommand(options, dependencies.canExecute ?? canExecute);
  const spawnProcess = dependencies.spawnProcess ?? spawn;
  let child: ChildProcess;
  try {
    child = spawnProcess(command.executable, command.args, {
      cwd: command.cwd,
      stdio: ["ignore", "ignore", "pipe"],
    });
  } catch (error) {
    throw daemonSpawnError(options, command.executable, error);
  }
  await waitForSpawn(child, options, command.executable);
  child.on("error", (error) => {
    console.error(`[ait-daemon] ${daemonSpawnError(options, command.executable, error).message}`);
  });
  return child;
}

async function daemonLaunchCommand(
  options: DesktopDaemonLaunchOptions,
  isExecutable: CanExecute,
): Promise<DaemonLaunchCommand> {
  const database = join(options.userData, options.databaseFilename);
  if (options.isPackaged) {
    const executable = join(
      options.resourcesPath,
      "bin",
      options.platform === "win32" ? "ait-daemon.exe" : "ait-daemon",
    );
    if (!await isExecutable(executable)) {
      throw new Error(`The packaged Ait daemon is missing or cannot be executed at ${executable}. Reinstall Ait and try again.`);
    }
    return {
      executable,
      args: ["--database", database, "--listen", options.listen],
      cwd: resolve(options.appRoot, "../.."),
    };
  }

  const executable = await findCargo(options, isExecutable);
  if (!executable) {
    throw cargoNotFoundError();
  }
  return {
    executable,
    args: [
      "run", "--quiet", "-p", "ait-daemon", "--features", "dev-mock-provider", "--",
      "--database", database, "--listen", options.listen,
    ],
    cwd: resolve(options.appRoot, "../.."),
  };
}

async function findCargo(options: DesktopDaemonLaunchOptions, isExecutable: CanExecute): Promise<string | undefined> {
  const env = options.env ?? process.env;
  const executableName = options.platform === "win32" ? "cargo.exe" : "cargo";
  const candidates: string[] = [];
  if (env.CARGO && isAbsolute(env.CARGO)) candidates.push(env.CARGO);
  if (env.CARGO_HOME && isAbsolute(env.CARGO_HOME)) candidates.push(join(env.CARGO_HOME, "bin", executableName));
  const home = env.HOME ?? env.USERPROFILE;
  if (home && isAbsolute(home)) candidates.push(join(home, ".cargo", "bin", executableName));
  for (const directory of (env.PATH ?? "").split(delimiter)) {
    if (directory && isAbsolute(directory)) candidates.push(join(directory, executableName));
  }

  for (const candidate of new Set(candidates)) {
    if (await isExecutable(candidate)) return candidate;
  }
  return undefined;
}

async function canExecute(path: string): Promise<boolean> {
  try {
    const metadata = await stat(path);
    if (!metadata.isFile()) return false;
    await access(path, process.platform === "win32" ? constants.F_OK : constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function waitForSpawn(
  child: ChildProcess,
  options: DesktopDaemonLaunchOptions,
  executable: string,
): Promise<void> {
  return new Promise((resolveSpawn, rejectSpawn) => {
    const onSpawn = (): void => {
      child.off("error", onError);
      resolveSpawn();
    };
    const onError = (error: Error): void => {
      child.off("spawn", onSpawn);
      rejectSpawn(daemonSpawnError(options, executable, error));
    };
    child.once("spawn", onSpawn);
    child.once("error", onError);
  });
}

function cargoNotFoundError(): Error {
  return new Error(
    "Cargo was not found. Ait development mode requires Rust. Install Rust from https://rustup.rs/, "
    + "or set CARGO to the absolute cargo executable path before starting Ait.",
  );
}

function daemonSpawnError(options: DesktopDaemonLaunchOptions, executable: string, error: unknown): Error {
  const code = typeof error === "object" && error !== null && "code" in error ? error.code : undefined;
  if (code === "ENOENT") {
    if (!options.isPackaged) return cargoNotFoundError();
    return new Error(`The packaged Ait daemon is missing at ${executable}. Reinstall Ait and try again.`);
  }
  const detail = error instanceof Error ? error.message : String(error);
  return new Error(`Could not start the Ait daemon executable at ${executable}: ${detail}`);
}
