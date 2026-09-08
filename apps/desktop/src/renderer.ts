import { renderProviderSettings, providerChoices } from "./agent-settings.js";
import { createAgentsPage } from "./agents-page.js";
import { bindCodeBlockActions, renderMessage, renderMessageTime, renderRunProgress, renderRunTerminal } from "./message-renderer.js";
import { applyProgressEvent, isTerminalRunEvent, terminalRunForSession } from "./run-progress.js";
import { BoundedRunStreamBacklog } from "./run-event-delivery.js";
import { pendingBranchResolution, type PendingBranch } from "./runs.js";
import { buildMessageTimeline, directMessageChildren, messageText, pathToMessage, resolveBranchHead, sessionForBranch, type TimelineNode } from "./tree.js";
import {
  agentDisplayName,
  agentLabel,
  availableProjectDefaultAgentId,
  groupProjects,
  projectNameFromWorkdir,
} from "./projects.js";
import { PendingSessionTitles, sanitizeSessionPrompt, temporarySessionTitle } from "./session-titles.js";
import { ProjectViewLoader } from "./project-view-loader.js";
import type {
  DesktopMessage,
  DesktopSession,
  DesktopView,
  RunStreamUpdate,
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
const composerConfigTrigger = $<HTMLButtonElement>("#composer-config-trigger");
const composerConfigPanel = $<HTMLElement>("#composer-config-panel");
const composerAgent = $<HTMLSelectElement>("#composer-agent");
const composerProvider = $<HTMLSelectElement>("#composer-provider");
const composerModel = $<HTMLSelectElement>("#composer-model");
const composerReasoning = $<HTMLSelectElement>("#composer-reasoning");
const sendButton = $<HTMLButtonElement>("#send-button");
const settingsDialog = $("#settings-dialog");
const commandDialog = $("#command-dialog");
const projectDialog = $("#project-dialog");
const projectSettingsDialog = $("#project-settings-dialog");
const renameSessionDialog = $("#rename-session-dialog");
const sessionContextMenu = $<HTMLElement>("#session-context-menu");
const messageContextMenu = $<HTMLElement>("#message-context-menu");

let view: DesktopView | undefined;
let selectedProjectId: string | undefined;
let selectedSessionId: string | undefined;
let inspectedNodeId: string | undefined;
let branchSourceNodeId: string | undefined;
let messageContextNodeId: string | undefined;
let pendingBranch: PendingBranch | undefined;
let configuringProjectId: string | undefined;
let renamingSessionId: string | undefined;
let creatingSessionProjectId: string | undefined;
let viewedTreeHeadId: string | undefined;
let configuringSessionId: string | undefined;
let timeline: TimelineNode[] = [];
const pendingSessions = new Set<string>();
let settings: SettingsResponse | undefined;
let settingsDraft: Record<string, unknown> = {};
let settingsCategory: SettingCategory = "models";
let activePage: "sessions" | "agents" = "sessions";
let initialProviderId: string | undefined;
let disposeProviderSettings: (() => void) | undefined;
let toastTimer: number | undefined;
let streamConnected = true;
let renderedSessionId: string | undefined;
let viewRefreshPending = false;
const projectViews = new ProjectViewLoader<DesktopView>((projectId) => window.ait.view(projectId));
const pendingTitles = new PendingSessionTitles();
const pendingStream = new BoundedRunStreamBacklog();
const agentsPage = createAgentsPage($("#agents-page"), {
  update: (updated) => { view = updated; renderAll(); },
  notify: showToast,
  configureProvider: openProviderSettings,
});

window.ait.subscribeRunEvents(handleRunStreamFrame);
void initialize();

async function initialize(): Promise<void> {
  bindInteractions();
  try {
    const [loadedView, loadedSettings] = await Promise.all([
      window.ait.view(),
      window.ait.settings(),
    ]);
    settings = loadedSettings;
    settingsDraft = structuredClone(loadedSettings.values);
    const initialProjectId = loadedView.messages[0]?.projectId
      ?? loadedView.sessions.at(-1)?.projectId
      ?? loadedView.projects[0]?.id;
    replaceProjectView(initialProjectId, loadedView);
    const newestSession = loadedView.sessions
      .filter((session) => session.projectId === selectedProjectId)
      .toSorted((left, right) => right.updatedAt - left.updatedAt)[0]?.id;
    selectedSessionId = newestSession;
    applyPreferences();
    drainPendingStreamUpdates();
    renderAll();
    appShell.classList.remove("is-loading");
    const coreStatus = $("#core-status");
    coreStatus.classList.add("is-ready");
    coreStatus.lastChild!.textContent = " Core ready";
  } catch (error) {
    renderFatal(error);
  }
}

function replaceProjectView(projectId: string | undefined, updated: DesktopView): void {
  projectViews.replace(projectId, updated);
  selectedProjectId = projectId;
  view = updated;
}

function acceptLoadedProjectView(): boolean {
  const projectId = projectViews.projectId;
  if (!projectViews.view || projectId !== projectViews.selectedProjectId) return false;
  selectedProjectId = projectId;
  view = projectViews.view;
  return true;
}

async function selectProjectView(projectId: string): Promise<boolean> {
  selectedProjectId = projectId;
  return await projectViews.select(projectId) && acceptLoadedProjectView();
}

async function ensureProjectView(projectId: string): Promise<boolean> {
  if (projectViews.projectId === projectId && projectViews.selectedProjectId === projectId) {
    selectedProjectId = projectId;
    return true;
  }
  return selectProjectView(projectId);
}

function bindInteractions(): void {
  bindCodeBlockActions(conversation, showToast, async (reference) => {
    const project = currentProject();
    if (!project) throw new Error("No Project is selected.");
    return window.ait.openProjectFile({ projectId: project.id, ...reference });
  });
  $("#sidebar-toggle").addEventListener("click", () => appShell.classList.toggle("sidebar-collapsed"));
  $("#tree-toggle").addEventListener("click", toggleTree);
  $("#settings-trigger").addEventListener("click", openSettings);
  $("#sessions-nav").addEventListener("click", () => showPage("sessions"));
  $("#agents-nav").addEventListener("click", () => showPage("agents"));
  $("#project-create-trigger").addEventListener("click", openProjectDialog);
  $("#project-close").addEventListener("click", closeProjectDialog);
  $("#project-cancel").addEventListener("click", closeProjectDialog);
  $("#project-settings-close").addEventListener("click", closeProjectSettingsDialog);
  $("#project-settings-cancel").addEventListener("click", closeProjectSettingsDialog);
  $("#rename-session-close").addEventListener("click", closeRenameSessionDialog);
  $("#rename-session-cancel").addEventListener("click", closeRenameSessionDialog);
  $("#session-rename-action").addEventListener("click", openRenameSessionDialog);
  $("#message-start-session-action").addEventListener("click", startBranchFromContextMenu);
  $("#project-choose-path").addEventListener("click", () => void chooseProjectPath());
  composerConfigTrigger.addEventListener("click", (event) => {
    event.preventDefault();
    toggleComposerConfig();
  });
  $("#composer-config-close").addEventListener("click", () => composerConfigPanel.hidePopover());
  composerConfigPanel.addEventListener("beforetoggle", (event) => {
    const open = (event as ToggleEvent).newState === "open";
    composerConfigTrigger.setAttribute("aria-expanded", String(open));
    if (!open) configuringSessionId = undefined;
  });
  window.addEventListener("resize", () => composerConfigPanel.hidePopover());
  composerAgent.addEventListener("change", () => void changeSessionAgent());
  composerReasoning.addEventListener("change", () => void changeSessionConfig(false));
  composerModel.addEventListener("change", () => void changeSessionConfig(true));
  composerProvider.addEventListener("change", () => void changeSessionConfig(true, true));
  $("#project-create").addEventListener("submit", (event) => {
    event.preventDefault();
    void createProject();
  });
  $("#project-settings").addEventListener("submit", (event) => {
    event.preventDefault();
    void saveProjectBackend();
  });
  $("#rename-session-form").addEventListener("submit", (event) => {
    event.preventDefault();
    void renameSession();
  });
  $("#settings-close").addEventListener("click", closeSettings);
  $("#settings-cancel").addEventListener("click", closeSettings);
  $("#settings-save").addEventListener("click", () => void saveSettings());
  $("#settings-reset").addEventListener("click", () => void resetSettings());
  $("#clear-branch").addEventListener("click", clearBranchSource);
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
  renameSessionDialog.addEventListener("click", (event) => {
    if (event.target === renameSessionDialog) closeRenameSessionDialog();
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
  document.addEventListener("pointerdown", (event) => {
    if (!sessionContextMenu.contains(event.target as Node)) closeSessionContextMenu();
    if (!messageContextMenu.contains(event.target as Node)) closeMessageContextMenu();
  });
  document.addEventListener("keydown", handleGlobalKeyboard);
}

function renderAll(): void {
  if (!view) return;
  renderRecoveryNotices();
  renderProjects();
  renderAgents();
  renderConversation();
  renderTree();
  updateComposerState();
  agentsPage.render(view);
}

function renderRecoveryNotices(): void {
  if (!view) return;
  const container = $<HTMLElement>("#recovery-notices");
  const notices = view.recoveryNotices ?? [];
  container.classList.toggle("is-hidden", notices.length === 0);
  container.innerHTML = notices.map((notice) => `<button type="button" class="recovery-notice" data-recovery-project="${escapeAttribute(notice.projectId)}"${notice.sessionId ? ` data-recovery-session="${escapeAttribute(notice.sessionId)}"` : ""}>
    <strong>Workspace recovery needs review</strong>
    <span>${escapeHtml(notice.projectName)}${notice.sessionTitle ? ` / ${escapeHtml(notice.sessionTitle)}` : ""} · Run ${escapeHtml(notice.runId.slice(0, 8))}</span>
    <small>${escapeHtml(notice.message)}</small>
  </button>`).join("");
  container.querySelectorAll<HTMLElement>("[data-recovery-project]").forEach((notice) => {
    notice.addEventListener("click", async () => {
      if (pendingBranch) return;
      const projectId = notice.dataset.recoveryProject;
      if (!projectId) return;
      selectedSessionId = notice.dataset.recoverySession;
      resetTreeView();
      const loading = ensureProjectView(projectId);
      showPage("sessions");
      if (!await loading) return;
      renderAll();
    });
  });
}

function showPage(page: "sessions" | "agents"): void {
  activePage = page;
  composerConfigPanel.hidePopover();
  closeSessionContextMenu();
  $("#sessions-page").classList.toggle("is-hidden", page !== "sessions");
  $("#agents-page").classList.toggle("is-hidden", page !== "agents");
  $("#tree-toggle").classList.toggle("is-hidden", page !== "sessions");
  for (const name of ["sessions", "agents"] as const) {
    const button = $(`#${name}-nav`);
    button.classList.toggle("is-active", page === name);
    if (page === name) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  }
  if (page === "agents") $<HTMLElement>("#agents-page-title").focus();
}

function currentSession(): DesktopSession | undefined {
  return view?.sessions.find((session) =>
    session.id === selectedSessionId && session.projectId === selectedProjectId);
}

function currentProject() {
  return view?.projects.find((project) => project.id === selectedProjectId);
}

function resetTreeView(): void {
  inspectedNodeId = undefined;
  branchSourceNodeId = undefined;
  messageContextNodeId = undefined;
  viewedTreeHeadId = undefined;
  closeMessageContextMenu();
}

function renderProjects(): void {
  if (!view) return;
  if (view.projects.length === 0) {
    projectList.innerHTML = '<div class="project-list-empty"><p>No Projects yet</p><small>Use + above to add a local workspace.</small></div>';
    return;
  }
  projectList.innerHTML = groupProjects(view).map(({ project, sessions }) => {
    const projectSelected = project.id === selectedProjectId;
    const sessionRows = sessions.map((session) => {
      const agent = view?.agents.find((candidate) => candidate.id === session.agentId);
      const selected = session.id === selectedSessionId;
      const branchGenerating = session.id === pendingBranch?.sessionId;
      return `<button class="session-item${selected ? " is-selected" : ""}" type="button" aria-current="${selected ? "page" : "false"}" data-session-id="${escapeAttribute(session.id)}"${pendingBranch ? ' disabled' : ""}${branchGenerating ? ' aria-busy="true"' : ""}>
        <span class="session-symbol">${session.active ? "◉" : "⑂"}</span>
        <span class="session-copy"><strong>${escapeHtml(session.title)}</strong><small>${branchGenerating ? "Generating new Session…" : `${escapeHtml(agent ? agentDisplayName(agent) : "Agent")} · ${relativeTime(session.updatedAt)}`}</small></span>
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
          <button class="project-action" type="button" data-new-session-project-id="${escapeAttribute(project.id)}" aria-label="Create Session in ${escapeAttribute(project.name)}"${creatingSessionProjectId === project.id ? ' disabled aria-busy="true"' : ""}>${creatingSessionProjectId === project.id ? "…" : "＋"}</button>
        </div>
      </div>
      <div class="project-sessions">${sessionRows}</div>
    </section>`;
  }).join("");
  projectList.querySelectorAll<HTMLElement>("[data-project-id]").forEach((button) => {
    button.addEventListener("click", () => { void selectProject(button.dataset.projectId); });
  });
  projectList.querySelectorAll<HTMLElement>("[data-session-id]").forEach((button) => {
    button.addEventListener("click", async () => {
      if (pendingBranch) return;
      const session = view?.sessions.find((candidate) => candidate.id === button.dataset.sessionId);
      if (!session) return;
      selectedSessionId = session?.id;
      resetTreeView();
      const loading = ensureProjectView(session.projectId);
      showPage("sessions");
      if (!await loading) return;
      renderAll();
    });
    button.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      openSessionContextMenu(event, button.dataset.sessionId);
    });
  });
  projectList.querySelectorAll<HTMLButtonElement>("[data-new-session-project-id]").forEach((button) => {
    button.addEventListener("click", () => void createSession(button.dataset.newSessionProjectId));
  });
  projectList.querySelectorAll<HTMLElement>("[data-project-settings-id]").forEach((button) => {
    button.addEventListener("click", () => openProjectSettingsDialog(button.dataset.projectSettingsId));
  });
}

function renderAgents(): void {
  if (!view) return;
  const current = currentSession();
  composerAgent.innerHTML = view.agents
    .filter((agent) => agent.enabled && (!agent.ownerSessionId || agent.id === current?.agentId))
    .map((agent) => `<option value="${escapeAttribute(agent.id)}"${agent.id === current?.agentId ? " selected" : ""}>${escapeHtml(agent.ownerSessionId ? "Custom configuration" : agentLabel(agent))}</option>`)
    .join("");
  composerAgent.disabled = !current || current.active;
  const agent = view.agents.find((candidate) => candidate.id === current?.agentId);
  const label = agent ? agentLabel(agent) : "No Agent";
  $("#agent-chip").textContent = label;
  $("#composer-config-label").textContent = label;
  composerConfigTrigger.title = `Configure Agent: ${label}`;
  composerProvider.innerHTML = view.providers.filter((p) => providerChoices([p]).length > 0 || p.id === agent?.config.provider_id).map((p) => `<option value="${escapeAttribute(p.id)}"${p.id === agent?.config.provider_id ? " selected" : ""}>${escapeHtml(p.name)}</option>`).join("");
  const provider = view.providers.find((item) => item.id === agent?.config.provider_id);
  composerModel.innerHTML = provider?.models.map((model) => `<option value="${escapeAttribute(model.id)}"${model.id === agent?.config.model ? " selected" : ""}>${escapeHtml(model.name)}</option>`).join("") ?? "";
  const efforts = agent?.supportedReasoningEfforts ?? [];
  composerReasoning.innerHTML = `<option value="">Reasoning: Default</option>` + efforts
    .map((effort) => `<option value="${escapeAttribute(effort)}"${effort === agent?.config.reasoning_effort ? " selected" : ""}>Reasoning: ${escapeHtml(humanize(effort))}</option>`).join("");
  $("#composer-reasoning-control").classList.toggle("is-hidden", efforts.length === 0);
  updateComposerState();
}

function renderConversation(): void {
  if (!view) return;
  const session = currentSession();
  if (!session) {
    renderedSessionId = undefined;
    $("#session-title").textContent = "No session selected";
    $("#session-breadcrumb").textContent = currentProject()?.name ?? "No Project";
    conversation.innerHTML = `<div class="empty-state"><p>${currentProject() ? "Create or choose a Session to begin." : "Create a Project to begin."}</p></div>`;
    return;
  }
  const project = view.projects.find((candidate) => candidate.id === session.projectId);
  const sameSession = renderedSessionId === session.id;
  const shouldFollow = !sameSession
    || conversationScroll.scrollHeight - conversationScroll.scrollTop - conversationScroll.clientHeight < 80;
  const previousScrollTop = conversationScroll.scrollTop;
  $("#session-title").textContent = session.title;
  $("#session-breadcrumb").textContent = `${project?.name ?? "Project"} / Session v${session.version}`;
  const messages = pathToMessage(
    view.messages.filter((message) => message.projectId === session.projectId),
    session.currentMessageId,
  );
  const progress = session.activeRunId
    ? view.runProgress.find((candidate) => candidate.runId === session.activeRunId && candidate.sessionId === session.id)
    : undefined;
  const agent = view.agents.find((candidate) => candidate.id === session.agentId);
  const live = session.activeRunId
    ? renderRunProgress(progress, agent ? agentDisplayName(agent) : "Assistant", streamConnected)
    : "";
  const latestRun = terminalRunForSession(view, session.id);
  const terminal = latestRun
    ? renderRunTerminal(
      latestRun.status,
      latestRun.error?.message,
      agent ? agentDisplayName(agent) : "Assistant",
    )
    : "";
  conversation.innerHTML = messages.map((message) => renderMessage(message, view!.agents, message.id === inspectedNodeId)).join("") + live + terminal;
  conversation.querySelectorAll<HTMLElement>(".message").forEach((item) => {
    item.addEventListener("click", (event) => {
      if ((event.target as Element).closest("button, a") || window.getSelection()?.toString()) return;
      inspectTreeNode(item.dataset.messageId, false);
    });
    item.addEventListener("keydown", (event) => {
      if (event.target !== item) return;
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        inspectTreeNode(item.dataset.messageId, false);
      }
    });
  });
  requestAnimationFrame(() => {
    if (shouldFollow) {
      const finalAnswer = conversation.querySelector<HTMLElement>(".message:last-child:not(.live-run) [data-codex-final-answer]");
      if (finalAnswer) finalAnswer.scrollIntoView({ block: "start" });
      else conversationScroll.scrollTop = conversationScroll.scrollHeight;
    } else {
      conversationScroll.scrollTop = previousScrollTop;
    }
  });
  renderedSessionId = session.id;
}

function handleRunStreamFrame(updates: RunStreamUpdate[]): void {
  if (!view || viewRefreshPending) {
    queuePendingStreamUpdates(updates);
    return;
  }
  let refresh = false;
  let renderCurrent = false;
  const gappedRuns = new Set<string>();
  for (const update of updates) {
    if (update.type === "connection") {
      streamConnected = update.connected;
      renderCurrent ||= Boolean(currentSession()?.activeRunId);
      continue;
    }
    if (update.type === "resync") {
      refresh = true;
      continue;
    }
    const event = update.event;
    if (event.kind === "run.progress") {
      const body = event.body as Record<string, unknown>;
      const runId = typeof body.run_id === "string" ? body.run_id : undefined;
      if (!runId || gappedRuns.has(runId)) continue;
      const index = view.runProgress.findIndex((candidate) => candidate.runId === runId);
      const current = index >= 0 ? view.runProgress[index] : undefined;
      const incomingSeq = typeof body.seq === "number" ? body.seq : undefined;
      if (incomingSeq !== undefined
        && ((current && incomingSeq > current.seq + 1) || (!current && incomingSeq > 1))) {
        gappedRuns.add(runId);
        refresh = true;
        continue;
      }
      const next = applyProgressEvent(current, body);
      if (!next) continue;
      if (index >= 0) view.runProgress[index] = next;
      else view.runProgress.push(next);
      renderCurrent ||= currentSession()?.activeRunId === runId;
      continue;
    }
    if (event.kind === "run.updated" || event.kind === "run.cancelled") {
      const run = event.body as Record<string, unknown>;
      const runId = typeof run.id === "string" ? run.id : undefined;
      const status = typeof run.status === "string" ? run.status : undefined;
      if (runId && status === "settling") {
        const progress = view.runProgress.find((candidate) => candidate.runId === runId);
        if (progress) progress.status = "settling";
        renderCurrent ||= currentSession()?.activeRunId === runId;
      }
      if (isTerminalRunEvent(event)) {
        refresh = true;
      }
      continue;
    }
    if (event.kind === "stream.reset_required") refresh = true;
  }
  if (refresh) scheduleViewRefresh();
  else if (renderCurrent) renderConversation();
}

function queuePendingStreamUpdates(updates: RunStreamUpdate[]): void {
  pendingStream.push(updates);
}

function drainPendingStreamUpdates(): void {
  const pending = pendingStream.drain();
  if (pending.resync) {
    scheduleViewRefresh();
    return;
  }
  if (pending.updates.length > 0) handleRunStreamFrame(pending.updates);
}

function scheduleViewRefresh(): void {
  if (viewRefreshPending) return;
  viewRefreshPending = true;
  queueMicrotask(async () => {
    let refreshed = false;
    let settled = false;
    try {
      refreshed = await projectViews.refresh();
      settled = true;
      if (refreshed && acceptLoadedProjectView()) {
        reconcilePendingBranch();
        renderAll();
        startReadySessionTitles();
      }
    } catch {
      streamConnected = false;
      if (currentSession()?.activeRunId) renderConversation();
    } finally {
      viewRefreshPending = false;
    }
    if (settled) drainPendingStreamUpdates();
  });
}

function startReadySessionTitles(): void {
  if (!view) return;
  for (const request of pendingTitles.takeReady(view.sessions, view.runs)) {
    void generateFirstSessionTitle(request.sessionId, request.prompt);
  }
}

function reconcilePendingBranch(): void {
  if (!view || !pendingBranch) return;
  const pending = pendingBranch;
  const resolution = pendingBranchResolution(pending, view);
  if (resolution.kind === "pending") return;
  pendingBranch = undefined;
  if (resolution.kind === "ready") {
    selectedSessionId = resolution.sessionId;
    resetTreeView();
    showToast("New Session is ready.");
    return;
  }
  branchSourceNodeId = pending.sourceMessageId;
  inspectedNodeId = pending.sourceMessageId;
  showToast(resolution.message, true);
}

function renderTree(): void {
  if (!view) return;
  const session = currentSession();
  const messages = view.messages.filter((message) => message.projectId === selectedProjectId);
  timeline = buildMessageTimeline(messages, session, viewedTreeHeadId);
  const visible = timeline.slice(0, 2_000);
  treeList.innerHTML = visible.map((node) => {
    const preview = messageText(node.message).replace(/\s+/g, " ").trim() || "Empty message";
    return `<div class="tree-timeline-item" role="none">
      <div class="tree-node${node.onCurrentBranch ? " on-current" : ""}" role="treeitem" tabindex="${node.message.id === inspectedNodeId ? "0" : "-1"}" data-message-id="${escapeAttribute(node.message.id)}"${node.children.length > 0 ? ` title="Right-click to start a new Session from this Message"` : ""}>
        <span class="tree-marker" aria-hidden="true"></span>
        <span class="tree-role">${roleLetter(node.message.role)}</span>
        <span class="tree-copy"><strong>${escapeHtml(preview)}</strong><small>${node.message.role} · ${renderMessageTime(node.message.createdAt)}</small></span>
      </div>
    </div>`;
  }).join("");
  if (timeline.length > visible.length) {
    treeList.insertAdjacentHTML("beforeend", `<div class="tree-limit">Showing first ${visible.length.toLocaleString()} of ${timeline.length.toLocaleString()} messages</div>`);
  }
  treeList.querySelectorAll<HTMLElement>(".tree-node").forEach((row) => {
    row.addEventListener("click", () => inspectTreeNode(row.dataset.messageId));
    row.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      const node = timeline.find((candidate) => candidate.message.id === row.dataset.messageId);
      if (node?.children.length) openMessageContextMenu(event.clientX, event.clientY, node.message.id);
    });
    row.addEventListener("keydown", (event) => {
      if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) return;
      event.preventDefault();
      const node = timeline.find((candidate) => candidate.message.id === row.dataset.messageId);
      if (!node?.children.length) return;
      const bounds = row.getBoundingClientRect();
      openMessageContextMenu(bounds.left + 16, bounds.top + bounds.height, node.message.id);
    });
  });
  renderNodeDetails();
}

