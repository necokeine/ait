import { spawn, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { appendFileSync, mkdirSync } from "node:fs";
import { createServer } from "node:net";
import { hostname, homedir } from "node:os";
import path from "node:path";
import { WebSocket } from "ws";

export function resolveDesktopServerHome(env: NodeJS.ProcessEnv): string {
  return path.resolve(env.AIT_SERVER_DATA_DIR || path.join(homedir(), ".ait-server-desktop"));
}

export interface RustServerStatus {
  serverId: string;
  status: "starting" | "running" | "stopped" | "errored";
  listen: string | null;
  hostname: string | null;
  pid: number | null;
  home: string;
  version: string | null;
  desktopManaged: boolean;
  ownedByDesktop: boolean;
  startedAt: string | null;
  error: string | null;
}

/** Owns only the child it spawned; never adopts or kills a PID read from disk. */
export class RustServerManager {
  private child: ChildProcess | null = null;
  private token: string | null = null;
  private listenAddress: string | null = null;
  private queue: Promise<unknown> = Promise.resolve();
  private state: RustServerStatus;

  constructor(
    private readonly options: {
      binary: string;
      home: string;
      listen?: string;
      timeoutMs?: number;
    },
  ) {
    this.state = {
      serverId: "",
      status: "stopped",
      listen: null,
      hostname: null,
      pid: null,
      home: options.home,
      version: null,
      desktopManaged: true,
      ownedByDesktop: false,
      startedAt: null,
      error: null,
    };
  }

  status(): RustServerStatus {
    return { ...this.state };
  }

  authorization(url: string): string | undefined {
    if (this.state.status !== "running" || !this.state.listen) return undefined;
    try {
      const requested = new URL(url);
      const owned = new URL(`ws://${this.state.listen}/v1/ws`);
      return requested.protocol === "ws:" &&
        requested.port === owned.port &&
        [owned.hostname, "localhost"].includes(requested.hostname) &&
        requested.pathname === owned.pathname &&
        !requested.search &&
        !requested.hash &&
        !requested.username &&
        !requested.password
        ? (this.token ?? undefined)
        : undefined;
    } catch {
      return undefined;
    }
  }

  private serialize<T>(work: () => Promise<T>): Promise<T> {
    const next = this.queue.then(work);
    this.queue = next.catch(() => {});
    return next;
  }

  start(): Promise<RustServerStatus> {
    return this.serialize(() => this.launch());
  }
  stop(confirmed?: { pid: number; startedAt: string }): Promise<RustServerStatus> {
    return this.serialize(async () => {
      if (
        confirmed &&
        (confirmed.pid !== this.state.pid || confirmed.startedAt !== this.state.startedAt)
      ) {
        throw new Error(
          "Server changed since confirmation; inspect the current instance before stopping it.",
        );
      }
      await this.terminate();
      this.state = {
        ...this.state,
        status: "stopped",
        pid: null,
        listen: null,
        ownedByDesktop: false,
        error: null,
      };
      return this.status();
    });
  }
  restart(): Promise<RustServerStatus> {
    return this.serialize(async () => {
      await this.terminate();
      return this.launch();
    });
  }

  private async terminate(): Promise<void> {
    const child = this.child;
    this.token = null;
    if (!child || child.exitCode !== null || child.signalCode !== null) {
      this.child = null;
      return;
    }
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => child.kill("SIGKILL"), 15_000);
      child.once("close", () => {
        clearTimeout(timer);
        resolve();
      });
      child.kill("SIGTERM");
    });
    this.child = null;
  }

  private async launch(): Promise<RustServerStatus> {
    if (this.child && this.state.status === "running") return this.status();
    mkdirSync(this.options.home, { recursive: true, mode: 0o700 });
    const listenAddress = await resolveListen(
      this.listenAddress ?? this.options.listen ?? "127.0.0.1:0",
    );
    this.token = randomBytes(32).toString("hex");
    const env = Object.fromEntries(
      Object.entries(process.env).filter(([key]) => !key.startsWith("AIT_SERVER_")),
    );
    const child = spawn(
      this.options.binary,
      ["--data-dir", this.options.home, "--listen", listenAddress, "--log-level", "info"],
      {
        env: { ...env, AIT_SERVER_TOKEN: this.token },
        stdio: ["ignore", "ignore", "pipe"],
        windowsHide: true,
      },
    );
    this.child = child;
    this.state = {
      ...this.state,
      status: "starting",
      pid: child.pid ?? null,
      listen: null,
      startedAt: new Date().toISOString(),
      ownedByDesktop: true,
      error: null,
    };
    let tail = "";
    let failure: Error | null = null;
    child.on("error", (error) => {
      failure = error;
    });
    child.once("close", () => {
      if (this.child !== child) return;
      this.child = null;
      this.token = null;
      this.state = {
        ...this.state,
        status: "errored",
        pid: null,
        ownedByDesktop: false,
        error: failure?.message ?? "Rust server exited.",
      };
    });
    child.stderr!.on("data", (chunk: Buffer) => {
      const text = chunk.toString();
      tail = (tail + text).slice(-16384);
      try {
        appendFileSync(path.join(this.options.home, "daemon.log"), text, { mode: 0o600 });
      } catch {
        /* A log write failure must not crash Electron or orphan its child. */
      }
    });
    try {
      const deadline = Date.now() + (this.options.timeoutMs ?? 30_000);
      let listen: string | undefined;
      while (!listen && Date.now() < deadline) {
        if (failure) throw failure;
        if (child.exitCode !== null || child.signalCode !== null || this.child !== child)
          throw new Error(tail || "Rust server exited before ready.");
        listen = tail.match(/server ready\s+listen=(127\.0\.0\.1:\d+|\[::1\]:\d+)/)?.[1];
        if (!listen) await new Promise((resolve) => setTimeout(resolve, 25));
      }
      if (!listen) throw new Error("Timed out waiting for Rust server readiness.");
      const serverId = await probeRustServer(
        listen,
        this.token!,
        Math.max(1, deadline - Date.now()),
      );
      if (this.child !== child) throw new Error("Rust server exited during readiness handshake.");
      this.listenAddress = listen;
      this.state = {
        ...this.state,
        serverId,
        listen,
        status: "running",
        hostname: hostname(),
      };
      return this.status();
    } catch (error) {
      await this.terminate();
      this.state = {
        ...this.state,
        status: "errored",
        pid: null,
        ownedByDesktop: false,
        error: error instanceof Error ? error.message : String(error),
      };
      throw error;
    }
  }
}

