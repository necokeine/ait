export type MessageRole = "user" | "system" | "assistant";
export type MessageKind = "standard" | "tool_result";
export type ReasoningEffort = string;

export interface AgentConfiguration {
  provider_id: string;
  model: string;
  reasoning_effort: string | null;
  system_prompt?: string | null;
}
export interface ProviderModel { id: string; name: string; reasoning_efforts: string[] }
export interface AgentProvider { id: string; name: string; kind: string; url: string | null; models: ProviderModel[]; has_secret: boolean }
export interface ProviderInput { provider: Omit<AgentProvider, "has_secret">; secret?: string }
export interface AgentView { id: string; name: string; config: AgentConfiguration; owner_session_id: string | null; revision: number; enabled: boolean }

export type MessagePart =
  | { type: "text"; text: string }
  | { type: "codex_message"; id: string; phase: string; text: string }
  | { type: "file"; name: string; media_type: string }
  | { type: "tool_use"; call_id: string; tool_name: string; arguments: string }
  | {
    type: "operation";
    id: string;
    kind: string;
    status: string;
    title: string;
    summary?: string;
    detail?: string;
    paths: string[];
  }
  | { type: "structured"; media_type: string; value: string }
  | { type: "tool_result"; call_id: string; status: string; output: string | null; error: string | null }
  | { type: "redacted" };

export interface DesktopMessage {
  id: string;
  projectId: string;
  parentMessageId: string | null;
  role: MessageRole;
  kind: MessageKind;
  parts: MessagePart[];
  gitCommit?: string;
  createdAt: number;
  agentId?: string | null;
}

export interface DesktopProject {
  id: string;
  name: string;
  workdir: string;
  description: string;
  repoUrl?: string;
  baseCommit: string;
  rootMessageId: string;
  defaultAgentId: string | null;
}

export interface AgentSummary {
  id: string;
  name: string;
  model: string;
  mode: string;
  enabled: boolean;
  supportedReasoningEfforts?: ReasoningEffort[];
  config: AgentConfiguration;
  ownerSessionId: string | null;
}

export interface DesktopSession {
  id: string;
  projectId: string;
  workdir: string;
  name: string;
  title: string;
  description: string;
  titleGenerationStarted: boolean;
  currentMessageId: string;
  agentId: string;
  version: number;
  active: boolean;
  activeRunId: string | null;
  updatedAt: number;
}

export type RunProgressItem =
  | Extract<MessagePart, { type: "codex_message" }>
  | Extract<MessagePart, { type: "operation" }>;

export interface RunProgress {
  runId: string;
  projectId: string;
  sessionId: string | null;
  seq: number;
  status: string;
  items: RunProgressItem[];
  warnings: Array<{ message: string; retrying: boolean; code?: string }>;
  updatedAt: number;
}

export interface DesktopRun {
  id: string;
  sessionId: string | null;
  baseMessageId: string;
  lastMessageId: string | null;
  status: string;
  permissionProfile: {
    sandbox: "read_only" | "workspace_write" | "full_access";
    approval: "on_request" | "untrusted_only";
  };
  nativeApprovals: NativeApproval[];
  toolApprovals?: ToolApproval[];
  toolInteractions?: ToolInteraction[];
  agentName?: string;
  providerName?: string;
  error?: { code?: string; message: string };
}

export interface DesktopCron {
  id: string;
  name: string;
  projectId: string;
  baseMessageId: string;
  agentId: string;
  schedule: string;
  timezone: string;
  enabled: boolean;
}

export interface CronRunSubmission {
  project: ProjectView;
  runId: string;
  selectedSessionId: string;
}

export interface ToolApproval {
  grant: {
    request_id: string; run_id: string; execution_id: string; call_id: string;
    expires_at: number;
    target: { tool_name: string; cwd: string; operation: string; reason: string;
      current: "read_only" | "workspace_write" | "full_access";
      requested: "read_only" | "workspace_write" | "full_access" };
  };
  status: "pending" | "approved" | "consumed" | "denied" | "cancelled" | "expired";
}

