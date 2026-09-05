import { app, BrowserWindow, dialog, ipcMain, shell } from "electron";
import { spawn, type ChildProcess } from "node:child_process";
import { randomUUID } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const endpoint = "http://127.0.0.1:7314";
const allowedMethods = new Set([
  "workspace.snapshot", "settings.get", "settings.save", "settings.reset",
  "project.choose-directory", "project.create", "project.set-default-agent",
  "session.create", "session.send-message", "session.fork",
]);
const builtInCodexAgentId = "codex-local";

interface DaemonResponse {
  ok: boolean;
  result?: { kind: string; value: unknown };
  error?: { code: string; message: string };
}

interface WorkspaceView {
  projects: Array<{ id: string; name: string; workdir: string; default_agent_id?: string | null }>;
  agents: Array<{ id: string; name: string; model: string; enabled: boolean }>;
  sessions: Array<{ id: string; project_id: string; agent_id: string; current_message_id: string; active_run_id: string | null; version: number }>;
  messages: Array<{ id: string; project_id: string; parent_message_id: string | null; role: string; kind: string; text: string | null; data?: unknown }>;
}

class DaemonClient {
  private ownedProcess: ChildProcess | undefined;
  private startup: Promise<void> | undefined;
  private snapshotRevision = 0;

  ensureStarted(): Promise<void> {
    this.startup ??= this.start().then(() => this.ensureBuiltInAgents());
    return this.startup;
  }

  async request(method: string, rawParams: unknown): Promise<unknown> {
    if (!allowedMethods.has(method)) throw new Error("Unsupported desktop operation.");
    await this.ensureStarted();
    const params = objectParams(rawParams);
    if (method === "workspace.snapshot") return this.snapshot();
    if (method === "settings.get") return this.get("/v1/settings", "settings");
    if (method === "settings.save") return this.post("/v1/settings/save", "settings", {
      expected_revision: params.expectedRevision, values: params.values,
    });
    if (method === "settings.reset") return this.post("/v1/settings/reset", "settings", {});
    if (method === "project.choose-directory") {
      const result = await dialog.showOpenDialog({
        title: "Choose a Project directory",
        properties: ["openDirectory", "createDirectory"],
      });
      return result.canceled ? null : result.filePaths[0] ?? null;
    }
    if (method === "project.create") {
      const id = randomUUID();
      await this.post("/v1/project/register", "project", {
        id, name: params.name, workdir: params.workdir,
      });
      await this.post("/v1/project/set-default-agent", "project", {
        project_id: id, agent_id: params.agentId,
      });
      return { snapshot: await this.snapshot(), selectedProjectId: id };
    }
    if (method === "project.set-default-agent") {
      await this.post("/v1/project/set-default-agent", "project", {
        project_id: params.projectId, agent_id: params.agentId,
      });
      return this.snapshot();
    }
    if (method === "session.create") {
      const id = randomUUID();
      await this.post("/v1/session/create", "session", {
        id, project_id: params.projectId, agent_id: params.agentId,
      });
      return { snapshot: await this.snapshot(), selectedSessionId: id };
    }
    if (method === "session.send-message") {
      await this.post("/v1/session/send-message", "run", {
        session_id: params.sessionId, text: params.content,
        expected_version: params.expectedVersion,
      });
      return this.snapshot();
    }

    const id = randomUUID();
    await this.post("/v1/session/fork", "run", {
      id, project_id: params.projectId, agent_id: params.agentId,
      at_message_id: params.sourceMessageId, text: params.content,
    });
    return { snapshot: await this.snapshot(), selectedSessionId: id };
  }

  stop(): void {
    this.ownedProcess?.kill();
    this.ownedProcess = undefined;
  }

  private async start(): Promise<void> {
    if (await this.isReady()) return;
    const appRoot = resolve(here, "..");
    const workspaceRoot = resolve(appRoot, "../..");
    const executable = app.isPackaged
      ? join(process.resourcesPath, "bin", process.platform === "win32" ? "ait-daemon.exe" : "ait-daemon")
      : "cargo";
    const database = join(app.getPath("userData"), "ait.sqlite3");
    const args = app.isPackaged
      ? ["--database", database, "--listen", "127.0.0.1:7314"]
      : ["run", "--quiet", "-p", "ait-daemon", "--", "--database", database, "--listen", "127.0.0.1:7314"];
    this.ownedProcess = spawn(executable, args, { cwd: workspaceRoot, stdio: ["ignore", "ignore", "pipe"] });
    this.ownedProcess.stderr?.on("data", (chunk: Buffer) => {
      const message = chunk.toString("utf8").trim();
      if (message) console.error(`[ait-daemon] ${message}`);
    });
    this.ownedProcess.once("exit", () => { this.ownedProcess = undefined; });
    for (let attempt = 0; attempt < 60; attempt += 1) {
      if (await this.isReady()) return;
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
    }
    this.stop();
    throw new Error("Ait daemon did not become ready in time.");
  }

