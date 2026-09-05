import { messageAuthor } from "./messages.js";
import { buildMessageTimeline, messageText, pathToMessage, resolveBranchHead, sessionForMessage, type TimelineNode } from "./tree.js";
import { agentDisplayName, agentLabel, groupProjects, projectNameFromWorkdir } from "./projects.js";
import type {
  DesktopMessage,
  DesktopSession,
  DesktopSnapshot,
  ReasoningEffort,
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
const projectList = $("#project-list");
const conversation = $("#conversation");
const conversationScroll = $("#conversation-scroll");
const treeList = $("#tree-list");
const treeScroll = $<HTMLElement>("#tree-scroll");
const nodeDetails = $("#node-details");
const messageInput = $<HTMLTextAreaElement>("#message-input");
const composerAgent = $<HTMLSelectElement>("#composer-agent");
const composerReasoning = $<HTMLSelectElement>("#composer-reasoning");
const sendButton = $<HTMLButtonElement>("#send-button");
const settingsDialog = $("#settings-dialog");
const commandDialog = $("#command-dialog");
const projectDialog = $("#project-dialog");
const projectSettingsDialog = $("#project-settings-dialog");
const sessionDialog = $("#session-dialog");

let snapshot: DesktopSnapshot | undefined;
let selectedProjectId: string | undefined;
let selectedSessionId: string | undefined;
let selectedNodeId: string | undefined;
let configuringProjectId: string | undefined;
let viewedTreeHeadId: string | undefined;
let branchPickerNodeId: string | undefined;
let timeline: TimelineNode[] = [];
const sessionReasoningEfforts = new Map<string, ReasoningEffort>();
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
  } catch (error) {
    renderFatal(error);
  }
}

