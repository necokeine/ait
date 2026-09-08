import { contextBridge, ipcRenderer } from "electron";
import type { AitDesktopApi, RunStreamFrame, RunStreamUpdate } from "./types.js";

const invoke = <T,>(method: string, params: unknown = {}): Promise<T> =>
  ipcRenderer.invoke("ait:request", method, params) as Promise<T>;

const runEventListeners = new Set<(updates: RunStreamUpdate[]) => void>();
const runEventGeneration = globalThis.crypto.randomUUID();
let runEventsReady = false;
ipcRenderer.on("ait:run-event-frame", (_event, value: unknown) => {
  const frame = value as Partial<RunStreamFrame>;
  if (frame.generation !== runEventGeneration
    || !Number.isSafeInteger(frame.id)
    || !Array.isArray(frame.updates)) return;
  try {
    for (const listener of runEventListeners) listener(frame.updates);
  } finally {
    ipcRenderer.send("ait:run-event-ack", runEventGeneration, frame.id);
  }
});

const api: AitDesktopApi = {
  view: (projectId) => invoke("workspace.view", { projectId }),
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
  subscribeRunEvents: (listener) => {
    runEventListeners.add(listener);
    if (!runEventsReady) {
      runEventsReady = true;
      ipcRenderer.send("ait:run-event-ready", runEventGeneration);
    }
    return () => runEventListeners.delete(listener);
  },
  fork: (input) => invoke("session.fork", input),
};

contextBridge.exposeInMainWorld("ait", Object.freeze(api));
