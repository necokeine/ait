import { flattenMessageTree, messageText, pathToMessage, type FlatTreeNode } from "./tree.js";
import type {
  DesktopMessage,
  DesktopSession,
  DesktopSnapshot,
  SettingCategory,
  SettingDefinition,
  SettingsResponse,
} from "./types.js";

const $ = <T extends Element>(selector: string): T => {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`Missing UI element: ${selector}`);
  return element;
};

const appShell = $("#app");
const sessionList = $("#session-list");
const conversation = $("#conversation");
const conversationScroll = $("#conversation-scroll");
const treeList = $("#tree-list");
const treeScroll = $<HTMLElement>("#tree-scroll");
const nodeDetails = $("#node-details");
const messageInput = $<HTMLTextAreaElement>("#message-input");
const composerAgent = $<HTMLSelectElement>("#composer-agent");
const sendButton = $<HTMLButtonElement>("#send-button");
const settingsDialog = $("#settings-dialog");
const commandDialog = $("#command-dialog");
const projectDialog = $("#project-dialog");
const sessionDialog = $("#session-dialog");

let snapshot: DesktopSnapshot | undefined;
let selectedProjectId: string | undefined;
let selectedSessionId: string | undefined;
let selectedNodeId: string | undefined;
let collapsedNodes = new Set<string>();
let flatTree: FlatTreeNode[] = [];
let settings: SettingsResponse | undefined;
let settingsDraft: Record<string, unknown> = {};
let settingsCategory: SettingCategory = "models";
let toastTimer: number | undefined;

void initialize();

async function initialize(): Promise<void> {
  bindInteractions();
  try {
    const [loadedSnapshot, loadedSettings] = await Promise.all([
      window.ait.snapshot(),
      window.ait.settings(),
    ]);
    snapshot = loadedSnapshot;
    settings = loadedSettings;
    settingsDraft = structuredClone(loadedSettings.values);
    const newestSession = loadedSnapshot.sessions
      .toSorted((left, right) => right.updatedAt - left.updatedAt)[0]?.id;
    selectedSessionId = newestSession;
    selectedProjectId = loadedSnapshot.sessions.find((session) => session.id === newestSession)?.projectId
      ?? loadedSnapshot.projects[0]?.id;
    applyPreferences();
    renderAll();
    appShell.classList.remove("is-loading");
    const coreStatus = $("#core-status");
    coreStatus.classList.add("is-ready");
    coreStatus.lastChild!.textContent = " Core ready";
    if (loadedSnapshot.projects.length === 0) openProjectDialog();
  } catch (error) {
    renderFatal(error);
  }
}

function bindInteractions(): void {
  $("#sidebar-toggle").addEventListener("click", () => appShell.classList.toggle("sidebar-collapsed"));
  $("#tree-toggle").addEventListener("click", toggleTree);
  $("#tree-close").addEventListener("click", () => appShell.classList.add("tree-collapsed"));
  $("#settings-trigger").addEventListener("click", openSettings);
  $("#project-button").addEventListener("click", openProjectDialog);
  $("#project-close").addEventListener("click", closeProjectDialog);
  $("#new-session").addEventListener("click", openSessionDialog);
  $("#session-close").addEventListener("click", closeSessionDialog);
  $("#session-cancel").addEventListener("click", closeSessionDialog);
  $("#project-choose-path").addEventListener("click", () => void chooseProjectPath());
  $("#project-create").addEventListener("submit", (event) => {
    event.preventDefault();
    void createProject();
  });
  $("#project-backend-save").addEventListener("click", () => void saveProjectBackend());
  $("#session-create").addEventListener("submit", (event) => {
    event.preventDefault();
    void createSession();
  });
  $("#settings-close").addEventListener("click", closeSettings);
  $("#settings-cancel").addEventListener("click", closeSettings);
  $("#settings-save").addEventListener("click", () => void saveSettings());
  $("#settings-reset").addEventListener("click", () => void resetSettings());
  $("#clear-selection").addEventListener("click", clearNodeSelection);
  $("#command-trigger").addEventListener("click", openCommandPalette);
  commandDialog.addEventListener("click", (event) => {
    if (event.target === commandDialog) closeCommandPalette();
  });
  projectDialog.addEventListener("click", (event) => {
    if (event.target === projectDialog) closeProjectDialog();
  });
  sessionDialog.addEventListener("click", (event) => {
    if (event.target === sessionDialog) closeSessionDialog();
  });
  settingsDialog.addEventListener("click", (event) => {
    if (event.target === settingsDialog) closeSettings();
  });
  $<HTMLInputElement>("#command-input").addEventListener("input", renderCommandResults);
  messageInput.addEventListener("input", updateComposerState);
  messageInput.addEventListener("keydown", (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      void submitMessage();
    }
  });
  $("#composer").addEventListener("submit", (event) => {
    event.preventDefault();
    void submitMessage();
  });
  treeScroll.addEventListener("keydown", handleTreeKeyboard);
  document.addEventListener("keydown", handleGlobalKeyboard);
}