function bindInteractions(): void {
  $("#sidebar-toggle").addEventListener("click", () => appShell.classList.toggle("sidebar-collapsed"));
  $("#tree-toggle").addEventListener("click", toggleTree);
  $("#tree-close").addEventListener("click", () => appShell.classList.add("tree-collapsed"));
  $("#settings-trigger").addEventListener("click", openSettings);
  $("#project-create-trigger").addEventListener("click", openProjectDialog);
  $("#project-close").addEventListener("click", closeProjectDialog);
  $("#project-cancel").addEventListener("click", closeProjectDialog);
  $("#project-settings-close").addEventListener("click", closeProjectSettingsDialog);
  $("#project-settings-cancel").addEventListener("click", closeProjectSettingsDialog);
  $("#session-close").addEventListener("click", closeSessionDialog);
  $("#session-cancel").addEventListener("click", closeSessionDialog);
  $("#project-choose-path").addEventListener("click", () => void chooseProjectPath());
  composerAgent.addEventListener("change", () => void changeSessionAgent());
  composerReasoning.addEventListener("change", () => {
    const session = currentSession();
    if (session) sessionReasoningEfforts.set(session.id, composerReasoning.value as ReasoningEffort);
  });
  $("#project-create").addEventListener("submit", (event) => {
    event.preventDefault();
    void createProject();
  });
  $("#project-settings").addEventListener("submit", (event) => {
    event.preventDefault();
    void saveProjectBackend();
  });
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
  projectSettingsDialog.addEventListener("click", (event) => {
    if (event.target === projectSettingsDialog) closeProjectSettingsDialog();
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
  renderProjects();
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

function resetTreeView(): void {
  selectedNodeId = undefined;
  viewedTreeHeadId = undefined;
  branchPickerNodeId = undefined;
}

function renderProjects(): void {
  if (!snapshot) return;
  if (snapshot.projects.length === 0) {
    projectList.innerHTML = '<div class="project-list-empty"><p>No Projects yet</p><small>Use + above to add a local workspace.</small></div>';
    return;
  }
  projectList.innerHTML = groupProjects(snapshot).map(({ project, sessions }) => {
    const projectSelected = project.id === selectedProjectId;
    const sessionRows = sessions.map((session) => {
      const agent = snapshot?.agents.find((candidate) => candidate.id === session.agentId);
      const selected = session.id === selectedSessionId;
      return `<button class="session-item${selected ? " is-selected" : ""}" type="button" aria-current="${selected ? "page" : "false"}" data-session-id="${escapeAttribute(session.id)}">
        <span class="session-symbol">${session.active ? "◉" : "⑂"}</span>
        <span class="session-copy"><strong>${escapeHtml(session.title)}</strong><small>${escapeHtml(agent ? agentDisplayName(agent) : "Agent")} · ${relativeTime(session.updatedAt)}</small></span>
        ${session.active ? '<i class="running-dot" title="Run active"></i>' : ""}
      </button>`;
    }).join("") || '<p class="project-sessions-empty">No sessions</p>';
    return `<section class="project-group${projectSelected ? " is-current" : ""}" data-project-group="${escapeAttribute(project.id)}">
      <div class="project-row">
        <button class="project-select" type="button" data-project-id="${escapeAttribute(project.id)}" title="${escapeAttribute(project.workdir)}">
          <span class="project-avatar">${escapeHtml(project.name.trim().slice(0, 1).toUpperCase() || "P")}</span>
          <span class="project-copy"><strong>${escapeHtml(project.name)}</strong><small>${escapeHtml(project.workdir)}</small></span>
        </button>
        <div class="project-actions">
          <button class="project-action" type="button" data-project-settings-id="${escapeAttribute(project.id)}" aria-label="Configure ${escapeAttribute(project.name)}">•••</button>
          <button class="project-action" type="button" data-new-session-project-id="${escapeAttribute(project.id)}" aria-label="Create Session in ${escapeAttribute(project.name)}">＋</button>
        </div>
      </div>
      <div class="project-sessions">${sessionRows}</div>
    </section>`;
  }).join("");
  projectList.querySelectorAll<HTMLElement>("[data-project-id]").forEach((button) => {
    button.addEventListener("click", () => selectProject(button.dataset.projectId));
  });
  projectList.querySelectorAll<HTMLElement>("[data-session-id]").forEach((button) => {
    button.addEventListener("click", () => {
      const session = snapshot?.sessions.find((candidate) => candidate.id === button.dataset.sessionId);
      selectedProjectId = session?.projectId;
      selectedSessionId = session?.id;
      resetTreeView();
      renderAll();
    });
  });
  projectList.querySelectorAll<HTMLElement>("[data-new-session-project-id]").forEach((button) => {
    button.addEventListener("click", () => openSessionDialog(button.dataset.newSessionProjectId));
  });
  projectList.querySelectorAll<HTMLElement>("[data-project-settings-id]").forEach((button) => {
    button.addEventListener("click", () => openProjectSettingsDialog(button.dataset.projectSettingsId));
  });
}

function renderAgents(): void {
  if (!snapshot) return;
  const current = currentSession();
  composerAgent.innerHTML = snapshot.agents
    .filter((agent) => agent.enabled)
    .map((agent) => `<option value="${escapeAttribute(agent.id)}"${agent.id === current?.agentId ? " selected" : ""}>${escapeHtml(agentLabel(agent))}</option>`)
    .join("");
  composerAgent.disabled = !current || current.active;
  const agent = snapshot.agents.find((candidate) => candidate.id === current?.agentId);
  $("#agent-chip").textContent = agent ? agentLabel(agent) : "No Agent";
  const efforts = agent?.supportedReasoningEfforts ?? [];
  const savedEffort = current ? sessionReasoningEfforts.get(current.id) : undefined;
  const selectedEffort = savedEffort && efforts.includes(savedEffort)
    ? savedEffort
    : agent?.defaultReasoningEffort;
  composerReasoning.innerHTML = efforts
    .map((effort) => `<option value="${effort}"${effort === selectedEffort ? " selected" : ""}>Reasoning: ${humanize(effort)}</option>`)
    .join("");
  composerReasoning.classList.toggle("is-hidden", efforts.length === 0);
  composerReasoning.disabled = efforts.length === 0 || !current || current.active;
  if (current && selectedEffort) sessionReasoningEfforts.set(current.id, selectedEffort);
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
  conversation.querySelectorAll<HTMLElement>(".message").forEach((item) => {
    item.addEventListener("click", () => selectTreeNode(item.dataset.messageId, false));
    item.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        selectTreeNode(item.dataset.messageId, false);
      }
    });
  });
  requestAnimationFrame(() => {
    conversationScroll.scrollTop = conversationScroll.scrollHeight;
  });
}

