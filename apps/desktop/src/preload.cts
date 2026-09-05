import { contextBridge, ipcRenderer } from "electron";
import type { AitDesktopApi } from "./types.js";

const invoke = <T,>(method: string, params: unknown = {}): Promise<T> =>
  ipcRenderer.invoke("ait:request", method, params) as Promise<T>;

const api: AitDesktopApi = {
  snapshot: () => invoke("workspace.snapshot"),
  settings: () => invoke("settings.get"),
  saveSettings: (expectedRevision, values) =>
    invoke("settings.save", { expectedRevision, values }),
  resetSettings: () => invoke("settings.reset"),
  chooseProjectDirectory: () => invoke("project.choose-directory"),
  createProject: (input) => invoke("project.create", input),
  setProjectDefaultAgent: (input) => invoke("project.set-default-agent", input),
  createSession: (input) => invoke("session.create", input),
  setSessionAgent: (input) => invoke("session.set-agent", input),
  sendMessage: (input) => invoke("session.send-message", input),
  fork: (input) => invoke("session.fork", input),
};

contextBridge.exposeInMainWorld("ait", Object.freeze(api));