function inspectTreeNode(id: string | undefined, focusTree = true): void {
  if (!id || !view || !selectedProjectId) return;
  inspectedNodeId = id;
  renderTree();
  if (focusTree) treeList.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(id)}"]`)?.focus();
}

function switchTreeBranch(branchRootId: string | undefined): void {
  if (!view || !branchRootId || !selectedProjectId || pendingBranch) return;
  const messages = view.messages.filter((message) => message.projectId === selectedProjectId);
  const sessions = view.sessions.filter((session) => session.projectId === selectedProjectId);
  const headId = resolveBranchHead(messages, sessions, branchRootId);
  if (!headId) return;
  const session = sessionForBranch(messages, sessions, branchRootId);
  if (session) selectedSessionId = session.id;
  viewedTreeHeadId = session ? undefined : headId;
  inspectedNodeId = branchRootId;
  branchSourceNodeId = undefined;
  renderAll();
  treeList.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(branchRootId)}"]`)?.focus();
}

function renderNodeDetails(): void {
  const inspected = view?.messages.find((message) => message.id === inspectedNodeId);
  if (!inspected || !view) {
    nodeDetails.innerHTML = '<div class="empty-details"><span>⑂</span><p>Click a Message to inspect its details.</p></div>';
  } else {
    const projectMessages = view.messages.filter((message) => message.projectId === inspected.projectId);
    const children = directMessageChildren(projectMessages, inspected.id);
    const activeChildId = timeline
      .find((node) => node.message.id === inspected.id)
      ?.children.find((child) => child.active)?.message.id;
    const preview = messageText(inspected);
    const source = messageSourceLabel(inspected);
    nodeDetails.innerHTML = `<div class="node-details-header"><span class="node-role-pill">${escapeHtml(inspected.role)}</span><code class="node-id">${escapeHtml(shortId(inspected.id))}</code></div>
      <p class="node-preview">${escapeHtml(preview)}</p>
      <dl class="node-metadata">
        <div><dt>Source</dt><dd>${escapeHtml(source)}</dd></div>
        <div><dt>Created</dt><dd>${renderMessageTime(inspected.createdAt)}</dd></div>
        <div><dt>Git revision</dt><dd>${inspected.gitCommit ? `<code title="${escapeAttribute(inspected.gitCommit)}">${escapeHtml(inspected.gitCommit.slice(0, 12))}</code>` : "Not recorded"}</dd></div>
      </dl>
      <section class="node-children" aria-labelledby="node-children-title">
        <header><strong id="node-children-title">Child paths</strong><span>${children.length}</span></header>
        ${children.length > 0 ? children.map((child) => {
          const childPreview = messageText(child).replace(/\s+/g, " ").trim() || "Empty message";
          const active = child.id === activeChildId;
          return `<button class="${active ? "is-active" : ""}" type="button" data-child-root-id="${escapeAttribute(child.id)}"${active ? ' aria-current="true"' : ""} title="Switch to the longest Session on this child path"><span class="tree-role">${roleLetter(child.role)}</span><span><strong>${escapeHtml(childPreview)}</strong><small>${escapeHtml(child.role)} · ${renderMessageTime(child.createdAt)}</small></span><i aria-hidden="true">${active ? "Current" : "›"}</i></button>`;
        }).join("") : '<p class="node-children-empty">No child Messages</p>'}
      </section>`;
    nodeDetails.querySelectorAll<HTMLElement>("[data-child-root-id]").forEach((button) => {
      button.addEventListener("click", () => switchTreeBranch(button.dataset.childRootId));
    });
  }
  renderBranchContext();
}

