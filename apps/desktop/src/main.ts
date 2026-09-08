import type { AgentProvider, AgentView, ControlEvent, NativeApproval, RunProgress, RunStreamUpdate } from "./types.js";
import { app, BrowserWindow, dialog, ipcMain, shell, type WebContents } from "electron";
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
import { ReadyRunEventDelivery, cursorAfterEvent } from "./run-event-delivery.js";
import { startupRecoveryNotices } from "./runs.js";
import { messageAgentIds, projectMessage, type WorkspaceMessage } from "./messages.js";
import { sessionDisplayTitle } from "./session-titles.js";
import { resolveProjectPath, vscodeFileUrl } from "./project-files.js";
import { approvalAction, approvalScope } from "./approval-ui.js";

const here = dirname(fileURLToPath(import.meta.url));
const endpoint = "http://127.0.0.1:7314";
const allowedMethods = new Set([
  "provider.save", "provider.refresh-models", "provider.discover-models", "agent.save", "session.set-config",
  "workspace.view", "settings.get", "settings.save", "settings.reset",
  "project.choose-directory", "project.open-file", "project.create", "project.set-default-agent",
  "session.create", "session.set-agent", "session.rename", "session.set-title",
  "session.generate-title", "session.send-message", "session.fork",
  "run.resolve-approval",
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
    permission_profile: { sandbox: "read_only" | "workspace_write" | "full_access"; approval: "on_request" | "untrusted_only" };
    native_approvals?: Array<{
      id: string; run_id: string; protocol_request_id: string | number; method: string; kind: string;
      thread_id: string; turn_id: string; item_id: string;
      target: NativeApproval["target"];
      requested_permissions?: Record<string, unknown>; status: string; granted_scope?: string;
      granted_permissions?: Record<string, unknown>; created_at: number; decided_at?: number;
    }>;
    error?: { code?: string; message?: string } | null;
  }>;
}

class DaemonClient {
  private ownedProcess: ChildProcess | undefined;
  private startup: Promise<void> | undefined;
  private viewRevision = 0;
  private viewQueue: Promise<void> = Promise.resolve();
  private viewedProjectId: string | undefined;
  private eventAbort: AbortController | undefined;
  private eventLoop: Promise<void> | undefined;
  private eventCursor = 0;
  private streamConnected: boolean | undefined;
  private readonly deliveries = new Map<number, ReadyRunEventDelivery>();

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
    if (method === "workspace.view") {
      return this.view(typeof params.projectId === "string" ? params.projectId : undefined);
    }
    if (method === "settings.get") return this.get("/v1/settings", "settings");
    if (method === "settings.save") return this.post("/v1/settings/save", "settings", {
      expected_revision: params.expectedRevision, values: params.values,
    });
    if (method === "settings.reset") return this.post("/v1/settings/reset", "settings", {});
    if (method === "provider.save") {
      await this.post("/v1/agent-provider/save", "agent_provider", { provider: params.provider, secret: params.secret });
      return this.view();
    }
    if (method === "provider.discover-models") {
      return this.post("/v1/agent-provider/discover-models", "provider_models", {
        provider: params.provider, secret: params.secret,
      });
    }
    if (method === "provider.refresh-models") {
      await this.post("/v1/agent-provider/refresh-models", "agent_provider", { provider_id: params.providerId });
      return this.view();
    }
    if (method === "agent.save") {
      await this.post(params.id ? "/v1/agent/update" : "/v1/agent/register", "agent", {
        id: params.id || randomUUID(), name: params.name, config: params.config,
      });
      return this.view();
    }
    if (method === "session.set-config") {
      await this.post("/v1/session/set-config", "session", { session_id: params.sessionId, config: params.config });
      return this.view();
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
      const projects = await this.get("/v1/project/list", "projects") as WorkspaceView["projects"];
      const project = projects.find((candidate) => candidate.id === projectId);
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
      return { view: await this.view(id), selectedProjectId: id };
    }
    if (method === "project.set-default-agent") {
      await this.post("/v1/project/set-default-agent", "project", {
        project_id: params.projectId, agent_id: params.agentId,
      });
      return this.view(String(params.projectId));
    }
    if (method === "session.create") {
      const id = randomUUID();
      await this.post("/v1/session/create", "session", {
        id, project_id: params.projectId, agent_id: params.agentId,
      });
      return { view: await this.view(String(params.projectId)), selectedSessionId: id };
    }
    if (method === "session.set-agent") {
      await this.post("/v1/session/set-agent", "session", {
        session_id: params.sessionId, agent_id: params.agentId,
      });
      return this.view();
    }
    if (method === "session.rename") {
      await this.post("/v1/session/rename", "session", {
        session_id: params.sessionId, name: params.name,
      });
      return this.view();
    }
    if (method === "session.set-title") {
      await this.post("/v1/session/set-title", "session", {
        session_id: params.sessionId, title: params.title,
      });
      return this.view();
    }
    if (method === "session.generate-title") {
      await this.post("/v1/session/generate-title", "session", {
        session_id: params.sessionId, prompt: params.prompt,
      });
      return this.view();
    }
    if (method === "session.send-message") {
      const run = await this.post("/v1/session/submit-message", "run", {
        session_id: params.sessionId, text: params.content,
      }) as { id: string };
      return { view: await this.view(), runId: run.id };
    }
    if (method === "run.resolve-approval") {
      const runId = boundedId(params.runId, "Run");
      const approvalId = boundedId(params.approvalId, "approval");
      const action = approvalAction(params.action);
      const scope = approvalScope(params.scope, action);
      await this.post("/v1/run/approval/resolve", "run", {
        run_id: runId,
        approval_id: approvalId,
        action,
        ...(scope ? { scope } : {}),
      });
      return this.view();
    }