export interface ToolInteraction {
  id: string;
  toolName: "question" | "plan_exit";
  request: Record<string, unknown>;
  response?: unknown;
  status: "pending" | "answered" | "approved" | "denied" | "cancelled" | "expired";
  expiresAt: number;
  createdAt: number;
  decidedAt?: number;
}

export interface NativeApproval {
  id: string;
  runId: string;
  protocolRequestId: string | number;
  method: string;
  kind: "command_execution" | "file_change" | "permissions" | "legacy_command" | "legacy_patch";
  threadId: string;
  turnId: string;
  itemId: string;
  target:
    | { type: "command"; command: string; cwd: string }
    | { type: "network"; host: string; protocol: "http" | "https" | "socks5Tcp" | "socks5Udp" }
    | { type: "file_change"; grant_root?: string; changes: Array<{ path: string; kind: "add" | "delete" | "update" }> }
    | { type: "permissions"; cwd: string };
  requestedPermissions?: Record<string, unknown>;
  status: "pending" | "approved" | "denied" | "cancelled" | "expired";
  grantedScope?: "one_shot" | "turn" | "session";
  grantedPermissions?: Record<string, unknown>;
  createdAt: number;
  decidedAt?: number;
}

export interface ControlEvent {
  api_version: number;
  cursor: number;
  kind: string;
  entity_id: string | null;
  body: unknown;
  created_at: number;
}

export type RunStreamUpdate =
  | { type: "event"; event: ControlEvent }
  | { type: "connection"; connected: boolean }
  | { type: "resync"; cursor: number };

export interface RunStreamFrame {
  generation: string;
  id: number;
  updates: RunStreamUpdate[];
}

export interface RunSubmission {
  project: ProjectView;
  runId: string;
}

export interface ForkReceipt {
  status: "accepted";
  selectedSessionId: string;
  runId: string;
  reusedCurrentSession: boolean;
}

export type ForkSubmission = ForkReceipt | { status: "rejected" | "unknown"; message: string };

/** Workspace activity summary; excludes conversation content and execution payloads. */
export interface ActiveRunSummary {
  id: string;
  projectId: string;
  projectName: string;
  sessionId: string | null;
  sessionTitle: string | null;
  agentId: string;
  model: string;
  status: string;
  phase: string | null;
  trigger: string;
  pendingApprovals: number;
}

export interface ActiveRunsCatalog {
  runs: ActiveRunSummary[];
  unavailableProjects: Array<{ projectId: string; projectName: string; message: string }>;
}

export interface ProjectCatalog {
  protocolVersion: number;
  revision: number;
  projects: DesktopProject[];
}

export interface AgentCatalog {
  protocolVersion: number;
  revision: number;
  agents: AgentSummary[];
  providers: AgentProvider[];
}

export interface ProjectView {
  protocolVersion: number;
  revision: number;
  projectId: string;
  sessions: DesktopSession[];
  messages: DesktopMessage[];
  runs: DesktopRun[];
  runProgress: RunProgress[];
  recoveryNotices?: Array<{
    projectId: string;
    sessionId?: string;
    sessionTitle?: string;
    runId: string;
    code?: string;
    message: string;
  }>;
}

/** Renderer-owned composition; never crosses the Electron bridge. */
export interface DesktopState {
  projects: DesktopProject[];
  agents: AgentSummary[];
  providers: AgentProvider[];
  sessions: DesktopSession[];
  messages: DesktopMessage[];
  runs: DesktopRun[];
  runProgress: RunProgress[];
  recoveryNotices?: ProjectView["recoveryNotices"];
}

export type SettingCategory =
  | "models"
  | "agents"
  | "runtime"
  | "permissions"
  | "projects"
  | "network"
  | "logging"
  | "interface";

export type SettingKind =
  | { type: "text" }
  | { type: "number"; min: number; max: number }
  | { type: "boolean" }
  | { type: "select"; options: string[] }
  | { type: "path" }
  | { type: "credential_reference" }
  | { type: "agent_reference" };

export interface SettingDefinition {
  id: string;
  category: SettingCategory;
  label: string;
  description: string;
  kind: SettingKind;
  defaultValue: unknown;
  restartRequired: boolean;
}