function renderAll(): void {
  if (!snapshot) return;
  const project = currentProject();
  $("#project-name").textContent = project?.name ?? "No Project";
  $("#project-path").textContent = project?.workdir ?? "Open a local Project";
  $(".workspace-avatar").textContent = project?.name.trim().slice(0, 1).toUpperCase() || "＋";
  renderSessions();
  renderAgents();
  renderConversation();
  renderTree();
  updateComposerState();
}

function currentSession(): DesktopSession | undefined {
  return snapshot?.sessions.find((session) =>
    session.id === selectedSessionId && session.projectId === selectedProjectId);
}

function currentProject() {
  return snapshot?.projects.find((project) => project.id === selectedProjectId);
}

function renderSessions(): void {
  if (!snapshot) return;
  const sessions = snapshot.sessions
    .filter((session) => session.projectId === selectedProjectId)
    .toSorted((left, right) => right.updatedAt - left.updatedAt);
  if (sessions.length === 0) {
    sessionList.innerHTML = '<div class="empty-state">No sessions yet</div>';
    return;
  }
  sessionList.innerHTML = sessions.map((session) => {
    const agent = snapshot?.agents.find((candidate) => candidate.id === session.agentId);
    const selected = session.id === selectedSessionId;
    return `<button class="session-item${selected ? " is-selected" : ""}" type="button" role="option" aria-selected="${selected}" data-session-id="${escapeAttribute(session.id)}">
      <span class="session-symbol">${session.active ? "◉" : "⑂"}</span>
      <span class="session-copy"><strong>${escapeHtml(session.title)}</strong><small>${escapeHtml(agent?.name ?? "Agent")} · ${relativeTime(session.updatedAt)}</small></span>
      ${session.active ? '<i class="running-dot" title="Run active"></i>' : ""}
    </button>`;
  }).join("");
  sessionList.querySelectorAll<HTMLElement>("[data-session-id]").forEach((button) => {
    button.addEventListener("click", () => {
      selectedSessionId = button.dataset.sessionId;
      selectedNodeId = undefined;
      renderAll();
    });
  });
}

function renderAgents(): void {
  if (!snapshot) return;
  const current = currentSession();
  composerAgent.innerHTML = snapshot.agents
    .filter((agent) => agent.enabled)
    .map((agent) => `<option value="${escapeAttribute(agent.id)}"${agent.id === current?.agentId ? " selected" : ""}>${escapeHtml(agent.name)} · ${escapeHtml(agent.model)}</option>`)
    .join("");
  composerAgent.disabled = selectedNodeId === undefined;
  const agent = snapshot.agents.find((candidate) => candidate.id === current?.agentId);
  $("#agent-chip").textContent = agent ? `${agent.name} · ${agent.model}` : "No Agent";
}

function renderConversation(): void {
  if (!snapshot) return;
  const session = currentSession();
  if (!session) {
    $("#session-title").textContent = "No session selected";
    $("#session-breadcrumb").textContent = currentProject()?.name ?? "No Project";
    conversation.innerHTML = `<div class="empty-state"><p>${currentProject() ? "Create or choose a Session to begin." : "Create a Project to begin."}</p></div>`;
    return;
  }
  const project = snapshot.projects.find((candidate) => candidate.id === session.projectId);
  $("#session-title").textContent = session.title;
  $("#session-breadcrumb").textContent = `${project?.name ?? "Project"} / Session v${session.version}`;
  const messages = pathToMessage(
    snapshot.messages.filter((message) => message.projectId === session.projectId),
    session.currentMessageId,
  );
  conversation.innerHTML = messages.map(renderMessage).join("");
  requestAnimationFrame(() => {
    conversationScroll.scrollTop = conversationScroll.scrollHeight;
  });
}