    const id = randomUUID();
    const run = await this.post("/v1/session/submit-fork", "run", {
      id, project_id: params.projectId, agent_id: params.agentId,
      at_message_id: params.sourceMessageId, text: params.content,
    }) as { id: string };
    return { view: await this.view(String(params.projectId)), selectedSessionId: id, runId: run.id };
  }

  stop(): void {
    this.eventAbort?.abort();
    this.eventAbort = undefined;
    this.eventLoop = undefined;
    for (const delivery of this.deliveries.values()) delivery.close();
    this.deliveries.clear();
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
    const [projects, agents] = await Promise.all([
      this.get("/v1/project/list", "projects") as Promise<WorkspaceView["projects"]>,
      this.get("/v1/agent/list", "agents") as Promise<WorkspaceView["agents"]>,
    ]);
    if (!agents.some((agent) => agent.id === builtInCodexAgentId)) {
      await this.post("/v1/agent/register", "agent", {
        id: builtInCodexAgentId,
        name: "Codex",
        config: { provider_id: "builtin-codex", model: builtInCodexModel, reasoning_effort: "low" },
      });
    }
    await Promise.all(projects
      .filter((project) => project.default_agent_id === legacyBuiltInCodexAgentId)
      .map((project) => this.post("/v1/project/set-default-agent", "project", {
        project_id: project.id, agent_id: builtInCodexAgentId,
      })));
  }

  private async isReady(): Promise<boolean> {
    try {
      const response = await fetch(`${endpoint}/v1/project/list`, { signal: AbortSignal.timeout(500) });
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
      this.eventCursor = cursorAfterEvent(this.eventCursor, event);
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
      if (window.isDestroyed()) continue;
      this.deliveries.get(window.webContents.id)?.enqueue(update);
    }
  }

  rendererReady(webContents: WebContents, generation: string): void {
    if (!generation || generation.length > 128 || webContents.isDestroyed()) return;
    let delivery = this.deliveries.get(webContents.id);
    if (!delivery) {
      delivery = new ReadyRunEventDelivery((frame) => {
        if (!webContents.isDestroyed()) webContents.send("ait:run-event-frame", frame);
      });
      this.deliveries.set(webContents.id, delivery);
    }
    delivery.ready(generation, [
      { type: "connection", connected: this.streamConnected ?? false },
      { type: "resync", cursor: this.eventCursor },
    ]);
  }

  rendererLoading(webContentsId: number): void {
    this.deliveries.get(webContentsId)?.suspend();
  }

  removeRenderer(webContentsId: number): void {
    this.deliveries.get(webContentsId)?.close();
    this.deliveries.delete(webContentsId);
  }

  acknowledge(webContentsId: number, generation: string, frameId: number): void {
    if (Number.isSafeInteger(frameId)) {
      this.deliveries.get(webContentsId)?.acknowledge(generation, frameId);
    }
  }

  private view(projectId?: string): Promise<unknown> {
    const result = this.viewQueue.then(() => this.readView(projectId));
    this.viewQueue = result.then(() => undefined, () => undefined);
    return result;
  }

  private async readView(projectId?: string): Promise<unknown> {
    const [projects, agents, providers, sessions, progressValues] = await Promise.all([
      this.get("/v1/project/list", "projects") as Promise<WorkspaceView["projects"]>,
      this.get("/v1/agent/list", "agents") as Promise<WorkspaceView["agents"]>,
      this.get("/v1/agent-provider/list", "agent_providers") as Promise<WorkspaceView["providers"]>,
      this.get("/v1/session/list", "sessions") as Promise<WorkspaceView["sessions"]>,
      fetch(`${endpoint}/v1/run/progress`).then(async (response) => {
        if (!response.ok) throw new Error(`Ait daemon returned HTTP ${response.status}.`);
        return response.json() as Promise<unknown[]>;
      }),
    ]);
    const candidate = projectId ?? this.viewedProjectId;
    const selectedProjectId = projects.some((project) => project.id === candidate)
      ? candidate
      : sessions.at(-1)?.project_id ?? projects[0]?.id;
    this.viewedProjectId = selectedProjectId;
    const [messages, runs] = selectedProjectId
      ? await Promise.all([
        this.get(`/v1/message/list?project_id=${encodeURIComponent(selectedProjectId)}`, "messages") as Promise<WorkspaceView["messages"]>,
        this.get(`/v1/run/list?project_id=${encodeURIComponent(selectedProjectId)}`, "runs") as Promise<WorkspaceView["runs"]>,
      ])
      : [[], []];
    const workspace: WorkspaceView = { projects, agents, providers, sessions, messages, runs };
    const messageAgents = messageAgentIds(workspace.messages, workspace.runs);
    const activeRunIds = new Set(workspace.sessions.flatMap((session) => session.active_run_id ? [session.active_run_id] : []));
    const runProgress = progressValues
      .map(progressFromCheckpoint)
      .filter((progress): progress is RunProgress => progress !== undefined && activeRunIds.has(progress.runId));
    this.viewRevision += 1;
    return {
      protocolVersion: 1,
      revision: this.viewRevision,
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
        permissionProfile: run.permission_profile,
        nativeApprovals: (run.native_approvals ?? []).map((approval) => ({
          id: approval.id,
          runId: approval.run_id,
          protocolRequestId: approval.protocol_request_id,
          method: approval.method,
          kind: approval.kind,
          threadId: approval.thread_id,
          turnId: approval.turn_id,
          itemId: approval.item_id,
          target: approval.target,
          ...(approval.requested_permissions ? { requestedPermissions: approval.requested_permissions } : {}),
          status: approval.status,
          ...(approval.granted_scope ? { grantedScope: approval.granted_scope } : {}),
          ...(approval.granted_permissions ? { grantedPermissions: approval.granted_permissions } : {}),
          createdAt: approval.created_at,
          ...(approval.decided_at !== undefined ? { decidedAt: approval.decided_at } : {}),
        })),
        ...(run.error?.message ? {
          error: { message: run.error.message, ...(run.error.code ? { code: run.error.code } : {}) },
        } : {}),
      })),
      runProgress,
      recoveryNotices: startupRecoveryNotices(workspace),
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