function renderBranchContext(): void {
  const sourceId = pendingBranch?.sourceMessageId ?? branchSourceNodeId;
  const source = view?.messages.find((message) => message.id === sourceId);
  const context = $("#branch-context");
  context.classList.toggle("is-hidden", !source);
  if (!source) return;
  const preview = messageText(source).replace(/\s+/g, " ").trim() || "Empty message";
  $("#branch-state-label").textContent = pendingBranch
    ? "Creating new Session from"
    : "Branching new Session from";
  $("#branch-node-label").textContent = `${messageSourceLabel(source)} · ${shortId(source.id)} · ${preview}`;
  const clear = $<HTMLButtonElement>("#clear-branch");
  clear.disabled = Boolean(pendingBranch);
  clear.classList.toggle("is-hidden", Boolean(pendingBranch));
}

function clearBranchSource(): void {
  if (pendingBranch) return;
  branchSourceNodeId = undefined;
  renderBranchContext();
  updateComposerState();
}

async function submitMessage(): Promise<void> {
  const session = currentSession();
  const content = messageInput.value.trim();
  if (!view || !session || !content || sendButton.disabled) return;
  const source = branchSourceNodeId
    ? view.messages.find((message) => message.id === branchSourceNodeId)
    : undefined;
  if (branchSourceNodeId && (!source || directMessageChildren(view.messages, branchSourceNodeId).length === 0)) {
    branchSourceNodeId = undefined;
    renderAll();
    showToast("A new Session can only start from a Message that has replies.", true);
    return;
  }
  pendingSessions.add(session.id);
  updateComposerState();
  sendButton.disabled = true;
  sendButton.textContent = "…";
  try {
    if (source) {
      const result = await window.ait.fork({
        projectId: session.projectId,
        sourceMessageId: source.id,
        agentId: composerAgent.value,
        content,
      });
      view = result.view;
      pendingBranch = {
        sourceMessageId: source.id,
        sessionId: result.selectedSessionId,
        runId: result.runId,
      };
      branchSourceNodeId = undefined;
      pendingTitles.register(result.runId, result.selectedSessionId, content);
      reconcilePendingBranch();
      if (pendingBranch) showToast("Creating the new Session. The current path will stay in place until it is ready.");
    } else {
      const result = await window.ait.sendMessage({
        sessionId: session.id,
        content,
      });
      view = result.view;
      pendingTitles.register(result.runId, session.id, content);
      showToast("Message accepted.");
      resetTreeView();
    }
    startReadySessionTitles();
    scheduleViewRefresh();
    messageInput.value = "";
    renderAll();
  } catch (error) {
    try {
      if (await projectViews.refresh() && acceptLoadedProjectView()) renderAll();
    } catch { /* Keep the last visible view if disconnected. */ }
    showToast(errorMessage(error), true);
  } finally {
    pendingSessions.delete(session.id);
    sendButton.textContent = "↑";
    updateComposerState();
  }
}

