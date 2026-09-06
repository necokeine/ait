import { contextBridge, ipcRenderer } from "electron";
import type { AitDesktopApi } from "./types.js";

const invoke = <T,>(method: string, params: unknown = {}): Promise<T> =>
  ipcRenderer.invoke("ait:request", method, params) as Promise<T>;

const api: AitDesktopApi = {
  snapshot: () => invoke("workspace.snapshot"),
  saveProvider: (input) => invoke("provider.save", input),
  discoverProviderModels: (input) => invoke("provider.discover-models", input),
  refreshProviderModels: (providerId) => invoke("provider.refresh-models", { providerId }),
  saveAgent: (input) => invoke("agent.save", input),
  setSessionConfig: (input) => invoke("session.set-config", input),
  settings: () => invoke("settings.get"),
  saveSettings: (expectedRevision, values) =>
    invoke("settings.save", { expectedRevision, values }),
  resetSettings: () => invoke("settings.reset"),
  chooseProjectDirectory: () => invoke("project.choose-directory"),
  openProjectFile: (input) => invoke("project.open-file", input),
  createProject: (input) => invoke("project.create", input),
  setProjectDefaultAgent: (input) => invoke("project.set-default-agent", input),
  createSession: (input) => invoke("session.create", input),
  setSessionAgent: (input) => invoke("session.set-agent", input),
  renameSession: (input) => invoke("session.rename", input),
  setSessionTitle: (input) => invoke("session.set-title", input),
  generateSessionTitle: (input) => invoke("session.generate-title", input),
  sendMessage: (input) => invoke("session.send-message", input),
  fork: (input) => invoke("session.fork", input),
};

contextBridge.exposeInMainWorld("ait", Object.freeze(api));