function boundedId(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0 || value.length > 512) {
    throw new Error(`${label} identifier is invalid.`);
  }
  return value;
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
  window.webContents.on("did-start-loading", () => daemon.rendererLoading(window.webContents.id));
  window.webContents.once("destroyed", () => daemon.removeRenderer(window.webContents.id));
  void window.loadFile(join(here, "index.html"));
  window.once("ready-to-show", () => window.show());
}

app.whenReady().then(() => {
  void daemon.ensureStarted();
  ipcMain.handle("ait:request", (_event, method: unknown, params: unknown) => {
    if (typeof method !== "string") throw new Error("Unsupported desktop operation.");
    return daemon.request(method, params ?? {});
  });
  ipcMain.on("ait:run-event-ready", (event, generation: unknown) => {
    const senderFrame = event.senderFrame;
    const mainFrame = event.sender.mainFrame;
    const fromMainFrame = senderFrame !== null
      && senderFrame.processId === mainFrame.processId
      && senderFrame.routingId === mainFrame.routingId;
    if (typeof generation === "string" && fromMainFrame) {
      daemon.rendererReady(event.sender, generation);
    }
  });
  ipcMain.on("ait:run-event-ack", (event, generation: unknown, frameId: unknown) => {
    if (typeof generation === "string" && typeof frameId === "number") {
      daemon.acknowledge(event.sender.id, generation, frameId);
    }
  });
  createWindow();
  app.on("activate", () => { if (BrowserWindow.getAllWindows().length === 0) createWindow(); });
});

app.on("window-all-closed", () => { if (process.platform !== "darwin") app.quit(); });
app.on("before-quit", () => daemon.stop());