async function generateFirstSessionTitle(sessionId: string, prompt: string): Promise<void> {
  const title = temporarySessionTitle(prompt);
  const modelPrompt = sanitizeSessionPrompt(prompt);
  if (!view || !title || !modelPrompt) return;
  try {
    view = await window.ait.setSessionTitle({ sessionId, title });
    renderAll();
    view = await window.ait.generateSessionTitle({ sessionId, prompt: modelPrompt });
    renderAll();
  } catch (error) {
    console.warn("Session title generation failed; keeping the temporary title.", error);
  }
}

function openSessionContextMenu(event: MouseEvent, sessionId?: string): void {
  if (!sessionId) return;
  renamingSessionId = sessionId;
  sessionContextMenu.classList.remove("is-hidden");
  const left = Math.min(event.clientX, window.innerWidth - sessionContextMenu.offsetWidth - 8);
  const top = Math.min(event.clientY, window.innerHeight - sessionContextMenu.offsetHeight - 8);
  sessionContextMenu.style.left = `${Math.max(8, left)}px`;
  sessionContextMenu.style.top = `${Math.max(54, top)}px`;
  $<HTMLButtonElement>("#session-rename-action").focus();
}

function closeSessionContextMenu(): void {
  sessionContextMenu.classList.add("is-hidden");
}