function probeRustServer(listen: string, token: string, timeout: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://${listen}/v1/ws`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    const timer = setTimeout(() => finish(new Error("Rust server handshake timed out.")), timeout);
    let settled = false;
    const finish = (error?: Error, serverId?: string) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      ws.close();
      if (error) reject(error);
      else resolve(serverId!);
    };
    ws.on("error", (error) => finish(error));
    ws.on("close", () => finish(new Error("Rust server closed before readiness handshake.")));
    ws.on("open", () =>
      ws.send(
        JSON.stringify({
          type: "hello",
          protocol: { major: 1, min_minor: 0, max_minor: 0 },
          client_id: "desktop-readiness",
          capabilities: [],
          required_capabilities: [],
        }),
      ),
    );
    ws.on("message", (data) => {
      try {
        const message = JSON.parse(data.toString());
        if (message.type === "server_info" && typeof message.info?.server_id === "string")
          finish(undefined, message.info.server_id);
        else finish(new Error("Unexpected Rust server readiness response."));
      } catch {
        finish(new Error("Invalid Rust server readiness response."));
      }
    });
  });
}

// Reserve an ephemeral port before spawn, then pass a concrete address so a
// server-internal restart rebinds the same endpoint. A bind race fails closed.
async function resolveListen(listen: string): Promise<string> {
  const match = /^(127\.0\.0\.1|\[::1\]):0$/.exec(listen);
  if (!match) return listen;
  return new Promise((resolve, reject) => {
    const listener = createServer();
    listener.once("error", reject);
    listener.listen(0, match[1].replace("[", "").replace("]", ""), () => {
      const address = listener.address();
      listener.close((error) => {
        if (error) reject(error);
        else if (!address || typeof address === "string")
          reject(new Error("Could not allocate a server port."));
        else resolve(`${match[1]}:${address.port}`);
      });
    });
  });
}