function renderMessage(message: DesktopMessage): string {
  const icon = message.role === "assistant" ? "AI" : message.role === "user" ? "U" : "S";
  const content = message.parts.map((part) => {
    if (part.type === "text") return `<div class="message-content">${escapeHtml(part.text)}</div>`;
    if (part.type === "tool_use") return `<div class="tool-card"><header><span>◇</span><strong>${escapeHtml(part.tool_name)}</strong><small>tool call</small></header><pre>${escapeHtml(prettyJson(part.arguments))}</pre></div>`;
    if (part.type === "file") return `<div class="tool-card"><header><span>＋</span><strong>${escapeHtml(part.name)}</strong><small>${escapeHtml(part.media_type)}</small></header></div>`;
    if (part.type === "structured") return `<div class="tool-card"><header><span>{ }</span><strong>${escapeHtml(part.media_type)}</strong></header><pre>${escapeHtml(part.value)}</pre></div>`;
    return '<div class="message-content">Content redacted</div>';
  }).join("");
  return `<article class="message ${message.role}" data-message-id="${escapeAttribute(message.id)}">
    <div class="message-avatar">${icon}</div>
    <div><div class="message-heading"><strong>${message.role}</strong><time>${formatTime(message.createdAt)}</time></div>${content}</div>
  </article>`;
}

function renderTree(): void {
  if (!snapshot) return;
  const session = currentSession();
  const messages = snapshot.messages.filter((message) => message.projectId === selectedProjectId);
  flatTree = flattenMessageTree(messages, session, selectedNodeId, collapsedNodes);
  const visible = flatTree.slice(0, 2_000);
  treeList.innerHTML = visible.map((node) => {
    const preview = messageText(node.message).replace(/\s+/g, " ").trim() || "Empty message";
    const expander = node.childCount > 0 ? (node.expanded ? "⌄" : "›") : "";
    return `<div class="tree-node tree-depth-${Math.min(node.depth, 12)}${node.selected ? " is-selected" : ""}${node.onCurrentBranch ? " on-current" : ""}" role="treeitem" aria-selected="${node.selected}" aria-expanded="${node.childCount > 0 ? node.expanded : "false"}" tabindex="${node.selected ? "0" : "-1"}" data-message-id="${escapeAttribute(node.message.id)}" data-depth="${node.depth}">
      <button class="tree-expander" type="button" aria-label="${node.expanded ? "Collapse" : "Expand"}">${expander}</button>
      <span class="tree-role">${roleLetter(node.message.role)}</span>
      <span class="tree-copy"><strong>${escapeHtml(preview)}</strong><small>${node.message.role} · ${formatTime(node.message.createdAt)}</small></span>
      ${node.childCount > 1 ? `<span class="tree-count">${node.childCount}</span>` : ""}
    </div>`;
  }).join("");
  if (flatTree.length > visible.length) {
    treeList.insertAdjacentHTML("beforeend", `<div class="tree-limit">Showing first ${visible.length.toLocaleString()} of ${flatTree.length.toLocaleString()} nodes</div>`);
  }
  treeList.querySelectorAll<HTMLElement>(".tree-node").forEach((row) => {
    row.addEventListener("click", () => selectTreeNode(row.dataset.messageId));
    row.querySelector(".tree-expander")?.addEventListener("click", (event) => {
      event.stopPropagation();
      toggleNode(row.dataset.messageId);
    });
  });
  renderNodeDetails();
}

function selectTreeNode(id: string | undefined): void {
  if (!id) return;
  selectedNodeId = id;
  renderTree();
  updateComposerState();
  treeList.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(id)}"]`)?.focus();
}

function toggleNode(id: string | undefined): void {
  if (!id) return;
  if (collapsedNodes.has(id)) collapsedNodes.delete(id);
  else collapsedNodes.add(id);
  renderTree();
}

function renderNodeDetails(): void {
  const selected = snapshot?.messages.find((message) => message.id === selectedNodeId);
  if (!selected) {
    nodeDetails.innerHTML = '<div class="empty-details"><span>⑂</span><p>Select a node to inspect it and start a branch.</p></div>';
    $("#branch-context").classList.add("is-hidden");
    return;
  }
  const preview = messageText(selected);
  nodeDetails.innerHTML = `<div class="node-details-header"><span class="node-role-pill">${selected.role}</span><code class="node-id">${escapeHtml(shortId(selected.id))}</code></div>
    <p class="node-preview">${escapeHtml(preview)}</p>
    <div class="node-actions"><button id="branch-focus" class="primary-button" type="button">Branch from here</button></div>`;
  $("#branch-focus").addEventListener("click", () => messageInput.focus());
  $("#branch-node-label").textContent = `${selected.role} · ${shortId(selected.id)}`;
  $("#branch-context").classList.remove("is-hidden");
}