function openMessageContextMenu(left: number, top: number, messageId: string): void {
  if (currentSession()?.active || pendingBranch) return;
  closeSessionContextMenu();
  messageContextNodeId = messageId;
  messageContextMenu.classList.remove("is-hidden");
  const boundedLeft = Math.min(left, window.innerWidth - messageContextMenu.offsetWidth - 8);
  const boundedTop = Math.min(top, window.innerHeight - messageContextMenu.offsetHeight - 8);
  messageContextMenu.style.left = `${Math.max(8, boundedLeft)}px`;
  messageContextMenu.style.top = `${Math.max(54, boundedTop)}px`;
  $<HTMLButtonElement>("#message-start-session-action").focus();
}

function closeMessageContextMenu(): void {
  messageContextMenu.classList.add("is-hidden");
  messageContextNodeId = undefined;
}

function startBranchFromContextMenu(): void {
  const sourceId = messageContextNodeId;
  const source = view?.messages.find((message) => message.id === sourceId);
  const canBranch = source && directMessageChildren(view?.messages ?? [], source.id).length > 0;
  closeMessageContextMenu();
  if (!source || !canBranch || currentSession()?.active || pendingBranch) return;
  inspectedNodeId = source.id;
  branchSourceNodeId = source.id;
  renderTree();
  updateComposerState();
  messageInput.focus();
}