export interface SettingsResponse {
  schema: { revision: number; definitions: SettingDefinition[] };
  values: Record<string, unknown>;
  revision: number;
}

export interface BridgeErrorShape {
  code: string;
  message: string;
  field: string | null;
}

export interface AitDesktopApi {
  projects(): Promise<ProjectCatalog>;
  activeRuns(): Promise<ActiveRunsCatalog>;
  agents(): Promise<AgentCatalog>;
  project(projectId: string): Promise<ProjectView>;
  projectSessions(projectId: string): Promise<DesktopSession[]>;
  crons(): Promise<DesktopCron[]>;
  createCron(input: {
    name: string;
    projectId: string;
    baseMessageId: string;
    agentId: string;
    schedule: string;
    timezone: string;
  }): Promise<DesktopCron>;
  setCronEnabled(cronId: string, enabled: boolean): Promise<DesktopCron>;
  triggerCron(cronId: string, scheduledAt: number): Promise<CronRunSubmission>;
  updateProject(input: { projectId: string; name: string; agentId?: string }): Promise<ProjectCatalog>;
  saveProvider(input: ProviderInput): Promise<AgentCatalog>;
  discoverProviderModels(input: ProviderInput): Promise<ProviderModel[]>;
  refreshProviderModels(providerId: string): Promise<AgentCatalog>;
  saveAgent(input: { id?: string; name: string; config: AgentConfiguration }): Promise<AgentCatalog>;
  setSessionConfig(input: {
    projectId: string;
    sessionId: string;
    config: AgentConfiguration;
  }): Promise<{ project: ProjectView; agents: AgentCatalog }>;
  settings(): Promise<SettingsResponse>;
  saveSettings(expectedRevision: number, values: Record<string, unknown>): Promise<SettingsResponse>;
  resetSettings(): Promise<SettingsResponse>;
  chooseProjectDirectory(defaultPath?: string): Promise<string | null>;
  openProjectFile(input: {
    projectId: string;
    sessionId?: string;
    path: string;
    line?: number;
    column?: number;
  }): Promise<{ positioned: boolean }>;
  createProject(input: {
    name: string;
    workdir?: string;
    agentId?: string;
    repoUrl?: string;
  }): Promise<{ catalog: ProjectCatalog; project: ProjectView; selectedProjectId: string }>;
  setProjectDefaultAgent(input: {
    projectId: string;
    agentId: string;
  }): Promise<ProjectCatalog>;
  setSessionAgent(input: {
    projectId: string;
    sessionId: string;
    agentId: string;
  }): Promise<ProjectView>;
  renameSession(input: { projectId: string; sessionId: string; name: string }): Promise<ProjectView>;
  setSessionTitle(input: { projectId: string; sessionId: string; title: string }): Promise<ProjectView>;
  generateSessionTitle(input: { projectId: string; sessionId: string; prompt: string }): Promise<ProjectView>;
  sendMessage(input: {
    projectId: string;
    sessionId: string;
    content: string;
  }): Promise<RunSubmission>;
  resolveApproval(input: {
    runId: string;
    projectId: string;
    approvalId: string;
    action: "approve" | "deny" | "cancel";
    scope?: "one_shot" | "turn" | "session";
  }): Promise<ProjectView>;
  resolveToolApproval(input: {
    runId: string; projectId: string; approvalId: string; action: "approve" | "deny" | "cancel";
  }): Promise<ProjectView>;
  resolveToolInteraction(input: {
    runId: string;
    projectId: string;
    interactionId: string;
    action: "submit" | "approve" | "deny" | "cancel";
    response?: Record<string, string | string[]>;
  }): Promise<ProjectView>;
  subscribeRunEvents(listener: (updates: RunStreamUpdate[]) => void): () => void;
  fork(input: {
    projectId: string;
    currentSessionId?: string;
    sourceMessageId: string;
    agentId: string;
    content: string;
    submissionId?: string;
    recover?: boolean;
  }): Promise<ForkSubmission>;
}

declare global {
  interface Window {
    ait: AitDesktopApi;
  }
}
