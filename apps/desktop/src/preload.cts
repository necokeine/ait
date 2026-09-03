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
  fork: (input) => invoke("session.fork", input),
};

contextBridge.exposeInMainWorld("ait", Object.freeze(api));