function openRenameSessionDialog(): void {
  const session = view?.sessions.find((candidate) => candidate.id === renamingSessionId);
  closeSessionContextMenu();
  if (!session) return;
  const input = $<HTMLInputElement>("#rename-session-name");
  input.value = session.name;
  renameSessionDialog.classList.remove("is-hidden");
  requestAnimationFrame(() => input.select());
}

function closeRenameSessionDialog(): void {
  renameSessionDialog.classList.add("is-hidden");
}

async function renameSession(): Promise<void> {
  if (!renamingSessionId) return;
  const button = $<HTMLButtonElement>("#rename-session-submit");
  button.disabled = true;
  try {
    view = await window.ait.renameSession({
      sessionId: renamingSessionId,
      name: $<HTMLInputElement>("#rename-session-name").value,
    });
    closeRenameSessionDialog();
    renderAll();
    showToast("Session renamed.");
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    button.disabled = false;
  }
}

function toggleComposerConfig(): void {
  if (composerConfigPanel.matches(":popover-open")) {
    composerConfigPanel.hidePopover();
    return;
  }
  if (composerConfigTrigger.disabled) return;
  configuringSessionId = currentSession()?.id;
  composerConfigPanel.showPopover();
  const trigger = composerConfigTrigger.getBoundingClientRect();
  const panel = composerConfigPanel.getBoundingClientRect();
  composerConfigPanel.style.left = `${Math.max(16, Math.min(trigger.left, window.innerWidth - panel.width - 16))}px`;
  composerConfigPanel.style.top = `${Math.max(16, trigger.top - panel.height - 8)}px`;
  composerAgent.focus();
}

async function changeSessionAgent(): Promise<void> {
  const session = currentSession();
  const agentId = composerAgent.value;
  if (!session || session.active || pendingSessions.has(session.id) || !agentId || agentId === session.agentId) return;
  pendingSessions.add(session.id);
  updateComposerState();
  composerAgent.disabled = true;
  sendButton.disabled = true;
  try {
    view = await window.ait.setSessionAgent({
      sessionId: session.id,
      agentId,
    });
    renderAll();
    const agent = view.agents.find((candidate) => candidate.id === agentId);
    showToast(`Session Agent changed to ${agent ? agentDisplayName(agent) : "Agent"}.`);
  } catch (error) {
    renderAll();
    showToast(errorMessage(error), true);
  } finally { pendingSessions.delete(session.id); updateComposerState(); }
}