function renderMessage(message: DesktopMessage): string {
  const author = messageAuthor(message, snapshot?.agents ?? []);
  const icon = message.role === "assistant" ? author.slice(0, 2).toUpperCase() : message.role === "user" ? "U" : "S";
  const content = message.parts.map((part) => {
    if (part.type === "text") return `<div class="message-content">${escapeHtml(part.text)}</div>`;
    if (part.type === "tool_use") return `<div class="tool-card"><header><span>◇</span><strong>${escapeHtml(part.tool_name)}</strong><small>tool call</small></header><pre>${escapeHtml(prettyJson(part.arguments))}</pre></div>`;
    if (part.type === "file") return `<div class="tool-card"><header><span>＋</span><strong>${escapeHtml(part.name)}</strong><small>${escapeHtml(part.media_type)}</small></header></div>`;
    if (part.type === "structured") return `<div class="tool-card"><header><span>{ }</span><strong>${escapeHtml(part.media_type)}</strong></header><pre>${escapeHtml(part.value)}</pre></div>`;
    return '<div class="message-content">Content redacted</div>';
  }).join("");
  return `<article class="message ${message.role}${message.id === selectedNodeId ? " is-selected" : ""}" data-message-id="${escapeAttribute(message.id)}" role="button" tabindex="0" aria-pressed="${message.id === selectedNodeId}">
    <div class="message-avatar">${escapeHtml(icon)}</div>
    <div class="message-body"><div class="message-heading"><strong>${escapeHtml(author)}</strong><time>${formatTime(message.createdAt)}</time></div>${content}</div>
  </article>`;
}

function renderTree(): void {
  if (!snapshot) return;
  const session = currentSession();
  const messages = snapshot.messages.filter((message) => message.projectId === selectedProjectId);
  timeline = buildMessageTimeline(messages, session, viewedTreeHeadId, selectedNodeId);
  const visible = timeline.slice(0, 2_000);
  treeList.innerHTML = visible.map((node) => {
    const preview = messageText(node.message).replace(/\s+/g, " ").trim() || "Empty message";
    const pickerOpen = branchPickerNodeId === node.message.id;
    const branchPicker = pickerOpen
      ? `<div class="tree-branches" role="group" aria-label="Branches after ${escapeAttribute(preview)}">
          ${node.branches.map((branch, index) => {
            const branchPreview = messageText(branch.message).replace(/\s+/g, " ").trim() || "Empty message";
            return `<button class="tree-branch${branch.active ? " is-active" : ""}" type="button" data-branch-root-id="${escapeAttribute(branch.message.id)}" aria-pressed="${branch.active}" title="${escapeAttribute(branchPreview)}"><span>${index + 1}</span>${escapeHtml(branchPreview)}</button>`;
          }).join("")}
        </div>`
      : "";
    return `<div class="tree-timeline-item" role="none">
      <div class="tree-node${node.selected ? " is-selected" : ""}${node.onCurrentBranch ? " on-current" : ""}" role="treeitem" aria-selected="${node.selected}" tabindex="${node.selected ? "0" : "-1"}" data-message-id="${escapeAttribute(node.message.id)}">
        <span class="tree-marker" aria-hidden="true"></span>
        <span class="tree-role">${roleLetter(node.message.role)}</span>
        <span class="tree-copy"><strong>${escapeHtml(preview)}</strong><small>${node.message.role} · ${formatTime(node.message.createdAt)}</small></span>
        ${node.branches.length > 0 ? `<button class="tree-branch-trigger" type="button" aria-label="Choose branch after this message" aria-expanded="${pickerOpen}">⑂ ${node.branches.length}</button>` : ""}
      </div>
      ${branchPicker}
    </div>`;
  }).join("");
  if (timeline.length > visible.length) {
    treeList.insertAdjacentHTML("beforeend", `<div class="tree-limit">Showing first ${visible.length.toLocaleString()} of ${timeline.length.toLocaleString()} messages</div>`);
  }
  treeList.querySelectorAll<HTMLElement>(".tree-node").forEach((row) => {
    row.addEventListener("click", () => selectTreeNode(row.dataset.messageId));
    row.querySelector(".tree-branch-trigger")?.addEventListener("click", (event) => {
      event.stopPropagation();
      branchPickerNodeId = branchPickerNodeId === row.dataset.messageId ? undefined : row.dataset.messageId;
      renderTree();
      treeList.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(row.dataset.messageId ?? "")}"] .tree-branch-trigger`)?.focus();
    });
  });
  treeList.querySelectorAll<HTMLElement>("[data-branch-root-id]").forEach((button) => {
    button.addEventListener("click", () => switchTreeBranch(button.dataset.branchRootId));
  });
  renderNodeDetails();
}