function clearNodeSelection(): void {
  selectedNodeId = undefined;
  renderTree();
  updateComposerState();
}

async function submitMessage(): Promise<void> {
  const session = currentSession();
  const content = messageInput.value.trim();
  if (!snapshot || !session || !content || sendButton.disabled) return;
  sendButton.disabled = true;
  sendButton.textContent = "…";
  try {
    if (selectedNodeId) {
      const result = await window.ait.fork({
        projectId: session.projectId,
        sourceMessageId: selectedNodeId,
        agentId: composerAgent.value,
        content,
      });
      snapshot = result.snapshot;
      selectedSessionId = result.selectedSessionId;
      showToast("New branch created. The original session was left unchanged.");
    } else {
      snapshot = await window.ait.sendMessage({
        sessionId: session.id,
        expectedVersion: session.version,
        content,
      });
      showToast("Message sent.");
    }
    selectedNodeId = undefined;
    messageInput.value = "";
    renderAll();
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    sendButton.textContent = "↑";
    updateComposerState();
  }
}

function updateComposerState(): void {
  const session = currentSession();
  sendButton.disabled = !session || messageInput.value.trim().length === 0 || session.active;
  messageInput.disabled = !session;
  composerAgent.disabled = selectedNodeId === undefined;
  messageInput.placeholder = !session
    ? "Create a Session to start…"
    : selectedNodeId
      ? "Write the first user message on this branch…"
      : session.active
        ? "This Session is running…"
        : "Send a message to this Session…";
  $("#composer-hint").textContent = selectedNodeId
    ? "A new immutable branch and Session will be created · ⌘ Enter to send"
    : "Send to the current Session, or select a tree node to branch · ⌘ Enter to send";
}

function toggleTree(): void {
  appShell.classList.toggle("tree-collapsed");
}

function handleTreeKeyboard(event: KeyboardEvent): void {
  if (!["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", "Enter"].includes(event.key)) return;
  event.preventDefault();
  const currentIndex = Math.max(0, flatTree.findIndex((node) => node.message.id === selectedNodeId));
  if (event.key === "ArrowDown") selectTreeNode(flatTree[Math.min(flatTree.length - 1, currentIndex + 1)]?.message.id);
  if (event.key === "ArrowUp") selectTreeNode(flatTree[Math.max(0, currentIndex - 1)]?.message.id);
  if (event.key === "Enter") messageInput.focus();
  const current = flatTree[currentIndex];
  if (event.key === "ArrowLeft" && current?.expanded && current.childCount > 0) toggleNode(current.message.id);
  if (event.key === "ArrowRight" && !current?.expanded && current?.childCount) toggleNode(current.message.id);
}

function handleGlobalKeyboard(event: KeyboardEvent): void {
  if ((event.metaKey || event.ctrlKey) && event.key === ",") {
    event.preventDefault();
    openSettings();
  }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
    event.preventDefault();
    openCommandPalette();
  }
  if (event.key === "Escape") {
    closeSettings();
    closeCommandPalette();
    closeProjectDialog();
    closeSessionDialog();
  }
}

function openProjectDialog(): void {
  if (!snapshot) return;
  projectDialog.classList.remove("is-hidden");
  renderProjectDialog();
}

function closeProjectDialog(): void {
  projectDialog.classList.add("is-hidden");
}

function renderProjectDialog(): void {
  if (!snapshot) return;
  const projects = snapshot.projects;
  $("#project-list").innerHTML = projects.map((project) =>
    `<button class="project-list-item${project.id === selectedProjectId ? " is-selected" : ""}" type="button" data-project-id="${escapeAttribute(project.id)}"><strong>${escapeHtml(project.name)}</strong><small>${escapeHtml(project.workdir)}</small></button>`,
  ).join("") || '<div class="empty-details"><p>No Projects yet</p></div>';
  $("#project-list").querySelectorAll<HTMLElement>("[data-project-id]").forEach((button) => {
    button.addEventListener("click", () => {
      selectProject(button.dataset.projectId);
      closeProjectDialog();
      void refreshSnapshot();
    });
  });
  const options = agentOptions();
  const project = currentProject();
  const backend = $<HTMLSelectElement>("#project-backend");
  const createAgent = $<HTMLSelectElement>("#project-create-agent");
  backend.innerHTML = options;
  createAgent.innerHTML = options;
  backend.disabled = !project || options.length === 0;
  $("#project-backend-save").toggleAttribute("disabled", !project || options.length === 0);
  if (project?.defaultAgentId) backend.value = project.defaultAgentId;
  $("#project-backend-copy").textContent = project
    ? `New Sessions in ${project.name} use this Agent by default.`
    : "Create a Project, then choose its default Agent.";
}

