import type { AgentProvider, AgentView, ControlEvent, RunProgress, RunStreamUpdate } from "./types.js";
import { app, BrowserWindow, dialog, ipcMain, shell } from "electron";
import { spawn, type ChildProcess } from "node:child_process";
import { randomUUID } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  builtInCodexAgentId,
  builtInCodexModel,
  legacyBuiltInCodexAgentId,
  projectAgent,
} from "./agents.js";
import { progressFromCheckpoint } from "./run-progress.js";
import { messageAgentIds, projectMessage, type WorkspaceMessage } from "./messages.js";
import { sessionDisplayTitle } from "./session-titles.js";
import { resolveProjectPath, vscodeFileUrl } from "./project-files.js";

const here = dirname(fileURLToPath(import.meta.url));
const endpoint = "http://127.0.0.1:7314";
const allowedMethods = new Set([
  "provider.save", "provider.refresh-models", "provider.discover-models", "agent.save", "session.set-config",
  "workspace.snapshot", "settings.get", "settings.save", "settings.reset",
  "project.choose-directory", "project.open-file", "project.create", "project.set-default-agent",
  "session.create", "session.set-agent", "session.rename", "session.set-title",
  "session.generate-title", "session.send-message", "session.fork",
]);
interface DaemonResponse {
  ok: boolean;
  result?: { kind: string; value: unknown };
  error?: { code: string; message: string };
}

interface WorkspaceView {
  projects: Array<{
    id: string; name: string; workdir: string; repo_url?: string | null;
    base_commit: string; default_agent_id?: string | null;
  }>;
  agents: AgentView[];
  providers: AgentProvider[];
  sessions: Array<{
    id: string; project_id: string; name?: string; title?: string | null; description?: string;
    title_generation_started?: boolean; agent_id: string; current_message_id: string;
    active_run_id: string | null; version: number;
  }>;
  messages: WorkspaceMessage[];
  runs: Array<{
    id: string; project_id: string; session_id: string | null; agent_id: string;
    base_message_id: string; last_message_id: string | null; status: string;
    error?: { code?: string; message?: string } | null;
  }>;
}

class DaemonClient {
  private ownedProcess: ChildProcess | undefined;
  private startup: Promise<void> | undefined;
  private snapshotRevision = 0;
  private eventAbort: AbortController | undefined;
  private eventLoop: Promise<void> | undefined;
  private eventCursor = 0;
  private streamConnected: boolean | undefined;

