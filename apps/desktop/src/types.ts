export type MessageRole = "user" | "system" | "assistant";
export type MessageKind = "standard" | "tool_result";
export type ReasoningEffort = string;

export interface AgentConfiguration { provider_id: string; model: string; reasoning_effort: string | null }
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
  error?: { code?: string; message: string };
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
  view: DesktopView;
  runId: string;
}

export interface DesktopView {
  protocolVersion: number;
  revision: number;
  projects: DesktopProject[];
  agents: AgentSummary[];
  providers: AgentProvider[];
  sessions: DesktopSession[];
  messages: DesktopMessage[];
  runs: DesktopRun[];
  runProgress: RunProgress[];
  recoveryNotices?: Array<{
    projectId: string;
    projectName: string;
    sessionId?: string;
    sessionTitle?: string;
    runId: string;
    code?: string;
    message: string;
  }>;
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
  | { type: "credential_reference" };

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
  view(projectId?: string): Promise<DesktopView>;
  saveProvider(input: ProviderInput): Promise<DesktopView>;
  discoverProviderModels(input: ProviderInput): Promise<ProviderModel[]>;
  refreshProviderModels(providerId: string): Promise<DesktopView>;
  saveAgent(input: { id?: string; name: string; config: AgentConfiguration }): Promise<DesktopView>;
  setSessionConfig(input: { sessionId: string; config: AgentConfiguration }): Promise<DesktopView>;
  settings(): Promise<SettingsResponse>;
  saveSettings(expectedRevision: number, values: Record<string, unknown>): Promise<SettingsResponse>;
  resetSettings(): Promise<SettingsResponse>;
  chooseProjectDirectory(): Promise<string | null>;
  openProjectFile(input: {
    projectId: string;
    path: string;
    line?: number;
    column?: number;
  }): Promise<{ positioned: boolean }>;
  createProject(input: {
    name: string;
    workdir: string;
    agentId: string;
    repoUrl?: string;
  }): Promise<{ view: DesktopView; selectedProjectId: string }>;
  setProjectDefaultAgent(input: {
    projectId: string;
    agentId: string;
  }): Promise<DesktopView>;
  createSession(input: {
    projectId: string;
    agentId: string;
  }): Promise<{ view: DesktopView; selectedSessionId: string }>;
  setSessionAgent(input: {
    sessionId: string;
    agentId: string;
  }): Promise<DesktopView>;
  renameSession(input: { sessionId: string; name: string }): Promise<DesktopView>;
  setSessionTitle(input: { sessionId: string; title: string }): Promise<DesktopView>;
  generateSessionTitle(input: { sessionId: string; prompt: string }): Promise<DesktopView>;
  sendMessage(input: {
    sessionId: string;
    content: string;
  }): Promise<RunSubmission>;
  resolveApproval(input: {
    runId: string;
    approvalId: string;
    action: "approve" | "deny" | "cancel";
    scope?: "one_shot" | "turn" | "session";
  }): Promise<DesktopView>;
  subscribeRunEvents(listener: (updates: RunStreamUpdate[]) => void): () => void;
  fork(input: {
    projectId: string;
    currentSessionId: string;
    sourceMessageId: string;
    agentId: string;
    content: string;
  }): Promise<RunSubmission & {
    selectedSessionId: string;
    reusedCurrentSession: boolean;
  }>;
}

declare global {
  interface Window {
    ait: AitDesktopApi;
  }
}
