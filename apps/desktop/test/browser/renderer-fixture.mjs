/** Isolated preload bridge: no daemon, provider, credentials or filesystem mutations. */
export function installRendererFixture() {
  const config = { provider_id: "codex", model: "fixture-model", reasoning_effort: null };
  const agent = { id: "agent", name: "Fixture Agent", model: config.model, mode: "codex", enabled: true, config, ownerSessionId: null };
  const projects = ["a", "b"].map((id) => ({ id, name: `Project ${id.toUpperCase()}`, workdir: `/fixture/${id}`, description: "", baseCommit: "a".repeat(40), rootMessageId: `root-${id}`, defaultAgentId: "agent" }));
  const session = (projectId, id, title, activeRunId = null) => ({
    id, projectId, workdir: `/fixture/${projectId}/${id}`, name: "", title, description: "",
    titleGenerationStarted: true, currentMessageId: `message-${projectId}`, agentId: "agent", version: 1,
    active: activeRunId !== null, activeRunId, updatedAt: 1,
  });
  const run = (id, sessionId, status = "running") => ({ id, sessionId, baseMessageId: "root", lastMessageId: null,
    status, permissionProfile: { sandbox: "workspace_write", approval: "on_request" }, nativeApprovals: [],
  });
  const f = window.fixture = {
    projects, agents: [agent, { ...agent, id: "alternate", name: "Alternate Agent" }], edits: [], created: [], sessionReads: [], updateFailure: false, sessionFailure: false,
    sessions: [session("a", "session-a", "Session A"), session("b", "session-b", "Session B", "run-b")],
    runs: [run("run-b", "session-b")], sent: [], forks: [], forkFailure: false, projectReads: [],
    catalogFailure: false, catalogDelay: false, viewFailure: false,
    emit: () => {}, releaseCatalog: () => {},
    async catalogRead() {
      if (f.catalogDelay) await new Promise((resolve) => { f.releaseCatalog = resolve; });
      if (f.catalogFailure) throw new Error("Project catalog unavailable");
      return { protocolVersion: 1, revision: 1, projects: structuredClone(projects) };
    },
    view(projectId) {
      return { protocolVersion: 1, revision: 1, projectId,
        sessions: structuredClone(f.sessions.filter((s) => s.projectId === projectId)),
        messages: [
          { id: `root-${projectId}`, projectId, parentMessageId: null, role: "system", kind: "standard", parts: [{ type: "text", text: "System" }], createdAt: 1 },
          { id: `message-${projectId}`, projectId, parentMessageId: `root-${projectId}`, role: "user", kind: "standard", parts: [{ type: "text", text: `Message ${projectId}` }], createdAt: 2 },
        ],
        runs: structuredClone(f.runs.filter((r) => f.sessions.some((s) => s.id === r.sessionId && s.projectId === projectId))), runProgress: [], recoveryNotices: [],
      };
    },
    finish(runId, status = "completed", emit = true) {
      const r = f.runs.find((r) => r.id === runId);
      r.status = status;
      if (status !== "completed") r.error = { message: `Fixture ${status}` };
      const s = f.sessions.find((s) => s.id === r.sessionId);
      s.active = false; s.activeRunId = null;
      if (emit) f.emit([{ type: "event", event: { api_version: 1, cursor: 10, kind: "run.updated", entity_id: r.id, body: { id: r.id, project_id: s.projectId, session_id: s.id, status, error: r.error }, created_at: 10 } }]);
    },
  };
  window.ait = {
    projects: () => f.catalogRead(),
    agents: async () => ({ protocolVersion: 1, revision: 1, agents: structuredClone(f.agents), providers: [{ id: "codex", name: "Codex", kind: "codex", url: null, has_secret: false, models: [{ id: config.model, name: config.model, reasoning_efforts: [] }] }] }),
    settings: async () => ({ schema: { revision: 1, definitions: [] }, values: { "interface.theme": "dark", "permissions.sandbox": "workspace_write" }, revision: 1 }),
    project: async (projectId) => {
      f.projectReads.push(projectId);
      if (f.viewFailure) throw new Error("Project view unavailable");
      return f.view(projectId);
    },
    projectSessions: async (projectId) => {
      f.sessionReads.push(projectId);
      if (f.sessionFailure) throw new Error("Sessions unavailable");
      return structuredClone(f.sessions.filter((s) => s.projectId === projectId));
    },
    updateProject: async (input) => {
      f.edits.push(input);
      if (f.updateFailure) throw new Error("Project update failed");
      const project = projects.find((p) => p.id === input.projectId);
      project.name = input.name;
      if (input.agentId) project.defaultAgentId = input.agentId;
      return f.catalogRead();
    },
    renameSession: async ({ projectId, sessionId, name }) => {
      const target = f.sessions.find((s) => s.projectId === projectId && s.id === sessionId);
      target.name = name; target.title = name;
      return f.view(projectId);
    },
    activeRuns: async () => ({ unavailableProjects: [], runs: f.runs.filter((r) => ["queued", "running"].includes(r.status)).map((r) => {
      const s = f.sessions.find((s) => s.id === r.sessionId);
      return { id: r.id, projectId: s.projectId, projectName: projects.find((p) => p.id === s.projectId).name,
        sessionId: s.id, sessionTitle: s.title, agentId: "agent", model: config.model, status: r.status, phase: "calling_agent", trigger: "manual", pendingApprovals: 0 };
    }) }),
    subscribeRunEvents: (listener) => { f.emit = listener; return () => {}; },
    sendMessage: async (input) => {
      f.sent.push(input);
      return { project: f.view(input.projectId), runId: "sent" };
    },
    fork: async (input) => {
      f.forks.push(input);
      if (f.forkFailure) throw new Error("First message rejected");
      const id = input.currentSessionId ? "derived" : "created";
      const created = session(input.projectId, id, input.currentSessionId ? "Derived Session" : "Created Session", `run-${id}`);
      created.agentId = input.agentId;
      f.sessions.push(created);
      f.runs.push(run(`run-${id}`, id));
      if (!input.currentSessionId) f.created.push(input);
      return { project: f.view(input.projectId), runId: `run-${id}`, selectedSessionId: id, reusedCurrentSession: false };
    },
  };
}