function selectTreeNode(id: string | undefined, focusTree = true): void {
  if (!id || !snapshot || !selectedProjectId) return;
  selectedNodeId = id;
  const messages = snapshot.messages.filter((message) => message.projectId === selectedProjectId);
  const sessions = snapshot.sessions.filter((session) => session.projectId === selectedProjectId);
  const session = sessionForMessage(messages, sessions, id, selectedSessionId);
  const sessionChanged = session !== undefined && session.id !== selectedSessionId;
  if (session) selectedSessionId = session.id;
  renderProjects();
  if (sessionChanged) {
    renderAgents();
    renderConversation();
  } else {
    syncMessageSelection();
  }
  renderTree();
  updateComposerState();
  if (focusTree) treeList.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(id)}"]`)?.focus();
}

function syncMessageSelection(): void {
  conversation.querySelectorAll<HTMLElement>(".message").forEach((message) => {
    const selected = message.dataset.messageId === selectedNodeId;
    message.classList.toggle("is-selected", selected);
    message.setAttribute("aria-pressed", String(selected));
  });
}

function switchTreeBranch(branchRootId: string | undefined): void {
  if (!snapshot || !branchRootId || !selectedProjectId) return;
  const messages = snapshot.messages.filter((message) => message.projectId === selectedProjectId);
  const sessions = snapshot.sessions.filter((session) => session.projectId === selectedProjectId);
  const headId = resolveBranchHead(messages, sessions, branchRootId);
  if (!headId) return;
  const session = sessionForMessage(messages, sessions, branchRootId, selectedSessionId);
  if (session) selectedSessionId = session.id;
  viewedTreeHeadId = headId;
  branchPickerNodeId = undefined;
  selectedNodeId = undefined;
  renderAll();
  treeList.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(branchRootId)}"]`)?.focus();
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
  const reasoningEffort = selectedReasoningEffort();
  sendButton.disabled = true;
  sendButton.textContent = "…";
  try {
    if (selectedNodeId) {
      const result = await window.ait.fork({
        projectId: session.projectId,
        sourceMessageId: selectedNodeId,
        agentId: composerAgent.value,
        content,
        ...(reasoningEffort ? { reasoningEffort } : {}),
      });
      snapshot = result.snapshot;
      selectedSessionId = result.selectedSessionId;
      showToast("New branch created. The original session was left unchanged.");
    } else {
      snapshot = await window.ait.sendMessage({
        sessionId: session.id,
        expectedVersion: session.version,
        content,
        ...(reasoningEffort ? { reasoningEffort } : {}),
      });
      showToast("Message sent.");
    }
    resetTreeView();
    messageInput.value = "";
    renderAll();
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    sendButton.textContent = "↑";
    updateComposerState();
  }
}

async function changeSessionAgent(): Promise<void> {
  const session = currentSession();
  const agentId = composerAgent.value;
  if (!session || session.active || !agentId || agentId === session.agentId) return;
  composerAgent.disabled = true;
  sendButton.disabled = true;
  try {
    snapshot = await window.ait.setSessionAgent({
      sessionId: session.id,
      agentId,
      expectedVersion: session.version,
    });
    renderAll();
    const agent = snapshot.agents.find((candidate) => candidate.id === agentId);
    showToast(`Session Agent changed to ${agent ? agentDisplayName(agent) : "Agent"}.`);
  } catch (error) {
    renderAll();
    showToast(errorMessage(error), true);
  }
}

function updateComposerState(): void {
  const session = currentSession();
  sendButton.disabled = !session || messageInput.value.trim().length === 0 || session.active;
  messageInput.disabled = !session;
  composerAgent.disabled = !session || session.active;
  composerReasoning.disabled = composerReasoning.options.length === 0 || !session || session.active;
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

function selectedReasoningEffort(): ReasoningEffort | undefined {
  const session = currentSession();
  const agent = snapshot?.agents.find((candidate) => candidate.id === session?.agentId);
  const effort = composerReasoning.value as ReasoningEffort;
  return agent?.supportedReasoningEfforts?.includes(effort) ? effort : undefined;
}

function toggleTree(): void {
  appShell.classList.toggle("tree-collapsed");
}

function handleTreeKeyboard(event: KeyboardEvent): void {
  if (!["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", "Enter"].includes(event.key)) return;
  event.preventDefault();
  const currentIndex = Math.max(0, timeline.findIndex((node) => node.message.id === selectedNodeId));
  if (event.key === "ArrowDown") selectTreeNode(timeline[Math.min(timeline.length - 1, currentIndex + 1)]?.message.id);
  if (event.key === "ArrowUp") selectTreeNode(timeline[Math.max(0, currentIndex - 1)]?.message.id);
  if (event.key === "Enter") messageInput.focus();
  const current = timeline[currentIndex];
  if (event.key === "ArrowLeft" && branchPickerNodeId === current?.message.id) {
    branchPickerNodeId = undefined;
    renderTree();
  }
  if (event.key === "ArrowRight" && current?.branches.length) {
    branchPickerNodeId = current.message.id;
    renderTree();
    treeList.querySelector<HTMLElement>(".tree-branch.is-active")?.focus();
  }
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
    closeProjectSettingsDialog();
    closeSessionDialog();
  }
}