function updateComposerState(): void {
  const session = currentSession();
  const busy = !!session && (session.active || pendingSessions.has(session.id) || Boolean(pendingBranch));
  if (!session || session.active || (configuringSessionId && configuringSessionId !== session.id)) {
    composerConfigPanel.hidePopover();
  }
  sendButton.disabled = !session || messageInput.value.trim().length === 0 || busy;
  messageInput.disabled = !session || busy;
  composerConfigTrigger.disabled = !session || busy;
  composerAgent.disabled = !session || busy;
  composerModel.disabled = !session || busy;
  composerProvider.disabled = !session || busy;
  composerReasoning.disabled = composerReasoning.options.length <= 1 || !session || busy;
  messageInput.placeholder = !session
    ? "Create a Session to start…"
    : pendingBranch
      ? "Creating the new Session…"
      : branchSourceNodeId
        ? "Write the first message for the new Session…"
        : busy
          ? "This Session is running…"
          : "Send a message to this Session…";
  $("#composer-hint").textContent = pendingBranch
    ? "The current Session path stays visible until the new Session is fully generated"
    : branchSourceNodeId
      ? "Sending creates a new immutable branch and Session · ⌘ Enter to send"
      : "Send to the current Session · Right-click a Message with replies to start a new Session · ⌘ Enter to send";
}

async function changeSessionConfig(modelChanged: boolean, providerChanged = false): Promise<void> {
  const session = currentSession();
  const agent = view?.agents.find((candidate) => candidate.id === session?.agentId);
  if (!session || !agent || session.active || pendingSessions.has(session.id)) return;
  const provider = view?.providers.find((p) => p.id === composerProvider.value);
  const model = providerChanged ? provider?.models[0] : provider?.models.find((m) => m.id === composerModel.value);
  if (!provider || !model) { renderAgents(); showToast("Configure models for this provider in Settings first.", true); return; }
  const effort = modelChanged ? agent.config.reasoning_effort : composerReasoning.value || null;
  const config = { provider_id: provider.id, model: model.id, reasoning_effort: effort && model?.reasoning_efforts.includes(effort) ? effort : null };
  pendingSessions.add(session.id);
  updateComposerState();
  try {
    view = await window.ait.setSessionConfig({ sessionId: session.id, config });
  } catch (error) { showToast(errorMessage(error), true); }
  finally { pendingSessions.delete(session.id); renderAll(); }
}

function toggleTree(): void {
  setTreeExpanded(appShell.classList.contains("tree-collapsed"));
}

function setTreeExpanded(expanded: boolean): void {
  appShell.classList.toggle("tree-collapsed", !expanded);
  $("#tree-toggle").setAttribute("aria-pressed", String(expanded));
}

function handleTreeKeyboard(event: KeyboardEvent): void {
  if (!["ArrowDown", "ArrowUp", "Enter"].includes(event.key)) return;
  event.preventDefault();
  const foundIndex = timeline.findIndex((node) => node.message.id === inspectedNodeId);
  const currentIndex = foundIndex < 0 ? 0 : foundIndex;
  if (event.key === "ArrowDown") inspectTreeNode(timeline[Math.min(timeline.length - 1, currentIndex + 1)]?.message.id);
  if (event.key === "ArrowUp") inspectTreeNode(timeline[Math.max(0, currentIndex - 1)]?.message.id);
  if (event.key === "Enter") inspectTreeNode(timeline[currentIndex]?.message.id);
}

function handleGlobalKeyboard(event: KeyboardEvent): void {
  if ((event.metaKey || event.ctrlKey) && event.key === "1") {
    event.preventDefault();
    showPage("sessions");
  }
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
    closeSessionContextMenu();
    closeMessageContextMenu();
    closeRenameSessionDialog();
    closeProjectSettingsDialog();
  }
}

function openProjectDialog(): void {
  if (!view) return;
  const agent = $<HTMLSelectElement>("#project-create-agent");
  agent.innerHTML = agentOptions();
  projectDialog.classList.remove("is-hidden");
  requestAnimationFrame(() => $<HTMLInputElement>("#project-create-name").focus());
}

function closeProjectDialog(): void {
  projectDialog.classList.add("is-hidden");
}