function selectProject(projectId: string | undefined): void {
  if (!snapshot || !projectId || !snapshot.projects.some((project) => project.id === projectId)) return;
  selectedProjectId = projectId;
  selectedSessionId = snapshot.sessions
    .filter((session) => session.projectId === projectId)
    .toSorted((left, right) => right.updatedAt - left.updatedAt)[0]?.id;
  selectedNodeId = undefined;
  renderAll();
}

async function refreshSnapshot(): Promise<void> {
  try {
    snapshot = await window.ait.snapshot();
    renderAll();
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

async function chooseProjectPath(): Promise<void> {
  const path = await window.ait.chooseProjectDirectory();
  if (path) $<HTMLInputElement>("#project-create-path").value = path;
}

async function createProject(): Promise<void> {
  const name = $<HTMLInputElement>("#project-create-name").value.trim();
  const workdir = $<HTMLInputElement>("#project-create-path").value;
  const agentId = $<HTMLSelectElement>("#project-create-agent").value;
  if (!name || !workdir || !agentId) {
    showToast("Choose a name, directory, and backend.", true);
    return;
  }
  const button = $<HTMLButtonElement>("#project-create-submit");
  button.disabled = true;
  button.textContent = "Creating…";
  try {
    const result = await window.ait.createProject({ name, workdir, agentId });
    snapshot = result.snapshot;
    selectedProjectId = result.selectedProjectId;
    selectedSessionId = undefined;
    selectedNodeId = undefined;
    $<HTMLInputElement>("#project-create-name").value = "";
    $<HTMLInputElement>("#project-create-path").value = "";
    closeProjectDialog();
    renderAll();
    showToast(`${name} created with the selected backend.`);
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    button.disabled = false;
    button.textContent = "Create Project";
  }
}

async function saveProjectBackend(): Promise<void> {
  const project = currentProject();
  const agentId = $<HTMLSelectElement>("#project-backend").value;
  if (!project || !agentId) return;
  try {
    snapshot = await window.ait.setProjectDefaultAgent({ projectId: project.id, agentId });
    renderAll();
    renderProjectDialog();
    showToast(`${project.name} backend updated.`);
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

function openSessionDialog(): void {
  const project = currentProject();
  if (!project) {
    openProjectDialog();
    return;
  }
  const select = $<HTMLSelectElement>("#session-agent");
  select.innerHTML = agentOptions();
  if (project.defaultAgentId) select.value = project.defaultAgentId;
  $("#session-project-copy").textContent = `The Session belongs to ${project.name} and stays bound to this Agent.`;
  sessionDialog.classList.remove("is-hidden");
}

function closeSessionDialog(): void {
  sessionDialog.classList.add("is-hidden");
}

async function createSession(): Promise<void> {
  const project = currentProject();
  const agentId = $<HTMLSelectElement>("#session-agent").value;
  if (!project || !agentId) return;
  const button = $<HTMLButtonElement>("#session-create-submit");
  button.disabled = true;
  button.textContent = "Creating…";
  try {
    const result = await window.ait.createSession({ projectId: project.id, agentId });
    snapshot = result.snapshot;
    selectedSessionId = result.selectedSessionId;
    selectedNodeId = undefined;
    closeSessionDialog();
    renderAll();
    messageInput.focus();
    showToast("Session created.");
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    button.disabled = false;
    button.textContent = "Create Session";
  }
}

function agentOptions(): string {
  return snapshot?.agents
    .filter((agent) => agent.enabled)
    .map((agent) => `<option value="${escapeAttribute(agent.id)}">${escapeHtml(agent.name)} · ${escapeHtml(agent.model)}</option>`)
    .join("") ?? "";
}

function openSettings(): void {
  if (!settings) return;
  settingsDraft = structuredClone(settings.values);
  settingsDialog.classList.remove("is-hidden");
  renderSettings();
}

function closeSettings(): void {
  settingsDialog.classList.add("is-hidden");
}

function renderSettings(): void {
  if (!settings) return;
  const categories = [...new Set(settings.schema.definitions.map((definition) => definition.category))];
  $("#settings-nav").innerHTML = categories.map((category) =>
    `<button type="button" data-category="${category}" class="${category === settingsCategory ? "is-active" : ""}">${category}</button>`,
  ).join("");
  $("#settings-nav").querySelectorAll<HTMLElement>("[data-category]").forEach((button) => {
    button.addEventListener("click", () => {
      settingsCategory = button.dataset.category as SettingCategory;
      renderSettings();
    });
  });
  const definitions = settings.schema.definitions.filter((definition) => definition.category === settingsCategory);
  $("#settings-fields").innerHTML = `<header class="settings-section-header"><span class="eyebrow">Core configuration</span><h3>${settingsCategory}</h3><p>These fields and defaults come from the Rust core schema.</p></header>${definitions.map(renderSetting).join("")}`;
  $("#settings-fields").querySelectorAll<HTMLInputElement | HTMLSelectElement>("[data-setting-id]").forEach((control) => {
    control.addEventListener("change", () => readSettingControl(control));
    control.addEventListener("input", () => readSettingControl(control));
  });
  $("#settings-state").textContent = `Schema ${settings.schema.revision} · state ${settings.revision}`;
}

function renderSetting(definition: SettingDefinition): string {
  const value = settingsDraft[definition.id];
  const restart = definition.restartRequired ? '<span class="restart-badge">Restart</span>' : "";
  return `<div class="setting-row"><div class="setting-copy"><label for="setting-${escapeAttribute(definition.id)}">${escapeHtml(definition.label)}${restart}</label><p>${escapeHtml(definition.description)}</p></div><div class="setting-control">${renderSettingControl(definition, value)}</div></div>`;
}

function renderSettingControl(definition: SettingDefinition, value: unknown): string {
  const common = `id="setting-${escapeAttribute(definition.id)}" data-setting-id="${escapeAttribute(definition.id)}"`;
  if (definition.kind.type === "boolean") {
    return `<label class="switch"><input ${common} type="checkbox"${value === true ? " checked" : ""}/><span class="switch-track"></span></label>`;
  }
  if (definition.kind.type === "select") {
    return `<select ${common}>${definition.kind.options.map((option) => `<option value="${escapeAttribute(option)}"${value === option ? " selected" : ""}>${escapeHtml(humanize(option))}</option>`).join("")}</select>`;
  }
  if (definition.kind.type === "number") {
    return `<input ${common} type="number" min="${definition.kind.min}" max="${definition.kind.max}" value="${escapeAttribute(String(value ?? ""))}"/>`;
  }
  const type = definition.kind.type === "credential_reference" ? "password" : "text";
  return `<input ${common} type="${type}" value="${escapeAttribute(String(value ?? ""))}" autocomplete="off"/>`;
}

function readSettingControl(control: HTMLInputElement | HTMLSelectElement): void {
  const id = control.dataset.settingId;
  const definition = settings?.schema.definitions.find((item) => item.id === id);
  if (!id || !definition) return;
  if (definition.kind.type === "boolean" && control instanceof HTMLInputElement) settingsDraft[id] = control.checked;
  else if (definition.kind.type === "number") settingsDraft[id] = Number(control.value);
  else settingsDraft[id] = control.value;
  control.classList.remove("invalid");
}

async function saveSettings(): Promise<void> {
  if (!settings) return;
  const button = $<HTMLButtonElement>("#settings-save");
  button.disabled = true;
  button.textContent = "Saving…";
  try {
    settings = await window.ait.saveSettings(settings.revision, settingsDraft);
    settingsDraft = structuredClone(settings.values);
    applyPreferences();
    renderSettings();
    showToast("Settings saved by the Ait core.");
  } catch (error) {
    const field = (error as { field?: string | null }).field;
    if (field) settingsDialog.querySelector<HTMLElement>(`[data-setting-id="${CSS.escape(field)}"]`)?.classList.add("invalid");
    showToast(errorMessage(error), true);
  } finally {
    button.disabled = false;
    button.textContent = "Save changes";
  }
}

async function resetSettings(): Promise<void> {
  try {
    settings = await window.ait.resetSettings();
    settingsDraft = structuredClone(settings.values);
    applyPreferences();
    renderSettings();
    showToast("Core defaults restored.");
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

function applyPreferences(): void {
  const theme = settings?.values["interface.theme"];
  document.documentElement.dataset.theme = typeof theme === "string" ? theme : "system";
  document.documentElement.dataset.density = settings?.values["interface.density"] === "comfortable" ? "comfortable" : "compact";
  if (settings?.values["interface.session_tree_open"] === false) appShell.classList.add("tree-collapsed");
  else appShell.classList.remove("tree-collapsed");
}

function openCommandPalette(): void {
  commandDialog.classList.remove("is-hidden");
  const input = $<HTMLInputElement>("#command-input");
  input.value = "";
  renderCommandResults();
  requestAnimationFrame(() => input.focus());
}

function closeCommandPalette(): void {
  commandDialog.classList.add("is-hidden");
}

function renderCommandResults(): void {
  const query = $<HTMLInputElement>("#command-input").value.trim().toLowerCase();
  const sessions = snapshot?.sessions.filter((session) =>
    session.projectId === selectedProjectId && session.title.toLowerCase().includes(query)) ?? [];
  const commands = [
    { id: "projects", title: "Switch or Create Project", hint: "" },
    { id: "new-session", title: "Create Session", hint: "" },
    { id: "settings", title: "Open Settings", hint: "⌘," },
    { id: "tree", title: "Toggle Session Tree", hint: "" },
  ].filter((command) => command.title.toLowerCase().includes(query));
  $("#command-results").innerHTML = [
    ...sessions.map((session) => `<button class="command-result" type="button" data-session="${escapeAttribute(session.id)}"><span>⑂</span>${escapeHtml(session.title)}<small>Session</small></button>`),
    ...commands.map((command) => `<button class="command-result" type="button" data-command="${command.id}"><span>›</span>${command.title}<small>${command.hint}</small></button>`),
  ].join("") || '<div class="empty-details"><p>No matching command</p></div>';
  $("#command-results").querySelectorAll<HTMLElement>("[data-session]").forEach((button) => {
    button.addEventListener("click", () => {
      selectedSessionId = button.dataset.session;
      selectedNodeId = undefined;
      closeCommandPalette();
      renderAll();
    });
  });
  $("#command-results").querySelectorAll<HTMLElement>("[data-command]").forEach((button) => {
    button.addEventListener("click", () => {
      closeCommandPalette();
      if (button.dataset.command === "projects") openProjectDialog();
      if (button.dataset.command === "new-session") openSessionDialog();
      if (button.dataset.command === "settings") openSettings();
      if (button.dataset.command === "tree") toggleTree();
    });
  });
}

function renderFatal(error: unknown): void {
  $("#core-status").textContent = " Core unavailable";
  conversation.innerHTML = `<div class="empty-state"><h2>Could not start Ait daemon</h2><p>${escapeHtml(errorMessage(error))}</p><p>Run <code>cargo build -p ait-daemon</code> and reopen the app.</p></div>`;
  showToast(errorMessage(error), true);
}

function showToast(message: string, error = false): void {
  const toast = $("#toast");
  toast.textContent = message;
  toast.classList.toggle("is-error", error);
  toast.classList.remove("is-hidden");
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => toast.classList.add("is-hidden"), 4_500);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "The operation could not be completed.";
}

function roleLetter(role: DesktopMessage["role"]): string {
  return role === "assistant" ? "A" : role === "user" ? "U" : "S";
}

function shortId(id: string): string {
  return id.slice(0, 8);
}

function relativeTime(timestamp: number): string {
  if (timestamp <= 0) return "saved";
  const delta = Math.max(0, Date.now() - timestamp);
  if (delta < 60_000) return "now";
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m`;
  if (delta < 86_400_000) return `${Math.floor(delta / 3_600_000)}h`;
  return `${Math.floor(delta / 86_400_000)}d`;
}

function formatTime(timestamp: number): string {
  if (timestamp <= 0) return "—";
  return new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" }).format(timestamp);
}

function prettyJson(value: string): string {
  try { return JSON.stringify(JSON.parse(value), null, 2); }
  catch { return value; }
}

function humanize(value: string): string {
  return value.replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[character] ?? character);
}

function escapeAttribute(value: string): string {
  return escapeHtml(value);
}