  ensureStarted(): Promise<void> {
    this.startup ??= this.start().then(() => this.ensureBuiltInAgents()).then(() => {
      this.startEventStream();
    });
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
    if (method === "provider.save") {
      await this.post("/v1/agent-provider/save", "agent_provider", { provider: params.provider, secret: params.secret });
      return this.snapshot();
    }
    if (method === "provider.discover-models") {
      return this.post("/v1/agent-provider/discover-models", "provider_models", {
        provider: params.provider, secret: params.secret,
      });
    }
    if (method === "provider.refresh-models") {
      await this.post("/v1/agent-provider/refresh-models", "agent_provider", { provider_id: params.providerId });
      return this.snapshot();
    }
    if (method === "agent.save") {
      await this.post(params.id ? "/v1/agent/update" : "/v1/agent/register", "agent", {
        id: params.id || randomUUID(), name: params.name, config: params.config,
      });
      return this.snapshot();
    }
    if (method === "session.set-config") {
      await this.post("/v1/session/set-config", "session", { session_id: params.sessionId, config: params.config });
      return this.snapshot();
    }
    if (method === "project.choose-directory") {
      const result = await dialog.showOpenDialog({
        title: "Choose a Project directory",
        properties: ["openDirectory", "createDirectory"],
      });
      return result.canceled ? null : result.filePaths[0] ?? null;
    }
    if (method === "project.open-file") {
      const projectId = typeof params.projectId === "string" ? params.projectId : "";
      const reference = typeof params.path === "string" ? params.path : "";
      const workspace = await this.get("/v1/workspace/snapshot", "workspace") as WorkspaceView;
      const project = workspace.projects.find((candidate) => candidate.id === projectId);
      if (!project) throw new Error("Project not found.");
      const path = await resolveProjectPath(project.workdir, reference);
      const line = positiveInteger(params.line);
      const column = positiveInteger(params.column) ?? 1;
      if (line) {
        try {
          await shell.openExternal(vscodeFileUrl(path, line, column));
          return { positioned: true };
        } catch {
          // Fall through to the system's default application when VS Code is unavailable.
        }
      }
      const failure = await shell.openPath(path);
      if (failure) throw new Error(failure);
      return { positioned: false };
    }
    if (method === "project.create") {
      const id = randomUUID();
      await this.post("/v1/project/register", "project", {
        id, name: params.name, workdir: params.workdir, repo_url: params.repoUrl,
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
    if (method === "session.set-agent") {
      await this.post("/v1/session/set-agent", "session", {
        session_id: params.sessionId, agent_id: params.agentId,
      });
      return this.snapshot();
    }
    if (method === "session.rename") {
      await this.post("/v1/session/rename", "session", {
        session_id: params.sessionId, name: params.name,
      });
      return this.snapshot();
    }
    if (method === "session.set-title") {
      await this.post("/v1/session/set-title", "session", {
        session_id: params.sessionId, title: params.title,
      });
      return this.snapshot();
    }
    if (method === "session.generate-title") {
      await this.post("/v1/session/generate-title", "session", {
        session_id: params.sessionId, prompt: params.prompt,
      });
      return this.snapshot();
    }
    if (method === "session.send-message") {
      await this.post("/v1/session/submit-message", "run", {
        session_id: params.sessionId, text: params.content,
      });
      return this.snapshot();
    }

    const id = randomUUID();
    await this.post("/v1/session/submit-fork", "run", {
      id, project_id: params.projectId, agent_id: params.agentId,
      at_message_id: params.sourceMessageId, text: params.content,
    });
    return { snapshot: await this.snapshot(), selectedSessionId: id };
  }

  stop(): void {
    this.eventAbort?.abort();
    this.eventAbort = undefined;
    this.eventLoop = undefined;
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
    if (!workspace.agents.some((agent) => agent.id === builtInCodexAgentId)) {
      await this.post("/v1/agent/register", "agent", {
        id: builtInCodexAgentId,
        name: "Codex",
        config: { provider_id: "builtin-codex", model: builtInCodexModel, reasoning_effort: "low" },
      });
    }
    await Promise.all(workspace.projects
      .filter((project) => project.default_agent_id === legacyBuiltInCodexAgentId)
      .map((project) => this.post("/v1/project/set-default-agent", "project", {
        project_id: project.id, agent_id: builtInCodexAgentId,
      })));
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

  private startEventStream(): void {
    if (this.eventLoop) return;
    this.eventAbort = new AbortController();
    this.eventLoop = this.followEvents(this.eventAbort.signal);
  }

  private async followEvents(signal: AbortSignal): Promise<void> {
    let retryDelay = 250;
    while (!signal.aborted) {
      try {
        const response = await fetch(`${endpoint}/v1/event/stream?after=${this.eventCursor}`, { signal });
        if (!response.ok || !response.body) throw new Error(`event stream returned HTTP ${response.status}`);
        this.publishConnection(true);
        retryDelay = 250;
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        while (!signal.aborted) {
          const chunk = await reader.read();
          buffer += decoder.decode(chunk.value, { stream: !chunk.done }).replace(/\r\n/g, "\n");
          let boundary = buffer.indexOf("\n\n");
          while (boundary >= 0) {
            const frame = buffer.slice(0, boundary);
            buffer = buffer.slice(boundary + 2);
            this.acceptEventFrame(frame);
            boundary = buffer.indexOf("\n\n");
          }
          if (chunk.done) break;
        }
        if (!signal.aborted) throw new Error("event stream ended");
      } catch (error) {
        if (signal.aborted) break;
        console.error(`[ait-daemon] progress stream disconnected: ${error instanceof Error ? error.message : String(error)}`);
        this.publishConnection(false);
        await new Promise((resolveDelay) => setTimeout(resolveDelay, retryDelay));
        retryDelay = Math.min(5_000, retryDelay * 2);
      }
    }
  }

  private acceptEventFrame(frame: string): void {
    const data = frame.split("\n")
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trimStart())
      .join("\n");
    if (!data) return;
    try {
      const event = JSON.parse(data) as ControlEvent;
      if (!Number.isSafeInteger(event.cursor) || typeof event.kind !== "string") return;
      this.eventCursor = Math.max(this.eventCursor, event.cursor);
      this.publish({ type: "event", event });
    } catch {
      // A malformed frame is ignored; reconnect replay remains authoritative.
    }
  }

  private publishConnection(connected: boolean): void {
    if (this.streamConnected === connected) return;
    this.streamConnected = connected;
    this.publish({ type: "connection", connected });
  }

  private publish(update: RunStreamUpdate): void {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed()) window.webContents.send("ait:run-event", update);
    }
  }

  private async snapshot(): Promise<unknown> {
    const [workspace, progressValues] = await Promise.all([
      this.get("/v1/workspace/snapshot", "workspace") as Promise<WorkspaceView>,
      fetch(`${endpoint}/v1/run/progress`).then(async (response) => {
        if (!response.ok) throw new Error(`Ait daemon returned HTTP ${response.status}.`);
        return response.json() as Promise<unknown[]>;
      }),
    ]);
    const messageAgents = messageAgentIds(workspace.messages, workspace.runs);
    const activeRunIds = new Set(workspace.sessions.flatMap((session) => session.active_run_id ? [session.active_run_id] : []));
    const runProgress = progressValues
      .map(progressFromCheckpoint)
      .filter((progress): progress is RunProgress => progress !== undefined && activeRunIds.has(progress.runId));
    this.snapshotRevision += 1;
    return {
      protocolVersion: 1,
      revision: this.snapshotRevision,
      projects: workspace.projects.map((project) => ({
        id: project.id, name: project.name, workdir: project.workdir, description: "",
        repoUrl: project.repo_url ?? undefined, baseCommit: project.base_commit,
        defaultAgentId: project.default_agent_id ?? null,
      })),
      agents: workspace.agents.map((agent) => projectAgent(agent, workspace.providers)),
      providers: workspace.providers,
      sessions: workspace.sessions.map((session) => ({
        id: session.id, projectId: session.project_id, name: session.name ?? "",
        title: sessionDisplayTitle(session), description: session.description ?? "",
        titleGenerationStarted: session.title_generation_started ?? false,
        currentMessageId: session.current_message_id, agentId: session.agent_id,
        version: session.version, active: session.active_run_id !== null,
        activeRunId: session.active_run_id, updatedAt: 0,
      })),
      messages: workspace.messages.map((message) => projectMessage(message, messageAgents.get(message.id) ?? null)),
      runs: workspace.runs.map((run) => ({
        id: run.id,
        sessionId: run.session_id,
        baseMessageId: run.base_message_id,
        lastMessageId: run.last_message_id,
        status: run.status,
        ...(run.error?.message ? {
          error: { message: run.error.message, ...(run.error.code ? { code: run.error.code } : {}) },
        } : {}),
      })),
      runProgress,
    };
  }
}

function objectParams(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : {};
}

function positiveInteger(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0 ? value : undefined;
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