function openProjectSettingsDialog(projectId: string | undefined): void {
  const project = view?.projects.find((candidate) => candidate.id === projectId);
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

async function selectProject(projectId: string | undefined): Promise<void> {
  if (!view || pendingBranch || !projectId || !view.projects.some((project) => project.id === projectId)) return;
  selectedSessionId = view.sessions
    .filter((session) => session.projectId === projectId)
    .toSorted((left, right) => right.updatedAt - left.updatedAt)[0]?.id;
  resetTreeView();
  const loading = selectProjectView(projectId);
  showPage("sessions");
  if (!await loading) return;
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
    selectedSessionId = undefined;
    resetTreeView();
    replaceProjectView(result.selectedProjectId, result.view);
    await selectProjectView(result.selectedProjectId);
    $<HTMLInputElement>("#project-create-name").value = "";
    $<HTMLInputElement>("#project-create-path").value = "";
    closeProjectDialog();
    showPage("sessions");
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
  const project = view?.projects.find((candidate) => candidate.id === configuringProjectId);
  const agentId = $<HTMLSelectElement>("#project-backend").value;
  if (!project || !agentId) return;
  try {
    const updated = await window.ait.setProjectDefaultAgent({ projectId: project.id, agentId });
    if (selectedProjectId && selectedProjectId !== project.id) {
      if (!await projectViews.refresh() || !acceptLoadedProjectView()) return;
    } else {
      view = updated;
    }
    renderAll();
    closeProjectSettingsDialog();
    showToast(`${project.name} backend updated.`);
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

async function createSession(projectId = selectedProjectId): Promise<void> {
  if (!view || creatingSessionProjectId || pendingBranch) return;
  const project = view.projects.find((candidate) => candidate.id === projectId);
  if (!project) {
    openProjectDialog();
    return;
  }
  const agentId = availableProjectDefaultAgentId(project, view.agents);
  if (!agentId) {
    showToast(`Set an enabled default Agent for ${project.name} in Project settings.`, true);
    return;
  }
  creatingSessionProjectId = project.id;
  renderProjects();
  try {
    const result = await window.ait.createSession({ projectId: project.id, agentId });
    replaceProjectView(project.id, result.view);
    selectedSessionId = result.selectedSessionId;
    resetTreeView();
    const loading = selectProjectView(project.id);
    showPage("sessions");
    if (!await loading) return;
    renderAll();
    messageInput.focus();
    showToast("Session created.");
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    creatingSessionProjectId = undefined;
    renderProjects();
  }
}

function agentOptions(): string {
  return view?.agents
    .filter((agent) => agent.enabled && !agent.ownerSessionId)
    .map((agent) => `<option value="${escapeAttribute(agent.id)}">${escapeHtml(agentLabel(agent))}</option>`)
    .join("") ?? "";
}

function openSettings(): void {
  if (!settings) return;
  initialProviderId = undefined;
  composerConfigPanel.hidePopover();
  settingsDraft = structuredClone(settings.values);
  settingsDialog.classList.remove("is-hidden");
  renderSettings();
}

function openProviderSettings(id: string): void {
  if (!settings) return;
  initialProviderId = id;
  settingsCategory = "models";
  settingsDraft = structuredClone(settings.values);
  settingsDialog.classList.remove("is-hidden");
  renderSettings();
}

function closeSettings(): void {
  settingsDialog.classList.add("is-hidden");
  disposeProviderSettings?.();
  disposeProviderSettings = undefined;
  initialProviderId = undefined;
}

function renderSettings(): void {
  if (!settings) return;
  disposeProviderSettings?.();
  disposeProviderSettings = undefined;
  const categories = [...new Set<SettingCategory>(["models", ...settings.schema.definitions.map((definition) => definition.category)])];
  const categoryLabel = (category: SettingCategory): string => category === "models" ? "Providers" : category === "agents" ? "Execution" : category;
  $("#settings-nav").innerHTML = categories.map((category) =>
    `<button type="button" data-category="${category}" class="${category === settingsCategory ? "is-active" : ""}">${categoryLabel(category)}</button>`,
  ).join("");
  $("#settings-nav").querySelectorAll<HTMLElement>("[data-category]").forEach((button) => {
    button.addEventListener("click", () => {
      settingsCategory = button.dataset.category as SettingCategory;
      renderSettings();
    });
  });
  const definitions = settings.schema.definitions.filter((definition) => definition.category === settingsCategory);
  $("#settings-fields").innerHTML = definitions.length ? `<header class="settings-section-header"><h3>${categoryLabel(settingsCategory)} preferences</h3></header>${definitions.map(renderSetting).join("")}` : "";
  $("#settings-save").classList.toggle("is-hidden", definitions.length === 0);
  $("#settings-reset").classList.toggle("is-hidden", definitions.length === 0);
  $("#settings-cancel").textContent = settingsCategory === "models" ? "Close" : "Cancel";
  $("#settings-fields").querySelectorAll<HTMLInputElement | HTMLSelectElement>("[data-setting-id]").forEach((control) => {
    control.addEventListener("change", () => readSettingControl(control));
    control.addEventListener("input", () => readSettingControl(control));
  });
  if (view && settingsCategory === "models") {
    disposeProviderSettings = renderProviderSettings($("#settings-fields"), view, (updated, refreshSettings) => {
      view = updated;
      renderAll();
      if (refreshSettings && !settingsDialog.classList.contains("is-hidden")) renderSettings();
    }, showToast, initialProviderId);
    initialProviderId = undefined;
  }
  $("#settings-state").textContent = settingsCategory === "models"
    ? "Connections are saved with your selected models."
    : `Schema ${settings.schema.revision} · state ${settings.revision}`;
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
  setTreeExpanded(settings?.values["interface.session_tree_open"] !== false);
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
  const sessions = view?.sessions.filter((session) =>
    `${session.title} ${session.description}`.toLowerCase().includes(query)) ?? [];
  const commands = [
    { id: "new-project", title: "Create Project", hint: "" },
    { id: "new-session", title: "Create Session", hint: "" },
    { id: "settings", title: "Open Settings", hint: "⌘," },
    { id: "agents", title: "Open Agents", hint: "" },
    { id: "tree", title: "Toggle Session Tree", hint: "" },
  ].filter((command) => command.title.toLowerCase().includes(query) && (command.id !== "tree" || activePage === "sessions"));
  $("#command-results").innerHTML = [
    ...sessions.map((session) => `<button class="command-result" type="button" data-session="${escapeAttribute(session.id)}"><span>⑂</span>${escapeHtml(session.title)}<small>Session</small></button>`),
    ...commands.map((command) => `<button class="command-result" type="button" data-command="${command.id}"><span>›</span>${command.title}<small>${command.hint}</small></button>`),
  ].join("") || '<div class="empty-details"><p>No matching command</p></div>';
  $("#command-results").querySelectorAll<HTMLElement>("[data-session]").forEach((button) => {
    button.addEventListener("click", async () => {
      if (pendingBranch) return;
      const session = view?.sessions.find((candidate) => candidate.id === button.dataset.session);
      if (!session) return;
      selectedSessionId = session?.id;
      resetTreeView();
      closeCommandPalette();
      const loading = ensureProjectView(session.projectId);
      showPage("sessions");
      if (!await loading) return;
      renderAll();
    });
  });
  $("#command-results").querySelectorAll<HTMLElement>("[data-command]").forEach((button) => {
    button.addEventListener("click", () => {
      closeCommandPalette();
      if (button.dataset.command === "new-project") openProjectDialog();
      if (button.dataset.command === "new-session") void createSession();
      if (button.dataset.command === "settings") openSettings();
      if (button.dataset.command === "agents") showPage("agents");
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

function messageSourceLabel(message: DesktopMessage): string {
  if (message.kind === "tool_result") return "Tool result";
  if (message.role === "user") return "Human input";
  if (message.role === "system") return "System";
  const agent = view?.agents.find((candidate) => candidate.id === message.agentId);
  return agent ? `${agentDisplayName(agent)} Agent` : "Agent output";
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
