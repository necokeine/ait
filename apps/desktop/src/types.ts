export type MessageRole = "user" | "system" | "assistant";
export type MessageKind = "standard" | "tool_result";

export type MessagePart =
  | { type: "text"; text: string }
  | { type: "file"; name: string; media_type: string }
  | { type: "tool_use"; call_id: string; tool_name: string; arguments: string }
  | { type: "structured"; media_type: string; value: string }
  | { type: "redacted" };

export interface DesktopMessage {
  id: string;
  projectId: string;
  parentMessageId: string | null;
  role: MessageRole;
  kind: MessageKind;
  parts: MessagePart[];
  createdAt: number;
}

export interface DesktopProject {
  id: string;
  name: string;
  workdir: string;
  description: string;
  defaultAgentId: string | null;
}

export interface AgentSummary {
  id: string;
  name: string;
  model: string;
  enabled: boolean;
}

export interface DesktopSession {
  id: string;
  projectId: string;
  title: string;
  currentMessageId: string;
  agentId: string;
  version: number;
  active: boolean;
  updatedAt: number;
}

export interface DesktopSnapshot {
  protocolVersion: number;
  revision: number;
  projects: DesktopProject[];
  agents: AgentSummary[];
  sessions: DesktopSession[];
  messages: DesktopMessage[];
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
  snapshot(): Promise<DesktopSnapshot>;
  settings(): Promise<SettingsResponse>;
  saveSettings(expectedRevision: number, values: Record<string, unknown>): Promise<SettingsResponse>;
  resetSettings(): Promise<SettingsResponse>;
  chooseProjectDirectory(): Promise<string | null>;
  createProject(input: {
    name: string;
    workdir: string;
    agentId: string;
  }): Promise<{ snapshot: DesktopSnapshot; selectedProjectId: string }>;
  setProjectDefaultAgent(input: {
    projectId: string;
    agentId: string;
  }): Promise<DesktopSnapshot>;
  createSession(input: {
    projectId: string;
    agentId: string;
  }): Promise<{ snapshot: DesktopSnapshot; selectedSessionId: string }>;
  sendMessage(input: {
    sessionId: string;
    expectedVersion: number;
    content: string;
  }): Promise<DesktopSnapshot>;
  fork(input: {
    projectId: string;
    sourceMessageId: string;
    agentId: string;
    content: string;
  }): Promise<{ snapshot: DesktopSnapshot; selectedSessionId: string }>;
}

declare global {
  interface Window {
    ait: AitDesktopApi;
  }
}
