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
import { desktopDaemonRuntime, desktopProviderCatalog } from "./desktop-runtime.js";
import { registerDesktopProject, type ProjectCreationInput } from "./projects.js";
import { projectReadPaths } from "./desktop-slices.js";

const here = dirname(fileURLToPath(import.meta.url));
const daemonRuntime = desktopDaemonRuntime(app.isPackaged);
const endpoint = daemonRuntime.endpoint;
const allowedMethods = new Set([
  "provider.save", "provider.refresh-models", "provider.discover-models", "agent.save", "session.set-config",
  "project.list", "project.view", "agent.catalog", "settings.get", "settings.save", "settings.reset",
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

interface DaemonData {
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
    if (method === "project.list") return this.projectCatalog();
    if (method === "agent.catalog") return this.agentCatalog();
    if (method === "project.view") return this.projectView(boundedId(params.projectId, "Project"));
    if (method === "settings.get") return this.get("/v1/settings", "settings");
    if (method === "settings.save") return this.post("/v1/settings/save", "settings", {
      expected_revision: params.expectedRevision, values: params.values,
    });
    if (method === "settings.reset") return this.post("/v1/settings/reset", "settings", {});
    if (method === "provider.save") {
      await this.post("/v1/agent-provider/save", "agent_provider", { provider: params.provider, secret: params.secret });
      return this.agentCatalog();
    }
    if (method === "provider.discover-models") {
      return this.post("/v1/agent-provider/discover-models", "provider_models", {
        provider: params.provider, secret: params.secret,
      });
    }
    if (method === "provider.refresh-models") {
      await this.post("/v1/agent-provider/refresh-models", "agent_provider", { provider_id: params.providerId });
      return this.agentCatalog();
    }
    if (method === "agent.save") {
      await this.post(params.id ? "/v1/agent/update" : "/v1/agent/register", "agent", {
        id: params.id || randomUUID(), name: params.name, config: params.config,
      });
      return this.agentCatalog();
    }
    if (method === "session.set-config") {
      const projectId = boundedId(params.projectId, "Project");
      const session = await this.post("/v1/session/set-config", "session", {
        session_id: params.sessionId, config: params.config,
      });
      assertProject(session, projectId);
      const [project, agents] = await Promise.all([this.projectView(projectId), this.agentCatalog()]);
      return { project, agents };
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
      const projects = await this.get("/v1/project/list", "projects") as DaemonData["projects"];
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
      await registerDesktopProject(this.post.bind(this), id, params as unknown as ProjectCreationInput);
      const [catalog, project] = await Promise.all([this.projectCatalog(), this.projectView(id)]);
      return { catalog, project, selectedProjectId: id };
    }
    if (method === "project.set-default-agent") {
      const projectId = boundedId(params.projectId, "Project");
      await this.post("/v1/project/set-default-agent", "project", {
        project_id: projectId, agent_id: params.agentId,
      });
      return this.projectCatalog();
    }
    if (method === "session.create") {
      const projectId = boundedId(params.projectId, "Project");
      const id = randomUUID();
      const session = await this.post("/v1/session/create", "session", {
        id, project_id: projectId, agent_id: params.agentId,
      });
      assertProject(session, projectId);
      return { project: await this.projectView(projectId), selectedSessionId: id };
    }
    if (method === "session.set-agent") {
      const projectId = boundedId(params.projectId, "Project");
      const session = await this.post("/v1/session/set-agent", "session", {
        session_id: params.sessionId, agent_id: params.agentId,
      });
      assertProject(session, projectId);
      return this.projectView(projectId);
    }
    if (method === "session.rename") {
      const projectId = boundedId(params.projectId, "Project");
      const session = await this.post("/v1/session/rename", "session", {
        session_id: params.sessionId, name: params.name,
      });
      assertProject(session, projectId);
      return this.projectView(projectId);
    }
    if (method === "session.set-title") {
      const projectId = boundedId(params.projectId, "Project");
      const session = await this.post("/v1/session/set-title", "session", {
        session_id: params.sessionId, title: params.title,
      });
      assertProject(session, projectId);
      return this.projectView(projectId);
    }
    if (method === "session.generate-title") {
      const projectId = boundedId(params.projectId, "Project");
      const session = await this.post("/v1/session/generate-title", "session", {
        session_id: params.sessionId, prompt: params.prompt,
      });
      assertProject(session, projectId);
      return this.projectView(projectId);
    }
    if (method === "session.send-message") {
      const projectId = boundedId(params.projectId, "Project");
      const run = await this.post("/v1/session/submit-message", "run", {
        session_id: params.sessionId, text: params.content,
      }) as { id: string; project_id: string };
      assertProject(run, projectId);
      return { project: await this.projectView(projectId), runId: run.id };
    }
    if (method === "run.resolve-approval") {
      const projectId = boundedId(params.projectId, "Project");
      const runId = boundedId(params.runId, "Run");
      const approvalId = boundedId(params.approvalId, "approval");
      const action = approvalAction(params.action);
      const scope = approvalScope(params.scope, action);
      const run = await this.post("/v1/run/approval/resolve", "run", {
        run_id: runId,
        approval_id: approvalId,
        action,
        ...(scope ? { scope } : {}),
      });
      assertProject(run, projectId);
      return this.projectView(projectId);
    }

    const id = randomUUID();
    const currentSessionId = String(params.currentSessionId);
    const projectId = boundedId(params.projectId, "Project");
    const run = await this.post("/v1/session/submit-derive", "run", {
      id, project_id: projectId, source_session_id: currentSessionId,
      agent_id: params.agentId, at_message_id: params.sourceMessageId,
      text: params.content,
    }) as { id: string; project_id: string; session_id: string | null };
    assertProject(run, projectId);
    const selectedSessionId = run.session_id;
    if (selectedSessionId !== currentSessionId && selectedSessionId !== id) {
      throw new Error("Daemon returned an unexpected derived Session");
    }
    return {
      project: await this.projectView(projectId),
      selectedSessionId,
      runId: run.id,
      reusedCurrentSession: selectedSessionId === currentSessionId,
    };
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
    if (await this.isReady()) {
      throw new Error(`Ait refuses to reuse an unverified daemon already listening on ${daemonRuntime.listen}. Stop that daemon and try again.`);
    }
    const appRoot = resolve(here, "..");
    const workspaceRoot = resolve(appRoot, "../..");
    const executable = app.isPackaged
      ? join(process.resourcesPath, "bin", process.platform === "win32" ? "ait-daemon.exe" : "ait-daemon")
      : "cargo";
    const database = join(app.getPath("userData"), daemonRuntime.databaseFilename);
    const args = app.isPackaged
      ? ["--database", database, "--listen", daemonRuntime.listen]
      : ["run", "--quiet", "-p", "ait-daemon", "--features", "dev-mock-provider", "--", "--database", database, "--listen", daemonRuntime.listen];
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
      this.get("/v1/project/list", "projects") as Promise<DaemonData["projects"]>,
      this.get("/v1/agent/list", "agents") as Promise<DaemonData["agents"]>,
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
      const response = await fetch(`${endpoint}/v1/health`, { signal: AbortSignal.timeout(500) });
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

  private async projectCatalog(): Promise<unknown> {
    const projects = await this.get("/v1/project/list", "projects") as DaemonData["projects"];
    this.viewRevision += 1;
    return {
      protocolVersion: 1,
      revision: this.viewRevision,
      projects: projects.map((project) => ({
        id: project.id, name: project.name, workdir: project.workdir, description: "",
        repoUrl: project.repo_url ?? undefined, baseCommit: project.base_commit,
        defaultAgentId: project.default_agent_id ?? null,
      })),
    };
  }

  private async agentCatalog(): Promise<unknown> {
    const [agents, providers] = await Promise.all([
      this.get("/v1/agent/list", "agents") as Promise<DaemonData["agents"]>,
      this.get("/v1/agent-provider/list", "agent_providers") as Promise<DaemonData["providers"]>,
    ]);
    const catalog = desktopProviderCatalog(providers, agents, daemonRuntime.allowDevelopmentMock);
    this.viewRevision += 1;
    return {
      protocolVersion: 1,
      revision: this.viewRevision,
      agents: catalog.agents.map((agent) => projectAgent(agent, catalog.providers)),
      providers: catalog.providers,
    };
  }

  private async projectView(projectId: string): Promise<unknown> {
    const [sessionsPath, messagesPath, runsPath, progressPath] = projectReadPaths(projectId);
    const [sessions, messages, runs, progressValues] = await Promise.all([
      this.get(sessionsPath, "sessions") as Promise<DaemonData["sessions"]>,
      this.get(messagesPath, "messages") as Promise<DaemonData["messages"]>,
      this.get(runsPath, "runs") as Promise<DaemonData["runs"]>,
      fetch(`${endpoint}${progressPath}`).then(async (response) => {
        if (!response.ok) throw new Error(`Ait daemon returned HTTP ${response.status}.`);
        return response.json() as Promise<unknown[]>;
      }),
    ]);
    for (const record of [...sessions, ...messages, ...runs]) assertProject(record, projectId);
    const messageAgents = messageAgentIds(messages, runs);
    const activeRunIds = new Set(sessions.flatMap((session) => session.active_run_id ? [session.active_run_id] : []));
    const runProgress = progressValues
      .map(progressFromCheckpoint)
      .filter((progress): progress is RunProgress => progress !== undefined);
    for (const progress of runProgress) assertProject(progress, projectId);
    this.viewRevision += 1;
    return {
      protocolVersion: 1,
      revision: this.viewRevision,
      projectId,
      sessions: sessions.map((session) => ({
        id: session.id, projectId: session.project_id, name: session.name ?? "",
        title: sessionDisplayTitle(session), description: session.description ?? "",
        titleGenerationStarted: session.title_generation_started ?? false,
        currentMessageId: session.current_message_id, agentId: session.agent_id,
        version: session.version, active: session.active_run_id !== null,
        activeRunId: session.active_run_id, updatedAt: 0,
      })),
      messages: messages.map((message) => projectMessage(message, messageAgents.get(message.id) ?? null)),
      runs: runs.map((run) => ({
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
      runProgress: runProgress.filter((progress) => activeRunIds.has(progress.runId)),
      recoveryNotices: startupRecoveryNotices({ sessions, runs }),
    };
  }
}

function objectParams(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : {};
}

function assertProject(value: unknown, expectedProjectId: string): void {
  const record = objectParams(value);
  const actual = typeof record.project_id === "string" ? record.project_id : record.projectId;
  if (actual !== expectedProjectId) {
    throw new Error("Ait daemon returned data for an unexpected Project.");
  }
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