function openProjectDialog(): void {
  if (!snapshot) return;
  const agent = $<HTMLSelectElement>("#project-create-agent");
  agent.innerHTML = agentOptions();
  projectDialog.classList.remove("is-hidden");
  requestAnimationFrame(() => $<HTMLInputElement>("#project-create-name").focus());
}

function closeProjectDialog(): void {
  projectDialog.classList.add("is-hidden");
}

function openProjectSettingsDialog(projectId: string | undefined): void {
  const project = snapshot?.projects.find((candidate) => candidate.id === projectId);
  if (!project) return;
  configuringProjectId = project.id;
  const options = agentOptions();
  const backend = $<HTMLSelectElement>("#project-backend");
  backend.innerHTML = options;
  backend.disabled = options.length === 0;
  $("#project-backend-save").toggleAttribute("disabled", options.length === 0);
  if (project.defaultAgentId) backend.value = project.defaultAgentId;
  $("#project-settings-title").textContent = project.name;
  $("#project-backend-copy").textContent = `New Sessions in ${project.name} use this Agent by default.`;
  projectSettingsDialog.classList.remove("is-hidden");
}

function closeProjectSettingsDialog(): void {
  projectSettingsDialog.classList.add("is-hidden");
  configuringProjectId = undefined;
}

function selectProject(projectId: string | undefined): void {
  if (!snapshot || !projectId || !snapshot.projects.some((project) => project.id === projectId)) return;
  selectedProjectId = projectId;
  selectedSessionId = snapshot.sessions
    .filter((session) => session.projectId === projectId)
    .toSorted((left, right) => right.updatedAt - left.updatedAt)[0]?.id;
  resetTreeView();
  renderAll();
}

async function chooseProjectPath(): Promise<void> {
  const path = await window.ait.chooseProjectDirectory();
  if (path) $<HTMLInputElement>("#project-create-path").value = path;
}

async function createProject(): Promise<void> {
  const workdir = $<HTMLInputElement>("#project-create-path").value;
  const enteredName = $<HTMLInputElement>("#project-create-name").value.trim();
  const name = enteredName || projectNameFromWorkdir(workdir);
  const agentId = $<HTMLSelectElement>("#project-create-agent").value;
  if (!name || !workdir || !agentId) {
    showToast("Choose a directory and backend.", true);
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
    resetTreeView();
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
  const project = snapshot?.projects.find((candidate) => candidate.id === configuringProjectId);
  const agentId = $<HTMLSelectElement>("#project-backend").value;
  if (!project || !agentId) return;
  try {
    snapshot = await window.ait.setProjectDefaultAgent({ projectId: project.id, agentId });
    renderAll();
    closeProjectSettingsDialog();
    showToast(`${project.name} backend updated.`);
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

function openSessionDialog(projectId?: string): void {
  if (projectId) selectProject(projectId);
  const project = currentProject();
  if (!project) {
    openProjectDialog();
    return;
  }
  const select = $<HTMLSelectElement>("#session-agent");
  select.innerHTML = agentOptions();
  if (project.defaultAgentId) select.value = project.defaultAgentId;
  $("#session-project-copy").textContent = `The Session belongs to ${project.name}; its Agent can be changed while idle.`;
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
    resetTreeView();
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
    .map((agent) => `<option value="${escapeAttribute(agent.id)}">${escapeHtml(agentLabel(agent))}</option>`)
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
  const sessions = snapshot?.sessions.filter((session) => session.title.toLowerCase().includes(query)) ?? [];
  const commands = [
    { id: "new-project", title: "Create Project", hint: "" },
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
      const session = snapshot?.sessions.find((candidate) => candidate.id === button.dataset.session);
      selectedProjectId = session?.projectId;
      selectedSessionId = session?.id;
      resetTreeView();
      closeCommandPalette();
      renderAll();
    });
  });
  $("#command-results").querySelectorAll<HTMLElement>("[data-command]").forEach((button) => {
    button.addEventListener("click", () => {
      closeCommandPalette();
      if (button.dataset.command === "new-project") openProjectDialog();
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