  private async ensureBuiltInAgents(): Promise<void> {
    const workspace = await this.get("/v1/workspace/snapshot", "workspace") as WorkspaceView;
    if (workspace.agents.some((agent) => agent.id === builtInCodexAgentId)) return;
    await this.post("/v1/agent/register", "agent", {
      id: builtInCodexAgentId,
      name: "Codex",
      model: "gpt-5.6-codex",
      mode: "echo",
    });
  }

  private async isReady(): Promise<boolean> {
    try {
      const response = await fetch(`${endpoint}/v1/workspace/snapshot`, { signal: AbortSignal.timeout(500) });
      return response.ok;
    } catch { return false; }
  }

  private async get(path: string, kind: string): Promise<unknown> {
    return this.unwrap(await fetch(`${endpoint}${path}`), kind);
  }

  private async post(path: string, kind: string, body: unknown): Promise<unknown> {
    return this.unwrap(await fetch(`${endpoint}${path}`, {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
    }), kind);
  }

  private async unwrap(response: globalThis.Response, expectedKind: string): Promise<unknown> {
    if (!response.ok) throw new Error(`Ait daemon returned HTTP ${response.status}.`);
    const envelope = await response.json() as DaemonResponse;
    if (!envelope.ok || !envelope.result) {
      const error = new Error(envelope.error?.message ?? "Ait daemon rejected the operation.") as Error & { code?: string };
      if (envelope.error?.code !== undefined) error.code = envelope.error.code;
      throw error;
    }
    if (envelope.result.kind !== expectedKind) throw new Error("Ait daemon returned an unexpected response.");
    return envelope.result.value;
  }

  private async snapshot(): Promise<unknown> {
    const workspace = await this.get("/v1/workspace/snapshot", "workspace") as WorkspaceView;
    this.snapshotRevision += 1;
    return {
      protocolVersion: 1,
      revision: this.snapshotRevision,
      projects: workspace.projects.map((project) => ({
        id: project.id, name: project.name, workdir: project.workdir, description: "",
        defaultAgentId: project.default_agent_id ?? null,
      })),
      agents: workspace.agents.map(({ id, name, model, enabled }) => ({ id, name, model, enabled })),
      sessions: workspace.sessions.map((session) => ({
        id: session.id, projectId: session.project_id, title: `Session ${session.id.slice(0, 8)}`,
        currentMessageId: session.current_message_id, agentId: session.agent_id,
        version: session.version, active: session.active_run_id !== null, updatedAt: 0,
      })),
      messages: workspace.messages.map((message) => ({
        id: message.id, projectId: message.project_id, parentMessageId: message.parent_message_id,
        role: message.role, kind: message.kind, parts: messageParts(message), createdAt: 0,
      })),
    };
  }
}

function objectParams(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : {};
}

function messageParts(message: WorkspaceView["messages"][number]): unknown[] {
  if (message.text !== null) return [{ type: "text", text: message.text }];
  const data = objectParams(message.data);
  const toolUse = objectParams(data.tool_use);
  if (Object.keys(toolUse).length > 0) return [{
    type: "tool_use", call_id: String(toolUse.call_id ?? ""), tool_name: String(toolUse.tool_name ?? "tool"),
    arguments: JSON.stringify(toolUse.arguments ?? {}),
  }];
  return [{ type: "structured", media_type: "application/json", value: JSON.stringify(message.data ?? {}) }];
}

const daemon = new DaemonClient();

function createWindow(): void {
  const window = new BrowserWindow({
    width: 1480, height: 920, minWidth: 920, minHeight: 620,
    titleBarStyle: process.platform === "darwin" ? "hiddenInset" : "default",
    backgroundColor: "#111210", show: false,
    webPreferences: { preload: join(here, "preload.cjs"), contextIsolation: true, nodeIntegration: false, sandbox: true, webSecurity: true },
  });
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (url.startsWith("https://")) void shell.openExternal(url);
    return { action: "deny" };
  });
  window.webContents.on("will-navigate", (event) => event.preventDefault());
  void window.loadFile(join(here, "index.html"));
  window.once("ready-to-show", () => window.show());
}

app.whenReady().then(() => {
  void daemon.ensureStarted();
  ipcMain.handle("ait:request", (_event, method: unknown, params: unknown) => {
    if (typeof method !== "string") throw new Error("Unsupported desktop operation.");
    return daemon.request(method, params ?? {});
  });
  createWindow();
  app.on("activate", () => { if (BrowserWindow.getAllWindows().length === 0) createWindow(); });
});

app.on("window-all-closed", () => { if (process.platform !== "darwin") app.quit(); });
app.on("before-quit", () => daemon.stop());
