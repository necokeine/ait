import { renderProviderSettings, providerChoices } from "./agent-settings.js";
import { createAgentsPage, type AgentRole } from "./agents-page.js";
import { renderRunCommit } from "./run-commit.js";
import { createRunsPage } from "./runs-page.js";
import { createCronsPage } from "./crons-page.js";
import { createCodexImportDialog } from "./codex-import.js";
import { bindCodeBlockActions, renderConversationMessages, renderMessageTime, renderRunProgress, renderRunTerminal, replaceConversationContent } from "./message-renderer.js";
import { applyProgressEvent, isTerminalRunEvent, terminalRunForSession } from "./run-progress.js";
import { BoundedRunStreamBacklog } from "./run-event-delivery.js";
import { pendingBranchResolution, runFailure, type PendingBranch, type PendingBranchResolution } from "./runs.js";
import { buildMessageTimeline, directMessageChildren, messageText, pathToMessage, resolveBranchHead, sessionForBranch, type TimelineNode } from "./tree.js";
import {
  agentDisplayName,
  agentLabel,
  availableProjectDefaultAgentId,
  projectCreationInput,
} from "./projects.js";
import { PendingSessionTitles, sanitizeSessionPrompt, temporarySessionTitle } from "./session-titles.js";
import { ProjectSidebar } from "./project-sidebar.js";
import { ProjectViewLoader } from "./project-view-loader.js";
import {
  composeDesktopState,
  emptyProjectView,
  eventBelongsToProject,
  refreshVisibleSlices,
  resolveInitialProjectId,
} from "./desktop-slices.js";
import { expireToolApprovalCards } from "./tool-approval-ui.js";
import { isApprovalEvent, renderPendingApprovals } from "./approval-ui.js";
import { interactionResponse } from "./tool-interaction-ui.js";
import type {
  AgentCatalog,
  DesktopMessage,
  DesktopSession,
  DesktopState,
  ForkReceipt,
  ProjectCatalog,
  ProjectView,
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
const composerPermission = $<HTMLSelectElement>("#composer-permission");
let permissionSaving = false;
const composerReasoning = $<HTMLSelectElement>("#composer-reasoning");
const sendButton = $<HTMLButtonElement>("#send-button");
const settingsDialog = $("#settings-dialog");
const providerDialog = $("#provider-dialog");
const commandDialog = $("#command-dialog");
const projectDialog = $("#project-dialog");
const projectSettingsDialog = $("#project-settings-dialog");
const projectContextMenu = $<HTMLElement>("#project-context-menu");
let projectContextId: string | undefined;
const renameSessionDialog = $("#rename-session-dialog");
const sessionContextMenu = $<HTMLElement>("#session-context-menu");
const messageContextMenu = $<HTMLElement>("#message-context-menu");

let view: DesktopState | undefined;
let projectCatalog: ProjectCatalog | undefined;
let agentCatalog: AgentCatalog | undefined;
let selectedProjectId: string | undefined;
let selectedSessionId: string | undefined;
let inspectedNodeId: string | undefined;
let branchSourceNodeId: string | undefined;
interface NewSessionDraft {
  projectId: string;
  sourceMessageId: string;
  agentId: string;
  submitting: boolean;
  submission?: { id: string; content: string; agentId: string };
  receipt?: ForkReceipt;
}
let newSessionDraft: NewSessionDraft | undefined;
let messageContextNodeId: string | undefined;
interface PendingSessionBranch extends PendingBranch {
  projectId: string;
  sourceSessionId: string;
}
const pendingBranches = new Map<string, PendingSessionBranch>();
let configuringProjectId: string | undefined;
let renamingSessionId: string | undefined;
let renamingSessionProjectId: string | undefined;
let creatingSessionProjectId: string | undefined;
let viewedTreeHeadId: string | undefined;
let branchPickerNodeId: string | undefined;
let configuringSessionId: string | undefined;
let timeline: TimelineNode[] = [];
const pendingSessions = new Set<string>();
let settings: SettingsResponse | undefined;
let settingsDraft: Record<string, unknown> = {};
let selectingSettingPath = false;
let settingsCategory: SettingCategory = "runtime";
let archivedSessions: DesktopSession[] = [];
const archivedSessionLoadingProjects = new Set<string>();
const archivedSessionErrors = new Map<string, string>();
const archivedSessionReadVersions = new Map<string, number>();
let archivedSessionsGeneration = 0;
let activePage: "sessions" | "agents" | "runs" | "crons" = "sessions";
let pageGeneration = 0;
let disposeProviderSettings: (() => void) | undefined;
let toastTimer: number | undefined;
let streamConnected = true;
let renderedSessionId: string | undefined;
let viewRefreshPending = false;
let catalogRefreshPending = false;
const projectViews = new ProjectViewLoader<ProjectView>((projectId) => projectId
  ? window.ait.project(projectId)
  : Promise.resolve(emptyProjectView()));
const sidebar = new ProjectSidebar();
const codexImport = createCodexImportDialog($<HTMLElement>("#codex-import-dialog"), window.ait, (projectId) => {
  void refreshSidebarProject(projectId);
  if (selectedProjectId === projectId && projectViews.projectId === projectId) scheduleViewRefresh();
});
let projectSettingsSaving = false;
const pendingTitles = new PendingSessionTitles();
const pendingStream = new BoundedRunStreamBacklog();
const agentsPage = createAgentsPage($("#agents-page"), {
  update: (updated) => { replaceAgentCatalog(updated); renderAll(); },
  notify: showToast,
  configureProvider: openProviderSettings,
  setRole: saveAgentRole,
});
const runsPage = createRunsPage($("#runs-page"), {
  read: () => window.ait.activeRuns(),
  project: (id) => window.ait.project(id),
  resolve: (input) => window.ait.resolveToolApproval(input),
  retryCommit: (input) => window.ait.retryRunCommit(input),
  resolveInteraction: (input) => window.ait.resolveToolInteraction(input),
  agents: () => view?.agents ?? [],
  openSession: async (projectId, sessionId) => {
    const generation = pageGeneration;
    const navigation = projectViews.beginMutation(projectId);
    try {
      // Prepare both slices without changing the visible Session or its send target.
      const [project, projects] = await Promise.all([window.ait.project(projectId), window.ait.projects()]);
      if (generation !== pageGeneration) {
        projectViews.discardMutation(navigation);
        return;
      }
      if (project.projectId !== projectId || !project.sessions.some((session) => session.id === sessionId)) {
        throw new Error("This Run's Session is no longer available.");
      }
      if (!projectViews.commitMutation(navigation, project)) return;
      replaceProjectCatalog(projects);
      acceptLoadedProjectView();
      selectedSessionId = sessionId;
      resetTreeView();
      renderAll();
      showPage("sessions");
      // A Run may have finished while the catalog read was still in flight.
      scheduleViewRefresh();
    } catch (error) {
      projectViews.discardMutation(navigation);
      throw error;
    }
  },
  notify: showToast,
});
const cronsPage = createCronsPage($("#crons-page"), {
  read: () => window.ait.crons(),
  sessions: (projectId) => window.ait.projectSessions(projectId),
  create: (input) => window.ait.createCron(input),
  setEnabled: (cronId, enabled) => window.ait.setCronEnabled(cronId, enabled),
  trigger: (cronId, scheduledAt) => window.ait.triggerCron(cronId, scheduledAt),
  openRun: async (result) => {
    replaceProjectView(result.project.projectId, result.project);
    selectedSessionId = result.selectedSessionId;
    sidebar.expanded.add(result.project.projectId);
    resetTreeView();
    renderAll();
    showPage("sessions");
  },
  notify: showToast,
});

window.ait.subscribeRunEvents((updates) => {
  refreshSidebarForEvents(updates);
  runsPage.handleUpdates(updates);
  if (updates.some((update) => update.type === "event" && update.event.kind.startsWith("cron."))) {
    cronsPage.refresh();
  }
  reconcileBackgroundBranches(updates);
  handleRunStreamFrame(updates);
});
void initialize();

async function initialize(): Promise<void> {
  bindInteractions();
  try {
    const [loadedProjects, loadedAgents, loadedSettings] = await Promise.all([
      window.ait.projects(),
      window.ait.agents(),
      window.ait.settings(),
    ]);
    projectCatalog = loadedProjects;
    agentCatalog = loadedAgents;
    settings = loadedSettings;
    settingsDraft = structuredClone(loadedSettings.values);
    const rememberedProjectId = window.localStorage.getItem("ait:selected-project") ?? undefined;
    const initialProjectId = resolveInitialProjectId(loadedProjects.projects, rememberedProjectId);
    const loadedProject = initialProjectId
      ? await window.ait.project(initialProjectId)
      : emptyProjectView();
    replaceProjectView(initialProjectId, loadedProject);
    const newestSession = loadedProject.sessions
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

function replaceProjectView(projectId: string | undefined, updated: ProjectView): void {
  projectViews.replace(projectId, updated);
  if (projectId) sidebar.replace(projectId, updated.sessions);
  selectedProjectId = projectId;
  rememberProject(projectId);
  rebuildState();
}

function acceptLoadedProjectView(): boolean {
  const projectId = projectViews.projectId;
  if (!projectViews.view || projectId !== projectViews.selectedProjectId) return false;
  if (projectViews.view.projectId !== (projectId ?? "")) return false;
  selectedProjectId = projectId;
  if (projectId) sidebar.replace(projectId, projectViews.view.sessions);
  if (selectedSessionId && !projectViews.view.sessions.some((session) => session.id === selectedSessionId)) {
    selectedSessionId = projectViews.view.sessions
      .toSorted((left, right) => right.updatedAt - left.updatedAt)[0]?.id;
    resetTreeView();
  }
  rememberProject(projectId);
  rebuildState();
  return true;
}

function replaceAgentCatalog(updated: AgentCatalog): void {
  agentCatalog = updated;
  rebuildState();
}

function replaceProjectCatalog(updated: ProjectCatalog): void {
  projectCatalog = updated;
  rebuildState();
}

function rebuildState(): void {
  view = composeDesktopState(projectCatalog, agentCatalog, projectViews.view);
}

function rememberProject(projectId: string | undefined): void {
  if (projectId) window.localStorage.setItem("ait:selected-project", projectId);
  else window.localStorage.removeItem("ait:selected-project");
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
    const session = currentSession();
    return window.ait.openProjectFile({
      projectId: project.id,
      ...(session ? { sessionId: session.id } : {}),
      ...reference,
    });
  });
  conversation.addEventListener("click", (event) => {
    const interactionButton = (event.target as Element).closest<HTMLButtonElement>("[data-interaction-action]");
    const interactionCard = interactionButton?.closest<HTMLElement>("[data-interaction-id][data-run-id]");
    if (interactionButton && interactionCard) {
      const action = interactionButton.dataset.interactionAction;
      if (action !== "submit" && action !== "approve" && action !== "deny" && action !== "cancel") return;
      let response: Record<string, string | string[]> | undefined;
      try {
        if (action === "submit") response = interactionResponse(interactionCard);
      } catch (error) {
        showToast(errorMessage(error), true);
        return;
      }
      interactionButton.closest("footer")?.querySelectorAll<HTMLButtonElement>("button").forEach((candidate) => {
        candidate.disabled = true;
      });
      void resolveToolInteraction(interactionCard.dataset.runId!, interactionCard.dataset.interactionId!, action, response);
      return;
    }
    const button = (event.target as Element).closest<HTMLButtonElement>("[data-approval-action]");
    const card = button?.closest<HTMLElement>("[data-approval-id][data-run-id]");
    if (!button || !card) return;
    const action = button.dataset.approvalAction;
    const scope = button.dataset.approvalScope;
    if (action !== "approve" && action !== "deny" && action !== "cancel") return;
    if (scope !== undefined && scope !== "one_shot" && scope !== "turn" && scope !== "session") return;
    button.closest("footer")?.querySelectorAll<HTMLButtonElement>("button").forEach((candidate) => {
      candidate.disabled = true;
    });
    void resolveApproval(card.dataset.runId!, card.dataset.approvalId!, action, scope, card.dataset.toolApproval === "true");
  });
  $("#sidebar-toggle").addEventListener("click", () => appShell.classList.toggle("sidebar-collapsed"));
  $("#tree-toggle").addEventListener("click", toggleTree);
  $("#settings-trigger").addEventListener("click", openSettings);
  $("#sessions-nav").addEventListener("click", () => showPage("sessions"));
  $("#runs-nav").addEventListener("click", () => showPage("runs"));
  $("#crons-nav").addEventListener("click", () => showPage("crons"));
  $("#agents-nav").addEventListener("click", () => showPage("agents"));
  $("#project-create-trigger").addEventListener("click", openProjectDialog);
  $("#project-close").addEventListener("click", closeProjectDialog);
  $("#project-cancel").addEventListener("click", closeProjectDialog);
  $("#project-settings-close").addEventListener("click", closeProjectSettingsDialog);
  $("#project-settings-cancel").addEventListener("click", closeProjectSettingsDialog);
  $("#rename-session-close").addEventListener("click", closeRenameSessionDialog);
  $("#rename-session-cancel").addEventListener("click", closeRenameSessionDialog);
  $("#session-rename-action").addEventListener("click", openRenameSessionDialog);
  $("#session-archive-action").addEventListener("click", () => void archiveSession());
  $("#message-start-session-action").addEventListener("click", startBranchFromContextMenu);
  $("#project-choose-path").addEventListener("click", () => void chooseProjectPath());
  $("#project-clear-path").addEventListener("click", () => {
    $<HTMLInputElement>("#project-create-path").value = "";
  });
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
  composerPermission.addEventListener("change", () => void changePermission());
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
    void saveProjectSettings();
  });
  $("#rename-session-form").addEventListener("submit", (event) => {
    event.preventDefault();
    void renameSession();
  });
  $("#settings-close").addEventListener("click", closeSettings);
  $("#settings-cancel").addEventListener("click", closeSettings);
  $("#settings-save").addEventListener("click", () => void saveSettings());
  $("#settings-reset").addEventListener("click", () => void resetSettings());
  $("#provider-dialog-close").addEventListener("click", closeProviderSettings);
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
  providerDialog.addEventListener("click", (event) => {
    if (event.target === providerDialog) closeProviderSettings();
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
    if (!projectContextMenu.contains(event.target as Node)) closeProjectContextMenu();
    if (!sessionContextMenu.contains(event.target as Node)) closeSessionContextMenu();
    if (!messageContextMenu.contains(event.target as Node)) closeMessageContextMenu();
  });
  $("#project-settings-action").addEventListener("click", () => {
    const id = projectContextId;
    closeProjectContextMenu();
    openProjectSettingsDialog(id);
  });
  $("#project-codex-action").addEventListener("click", () => {
    const project = view?.projects.find((candidate) => candidate.id === projectContextId);
    closeProjectContextMenu();
    if (project && view) codexImport.open({ project, agents: view.agents, providers: view.providers });
  });
  projectContextMenu.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const items = [...projectContextMenu.querySelectorAll<HTMLButtonElement>("button")];
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    items[(index + (event.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
  });
  document.addEventListener("keydown", handleGlobalKeyboard);
}

function renderAll(): void {
  if (!view) return;
  reconcileAcceptedDraft();
  reconcilePendingBranch();
  renderRecoveryNotices();
  renderProjects();
  renderAgents();
  renderConversation();
  renderTree();
  updateComposerState();
  agentsPage.render(view, {
    defaultAgentId: String(settings?.values["agents.default_agent"] ?? ""),
    smallAgentId: String(settings?.values["agents.small_agent"] ?? ""),
  });
  cronsPage.render({
    projects: view.projects,
    agents: view.agents,
    selectedProjectId,
    selectedSessionId,
  });
}

function renderRecoveryNotices(): void {
  if (!view) return;
  const container = $<HTMLElement>("#recovery-notices");
  const notices = view.recoveryNotices ?? [];
  const projectName = currentProject()?.name ?? (selectedProjectId ? `Project ${selectedProjectId.slice(0, 8)}` : "Project");
  container.classList.toggle("is-hidden", notices.length === 0);
  container.innerHTML = notices.map((notice) => `<button type="button" class="recovery-notice" data-recovery-project="${escapeAttribute(notice.projectId)}"${notice.sessionId ? ` data-recovery-session="${escapeAttribute(notice.sessionId)}"` : ""}>
    <strong>Workspace recovery needs review</strong>
    <span>${escapeHtml(projectName)}${notice.sessionTitle ? ` / ${escapeHtml(notice.sessionTitle)}` : ""} · Run ${escapeHtml(notice.runId.slice(0, 8))}</span>
    <small>${escapeHtml(notice.message)}</small>
  </button>`).join("");
  container.querySelectorAll<HTMLElement>("[data-recovery-project]").forEach((notice) => {
    notice.addEventListener("click", async () => {
      if (currentPendingBranch()) return;
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

function showPage(page: "sessions" | "agents" | "runs" | "crons"): void {
  pageGeneration += 1;
  activePage = page;
  composerConfigPanel.hidePopover();
  closeSessionContextMenu();
  $("#sessions-page").classList.toggle("is-hidden", page !== "sessions");
  $("#agents-page").classList.toggle("is-hidden", page !== "agents");
  $("#runs-page").classList.toggle("is-hidden", page !== "runs");
  $("#crons-page").classList.toggle("is-hidden", page !== "crons");
  runsPage.setActive(page === "runs");
  cronsPage.setActive(page === "crons");
  $("#tree-toggle").classList.toggle("is-hidden", page !== "sessions");
  for (const name of ["sessions", "runs", "crons", "agents"] as const) {
    const button = $(`#${name}-nav`);
    button.classList.toggle("is-active", page === name);
    if (page === name) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  }
  if (page === "agents") $<HTMLElement>("#agents-page-title").focus();
  if (page === "runs") $<HTMLElement>("#runs-page-title").focus();
  if (page === "crons") $<HTMLElement>("#crons-page-title").focus();
}

function currentSession(): DesktopSession | undefined {
  return view?.sessions.find((session) =>
    session.id === selectedSessionId && session.projectId === selectedProjectId);
}

function currentNewSessionDraft(): NewSessionDraft | undefined {
  return !selectedSessionId && newSessionDraft?.projectId === selectedProjectId ? newSessionDraft : undefined;
}

function currentPendingBranch(): PendingSessionBranch | undefined {
  return Array.from(pendingBranches.values()).find((pending) =>
    pending.projectId === selectedProjectId && pending.sourceSessionId === selectedSessionId);
}

function currentProject() {
  return view?.projects.find((project) => project.id === selectedProjectId);
}

function resetTreeView(): void {
  if (newSessionDraft) messageInput.value = "";
  newSessionDraft = undefined;
  inspectedNodeId = undefined;
  branchSourceNodeId = undefined;
  messageContextNodeId = undefined;
  viewedTreeHeadId = undefined;
  branchPickerNodeId = undefined;
  closeMessageContextMenu();
}

function renderProjects(): void {
  if (!view) return;
  const pendingBranch = currentPendingBranch();
  if (view.projects.length === 0) {
    projectList.innerHTML = '<div class="project-list-empty"><p>No Projects yet</p><small>Use + above to add a local workspace.</small></div>';
    return;
  }
  projectList.innerHTML = view.projects.map((project) => {
    const expanded = sidebar.expanded.has(project.id);
    const sessions = sidebar.sessions.get(project.id)?.toSorted((left, right) => right.updatedAt - left.updatedAt) ?? [];
    const projectSelected = project.id === selectedProjectId;
    const sessionRows = sessions.map((session) => {
      const agent = view?.agents.find((candidate) => candidate.id === session.agentId);
      const selected = projectSelected && session.id === selectedSessionId;
      const branchGenerating = session.id === pendingBranch?.sessionId;
      return `<button class="session-item${selected ? " is-selected" : ""}" type="button" aria-current="${selected ? "page" : "false"}" data-session-id="${escapeAttribute(session.id)}" data-session-project-id="${escapeAttribute(project.id)}"${pendingBranch ? ' disabled' : ""}${branchGenerating ? ' aria-busy="true"' : ""}>
        <span class="session-symbol">${session.active ? "◉" : "⑂"}</span>
        <span class="session-copy"><strong>${escapeHtml(session.title)}</strong><small>${branchGenerating ? "Generating new Session…" : `${escapeHtml(agent ? agentDisplayName(agent) : "Agent")} · ${relativeTime(session.updatedAt)}`}</small></span>
        ${session.active ? '<i class="running-dot" title="Run active"></i>' : ""}
      </button>`;
    }).join("") || `<p class="project-sessions-empty">${sidebar.errors.has(project.id) ? "" : sidebar.sessions.has(project.id) ? "No sessions" : "Loading Sessions…"}</p>`;
    return `<section class="project-group${projectSelected ? " is-current" : ""}" data-project-group="${escapeAttribute(project.id)}">
      <div class="project-row">
        <button class="project-toggle" type="button" data-project-toggle="${escapeAttribute(project.id)}" aria-label="${expanded ? "Collapse" : "Expand"} ${escapeAttribute(project.name)}" aria-expanded="${expanded}">›</button>
        <button class="project-select" type="button" data-project-id="${escapeAttribute(project.id)}" aria-expanded="${expanded}" title="${escapeAttribute(project.workdir)}">
          <span class="project-avatar">${escapeHtml(project.name.trim().slice(0, 1).toUpperCase() || "P")}</span>
          <span class="project-copy"><strong>${escapeHtml(project.name)}</strong><small>${escapeHtml(project.workdir)}</small></span>
        </button>
        <div class="project-actions">
          <button class="project-action" type="button" data-project-settings-id="${escapeAttribute(project.id)}" aria-haspopup="menu" aria-label="Actions for ${escapeAttribute(project.name)}">•••</button>
          <button class="project-action" type="button" data-new-session-project-id="${escapeAttribute(project.id)}" aria-label="Create Session in ${escapeAttribute(project.name)}"${creatingSessionProjectId === project.id ? ' disabled aria-busy="true"' : ""}>${creatingSessionProjectId === project.id ? "…" : "＋"}</button>
        </div>
      </div>
      <div class="project-sessions${expanded ? "" : " is-hidden"}">${sidebar.errors.has(project.id) ? `<p class="project-sessions-empty" role="alert">${escapeHtml(sidebar.errors.get(project.id)!)} <button type="button" data-project-retry="${escapeAttribute(project.id)}">Retry</button></p>` : ""}${sessionRows}</div>
    </section>`;
  }).join("");
  projectList.querySelectorAll<HTMLElement>("[data-project-toggle]").forEach((button) => {
    button.addEventListener("click", () => {
      const id = button.dataset.projectToggle!;
      if (sidebar.expanded.has(id)) sidebar.expanded.delete(id);
      else { sidebar.expanded.add(id); void refreshSidebarProject(id); }
      renderProjects();
    });
  });
  projectList.querySelectorAll<HTMLElement>("[data-project-retry]").forEach((button) => {
    button.addEventListener("click", () => void refreshSidebarProject(button.dataset.projectRetry!));
  });
  projectList.querySelectorAll<HTMLElement>("[data-project-id]").forEach((button) => {
    button.addEventListener("click", () => { void selectProject(button.dataset.projectId); });
  });
  projectList.querySelectorAll<HTMLElement>("[data-session-id]").forEach((button) => {
    button.addEventListener("click", async () => {
      if (pendingBranch) return;
      await openSidebarSession(button.dataset.sessionProjectId!, button.dataset.sessionId!);
    });
    button.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      openSessionContextMenu(event, button.dataset.sessionProjectId, button.dataset.sessionId);
    });
  });
  projectList.querySelectorAll<HTMLButtonElement>("[data-new-session-project-id]").forEach((button) => {
    button.addEventListener("click", () => void createSession(button.dataset.newSessionProjectId));
  });
  projectList.querySelectorAll<HTMLElement>("[data-project-settings-id]").forEach((button) => {
    button.addEventListener("click", () => {
      closeSessionContextMenu();
      closeMessageContextMenu();
      projectContextId = button.dataset.projectSettingsId;
      projectContextMenu.classList.remove("is-hidden");
      const rect = button.getBoundingClientRect();
      projectContextMenu.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - projectContextMenu.offsetWidth - 8))}px`;
      projectContextMenu.style.top = `${Math.max(8, Math.min(rect.bottom + 4, window.innerHeight - projectContextMenu.offsetHeight - 8))}px`;
      projectContextMenu.querySelector<HTMLButtonElement>("button")?.focus();
    });
  });
}

function renderAgents(): void {
  if (!view) return;
  const current = currentSession();
  const agentId = currentNewSessionDraft()?.agentId ?? current?.agentId;
  composerAgent.innerHTML = view.agents
    .filter((agent) => agent.enabled && (!agent.ownerSessionId || agent.id === current?.agentId))
    .map((agent) => `<option value="${escapeAttribute(agent.id)}"${agent.id === agentId ? " selected" : ""}>${escapeHtml(agent.ownerSessionId ? "Custom configuration" : agentLabel(agent))}</option>`)
    .join("");
  if (currentNewSessionDraft()) composerAgent.value = agentId ?? "";
  composerAgent.disabled = !current || current.active;
  const agent = view.agents.find((candidate) => candidate.id === agentId);
  const label = agent ? agentLabel(agent) : "No Agent";
  $("#agent-chip").textContent = label;
  $("#composer-config-label").textContent = label;
  composerProvider.innerHTML = view.providers.filter((p) => providerChoices([p]).length > 0 || p.id === agent?.config.provider_id).map((p) => `<option value="${escapeAttribute(p.id)}"${p.id === agent?.config.provider_id ? " selected" : ""}>${escapeHtml(p.name)}</option>`).join("");
  const provider = view.providers.find((item) => item.id === agent?.config.provider_id);
  composerModel.innerHTML = provider?.models.map((model) => `<option value="${escapeAttribute(model.id)}"${model.id === agent?.config.model ? " selected" : ""}>${escapeHtml(model.name)}</option>`).join("") ?? "";
  const efforts = agent?.supportedReasoningEfforts ?? [];
  const effortLabel = humanize(agent?.config.reasoning_effort ?? "default");
  $("#composer-config-effort").textContent = `· ${effortLabel}`;
  $("#composer-config-effort").classList.toggle("is-hidden", efforts.length === 0);
  composerConfigTrigger.title = `Configure Agent: ${label}${efforts.length ? ` · Reasoning: ${effortLabel}` : ""}`;
  composerReasoning.innerHTML = `<option value="">Provider default</option>` + efforts
    .map((effort) => `<option value="${escapeAttribute(effort)}"${effort === agent?.config.reasoning_effort ? " selected" : ""}>${escapeHtml(humanize(effort))}</option>`).join("");
  $("#composer-reasoning-control").classList.toggle("is-hidden", efforts.length === 0);
  updateComposerState();
  positionComposerConfig();
}

function renderConversation(): void {
  if (!view) return;
  const session = currentSession();
  if (!session) {
    const draft = currentNewSessionDraft();
    renderedSessionId = undefined;
    $("#session-title").textContent = draft ? "New Session" : "No session selected";
    $("#session-breadcrumb").textContent = currentProject()?.name ?? "No Project";
    conversation.innerHTML = `<div class="empty-state"><p>${draft ? "Send your first message to start this Session." : currentProject() ? "Create or choose a Session to begin." : "Create a Project to begin."}</p></div>`;
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
  const activeRun = session.activeRunId
    ? view.runs.find((run) => run.id === session.activeRunId)
    : undefined;
  const approvals = renderPendingApprovals(activeRun);
  const commitRun = view.runs.findLast((run) => run.sessionId === session.id);
  const commit = commitRun ? renderRunCommit(commitRun, !session.activeRunId) : "";
  replaceConversationContent(conversation, renderConversationMessages(messages, view.agents, inspectedNodeId ?? undefined) + approvals + live + terminal + commit, sameSession);
  conversation.querySelector<HTMLButtonElement>("[data-retry-commit]")?.addEventListener("click", async (event) => {
    const button = event.currentTarget as HTMLButtonElement;
    if (!commitRun) return;
    button.disabled = true;
    try {
      await window.ait.retryRunCommit({ projectId: session.projectId, runId: commitRun.id });
      scheduleViewRefresh();
    } catch (error) { button.disabled = false; showToast(error instanceof Error ? error.message : "Git commit retry failed.", true); }
  });
  conversation.querySelectorAll<HTMLElement>(".message").forEach((item) => {
    item.addEventListener("click", (event) => {
      if ((event.target as Element).closest("button, a, summary") || window.getSelection()?.toString()) return;
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
  let refreshCatalogs = false;
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
      refreshCatalogs = true;
      continue;
    }
    const event = update.event;
    if (!eventBelongsToProject(event.body, selectedProjectId)) continue;
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
    if (isApprovalEvent(event.kind)) {
      refresh = true;
      continue;
    }
    if (event.kind === "session.archived" || event.kind === "session.restored") {
      refresh = true;
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
    if (event.kind === "stream.reset_required") {
      refresh = true;
      refreshCatalogs = true;
    }
  }
  if (refresh) scheduleViewRefresh(refreshCatalogs);
  else if (renderCurrent) renderConversation();
}

async function resolveApproval(
  runId: string,
  approvalId: string,
  action: "approve" | "deny" | "cancel",
  scope: "one_shot" | "turn" | "session" | undefined,
  toolApproval = false,
): Promise<void> {
  const projectId = selectedProjectId;
  if (!projectId) return;
  const mutation = projectViews.beginMutation(projectId);
  try {
    const updated = toolApproval ? await window.ait.resolveToolApproval({ runId, projectId, approvalId, action }) : await window.ait.resolveApproval({
      runId,
      projectId,
      approvalId,
      action,
      ...(scope ? { scope } : {}),
    });
    if (!projectViews.commitMutation(mutation, updated) || !acceptLoadedProjectView()) return;
    renderAll();
  } catch (error) {
    projectViews.discardMutation(mutation);
    showToast(errorMessage(error), true);
    scheduleViewRefresh();
  }
}

async function resolveToolInteraction(
  runId: string,
  interactionId: string,
  action: "submit" | "approve" | "deny" | "cancel",
  response?: Record<string, string | string[]>,
): Promise<void> {
  const projectId = selectedProjectId;
  if (!projectId) return;
  const mutation = projectViews.beginMutation(projectId);
  try {
    const updated = await window.ait.resolveToolInteraction({
      runId, projectId, interactionId, action, ...(response ? { response } : {}),
    });
    if (!projectViews.commitMutation(mutation, updated) || !acceptLoadedProjectView()) return;
    renderAll();
  } catch (error) {
    projectViews.discardMutation(mutation);
    showToast(errorMessage(error), true);
    scheduleViewRefresh();
  }
}

function queuePendingStreamUpdates(updates: RunStreamUpdate[]): void {
  pendingStream.push(updates);
}

function drainPendingStreamUpdates(): void {
  const pending = pendingStream.drain();
  if (pending.resync) {
    scheduleViewRefresh(true);
    return;
  }
  if (pending.updates.length > 0) handleRunStreamFrame(pending.updates);
}

function scheduleViewRefresh(includeCatalogs = false): void {
  catalogRefreshPending ||= includeCatalogs;
  if (viewRefreshPending) return;
  viewRefreshPending = true;
  queueMicrotask(async () => {
    let refreshed = false;
    let settled = false;
    try {
      const refreshCatalogs = catalogRefreshPending;
      catalogRefreshPending = false;
      const slices = await refreshVisibleSlices(
        () => projectViews.refresh(),
        window.ait,
        refreshCatalogs,
      );
      if (slices.projects) replaceProjectCatalog(slices.projects);
      if (slices.agents) replaceAgentCatalog(slices.agents);
      refreshed = slices.projectAccepted;
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
    if (catalogRefreshPending && !viewRefreshPending) scheduleViewRefresh(true);
  });
}

function startReadySessionTitles(): void {
  if (!view) return;
  for (const request of pendingTitles.takeReady(view.sessions, view.runs)) {
    void generateFirstSessionTitle(request.sessionId, request.prompt);
  }
}

function reconcilePendingBranch(): void {
  if (!view || projectViews.projectId !== selectedProjectId) return;
  for (const pending of pendingBranches.values()) {
    if (pending.projectId !== selectedProjectId) continue;
    finishPendingBranch(pending, pendingBranchResolution(pending, view));
  }
}

function finishPendingBranch(pending: PendingSessionBranch, resolution: PendingBranchResolution): void {
  if (resolution.kind === "pending") return;
  const followsSource = currentPendingBranch() === pending;
  pendingBranches.delete(pending.runId);
  if (resolution.kind === "ready") {
    if (followsSource) {
      selectedSessionId = resolution.sessionId;
      resetTreeView();
    }
    showToast(followsSource ? "New Session is ready." : `New Session is ready in ${branchProjectName(pending)}.`);
    return;
  }
  if (followsSource) {
    branchSourceNodeId = pending.sourceMessageId;
    inspectedNodeId = pending.sourceMessageId;
  }
  showToast(`${followsSource ? "" : `${branchProjectName(pending)}: `}${resolution.message}`, true);
}

function branchProjectName(pending: PendingSessionBranch): string {
  return projectCatalog?.projects.find((project) => project.id === pending.projectId)?.name ?? "another Project";
}

/** Settle offscreen derivations before the conversation's selected-Project filter. */
function reconcileBackgroundBranches(updates: RunStreamUpdate[]): void {
  for (const update of updates) {
    if (update.type !== "event" || !isTerminalRunEvent(update.event)) continue;
    const run = update.event.body as { id?: string; project_id?: string; status?: string };
    const pending = run.id ? pendingBranches.get(run.id) : undefined;
    if (!pending || pending.projectId !== run.project_id || pending.projectId === selectedProjectId) continue;
    finishPendingBranch(pending, run.status === "completed"
      ? { kind: "ready", sessionId: pending.sessionId }
      : { kind: "failed", message: runFailure(run)?.message ?? "New Session generation failed." });
  }
}

function renderTree(): void {
  if (!view) return;
  const session = currentSession();
  const messages = view.messages.filter((message) => message.projectId === selectedProjectId);
  timeline = buildMessageTimeline(messages, session, viewedTreeHeadId);
  const visible = timeline.slice(0, 2_000);
  treeList.innerHTML = visible.map((node) => {
    const preview = messageText(node.message).replace(/\s+/g, " ").trim() || "Empty message";
    const hasBranches = node.children.length > 1;
    const pickerOpen = hasBranches && branchPickerNodeId === node.message.id;
    const branchPicker = pickerOpen
      ? `<div class="tree-branches" role="group" aria-label="Child paths after ${escapeAttribute(preview)}">
          ${node.children.map((branch, index) => {
            const branchPreview = messageText(branch.message).replace(/\s+/g, " ").trim() || "Empty message";
            return `<button class="tree-branch${branch.active ? " is-active" : ""}" type="button" data-tree-child-root-id="${escapeAttribute(branch.message.id)}" aria-pressed="${branch.active}" title="${escapeAttribute(branchPreview)}"><span>${index + 1}</span>${escapeHtml(branchPreview)}</button>`;
          }).join("")}
        </div>`
      : "";
    return `<div class="tree-timeline-item" role="none">
      <div class="tree-node${node.onCurrentBranch ? " on-current" : ""}" role="treeitem" tabindex="${node.message.id === inspectedNodeId ? "0" : "-1"}" data-message-id="${escapeAttribute(node.message.id)}" title="Right-click to derive from this Message">
        <span class="tree-marker" aria-hidden="true"></span>
        <span class="tree-role">${roleLetter(node.message.role)}</span>
        <span class="tree-copy"><strong>${escapeHtml(preview)}</strong><small>${node.message.role} · ${renderMessageTime(node.message.createdAt)}</small></span>
        ${hasBranches ? `<button class="tree-branch-trigger" type="button" aria-label="Choose among ${node.children.length} child paths" aria-expanded="${pickerOpen}">⑂ ${node.children.length}</button>` : ""}
      </div>
      ${branchPicker}
    </div>`;
  }).join("");
  if (timeline.length > visible.length) {
    treeList.insertAdjacentHTML("beforeend", `<div class="tree-limit">Showing first ${visible.length.toLocaleString()} of ${timeline.length.toLocaleString()} messages</div>`);
  }
  treeList.querySelectorAll<HTMLElement>(".tree-node").forEach((row) => {
    row.addEventListener("click", () => inspectTreeNode(row.dataset.messageId));
    row.querySelector(".tree-branch-trigger")?.addEventListener("click", (event) => {
      event.stopPropagation();
      branchPickerNodeId = branchPickerNodeId === row.dataset.messageId ? undefined : row.dataset.messageId;
      renderTree();
      treeList.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(row.dataset.messageId ?? "")}"] .tree-branch-trigger`)?.focus();
    });
    row.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      const node = timeline.find((candidate) => candidate.message.id === row.dataset.messageId);
      if (node) openMessageContextMenu(event.clientX, event.clientY, node.message.id);
    });
    row.addEventListener("keydown", (event) => {
      if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) return;
      event.preventDefault();
      const node = timeline.find((candidate) => candidate.message.id === row.dataset.messageId);
      if (!node) return;
      const bounds = row.getBoundingClientRect();
      openMessageContextMenu(bounds.left + 16, bounds.top + bounds.height, node.message.id);
    });
  });
  treeList.querySelectorAll<HTMLElement>("[data-tree-child-root-id]").forEach((button) => {
    button.addEventListener("click", () => switchTreeBranch(button.dataset.treeChildRootId));
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
  if (!view || !branchRootId || !selectedProjectId || currentPendingBranch()) return;
  const messages = view.messages.filter((message) => message.projectId === selectedProjectId);
  const sessions = view.sessions.filter((session) => session.projectId === selectedProjectId);
  const headId = resolveBranchHead(messages, sessions, branchRootId);
  if (!headId) return;
  const session = sessionForBranch(messages, sessions, branchRootId);
  resetTreeView();
  if (session) selectedSessionId = session.id;
  viewedTreeHeadId = session ? undefined : headId;
  inspectedNodeId = branchRootId;
  branchSourceNodeId = undefined;
  branchPickerNodeId = undefined;
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
  const pendingBranch = currentPendingBranch();
  const sourceId = pendingBranch?.sourceMessageId ?? currentNewSessionDraft()?.sourceMessageId ?? branchSourceNodeId;
  const source = view?.messages.find((message) => message.id === sourceId);
  const context = $("#branch-context");
  context.classList.toggle("is-hidden", !source);
  if (!source) return;
  const preview = messageText(source).replace(/\s+/g, " ").trim() || "Empty message";
  $("#branch-state-label").textContent = pendingBranch
    ? "Creating new Session from"
    : "Deriving from";
  $("#branch-node-label").textContent = `${messageSourceLabel(source)} · ${shortId(source.id)} · ${preview}`;
  const clear = $<HTMLButtonElement>("#clear-branch");
  const submitting = Boolean(pendingBranch || currentNewSessionDraft()?.submitting);
  clear.disabled = submitting;
  clear.classList.toggle("is-hidden", submitting);
}

function clearBranchSource(): void {
  if (currentPendingBranch() || currentNewSessionDraft()?.submitting) return;
  if (currentNewSessionDraft()) {
    resetTreeView();
    renderAll();
    return;
  }
  branchSourceNodeId = undefined;
  renderBranchContext();
  updateComposerState();
}

async function submitMessage(): Promise<void> {
  const session = currentSession();
  const draft = currentNewSessionDraft();
  const projectId = draft?.projectId ?? session?.projectId;
  const content = messageInput.value.trim();
  if (!view || !projectId || (!session && !draft) || !content || sendButton.disabled) return;
  if (draft) {
    await submitNewSession(draft, content);
    return;
  }
  const sourceId = branchSourceNodeId;
  const source = sourceId
    ? view.messages.find((message) => message.id === sourceId && message.projectId === projectId)
    : undefined;
  if (sourceId && !source) {
    branchSourceNodeId = undefined;
    renderAll();
    showToast("The selected Message is no longer available.", true);
    return;
  }
  if (session) pendingSessions.add(session.id);
  renderBranchContext();
  updateComposerState();
  sendButton.disabled = true;
  sendButton.textContent = "…";
  const mutation = projectViews.beginMutation(projectId);
  try {
    if (source) {
      const result = await window.ait.fork({
        projectId,
        ...(session ? { currentSessionId: session.id } : {}),
        sourceMessageId: source.id,
        agentId: composerAgent.value,
        content,
      });
      if (result.status !== "accepted") throw new Error(result.message);
      // Track accepted work even if a later navigation supersedes its visible view.
      if (session && !result.reusedCurrentSession) {
        pendingBranches.set(result.runId, {
          projectId: session.projectId,
          sourceSessionId: session.id,
          sourceMessageId: source.id,
          sessionId: result.selectedSessionId,
          runId: result.runId,
        });
      }
      const project = await window.ait.project(projectId);
      sidebar.replace(projectId, project.sessions);
      if (!projectViews.commitMutation(mutation, project) || !acceptLoadedProjectView()) return;
      branchSourceNodeId = undefined;
      pendingTitles.register(result.runId, result.selectedSessionId, content);
      if (result.reusedCurrentSession) {
        selectedSessionId = result.selectedSessionId;
        showToast("Message accepted in the current Session.");
        resetTreeView();
      } else {
        reconcilePendingBranch();
        if (currentPendingBranch()) showToast("Creating the new Session. The current path will stay in place until it is ready.");
      }
    } else if (session) {
      const result = await window.ait.sendMessage({
        projectId: session.projectId,
        sessionId: session.id,
        content,
      });
      if (!projectViews.commitMutation(mutation, result.project) || !acceptLoadedProjectView()) return;
      pendingTitles.register(result.runId, session.id, content);
      showToast("Message accepted.");
      resetTreeView();
    }
    startReadySessionTitles();
    scheduleViewRefresh();
    messageInput.value = "";
    renderAll();
  } catch (error) {
    projectViews.discardMutation(mutation);
    try {
      if (await projectViews.refresh() && acceptLoadedProjectView()) renderAll();
    } catch { /* Keep the last visible view if disconnected. */ }
    showToast(errorMessage(error), true);
  } finally {
    if (session) pendingSessions.delete(session.id);
    renderBranchContext();
    sendButton.textContent = "↑";
    updateComposerState();
  }
}

function reconcileAcceptedDraft(): void {
  const draft = currentNewSessionDraft();
  const receipt = draft?.receipt;
  if (!receipt || activePage !== "sessions"
    || !view?.sessions.some((session) => session.id === receipt.selectedSessionId)) return;
  selectedSessionId = receipt.selectedSessionId;
  resetTreeView();
}

async function submitNewSession(draft: NewSessionDraft, content: string): Promise<void> {
  const recovering = Boolean(draft.submission);
  draft.submission ??= { id: crypto.randomUUID(), content, agentId: draft.agentId };
  const submission = draft.submission;
  draft.submitting = true;
  renderAll();
  try {
    if (!draft.receipt) {
      const result = await window.ait.fork({
        projectId: draft.projectId,
        sourceMessageId: draft.sourceMessageId,
        agentId: submission.agentId,
        content: submission.content,
        submissionId: submission.id,
        recover: recovering,
      });
      if (result.status !== "accepted") {
        if (result.status === "rejected") delete draft.submission;
        throw new Error(result.message);
      }
      // Consumption is independent of navigation and Project view generations.
      draft.receipt = result;
      pendingTitles.register(result.runId, result.selectedSessionId, submission.content);
    }
    if (currentNewSessionDraft() === draft) {
      // Refresh does not supersede a newer navigation or commit a stale mutation.
      if (await projectViews.refresh() && acceptLoadedProjectView()) renderAll();
    }
    await refreshSidebarProject(draft.projectId);
    startReadySessionTitles();
  } catch (error) {
    if (currentNewSessionDraft() === draft) showToast(errorMessage(error), true);
  } finally {
    draft.submitting = false;
    renderAll();
  }
}

async function generateFirstSessionTitle(sessionId: string, prompt: string): Promise<void> {
  const title = temporarySessionTitle(prompt);
  const modelPrompt = sanitizeSessionPrompt(prompt);
  const session = view?.sessions.find((candidate) => candidate.id === sessionId);
  if (!session || !title || !modelPrompt) return;
  try {
    // Automatic titles update data, not the user's navigation intent.
    await window.ait.setSessionTitle({ projectId: session.projectId, sessionId, title });
    scheduleViewRefresh();
    await window.ait.generateSessionTitle({ projectId: session.projectId, sessionId, prompt: modelPrompt });
    await refreshSidebarProject(session.projectId);
    scheduleViewRefresh();
  } catch (error) {
    console.warn("Session title generation failed; keeping the temporary title.", error);
  }
}

function openSessionContextMenu(event: MouseEvent, projectId?: string, sessionId?: string): void {
  if (!projectId || !sessionId) return;
  renamingSessionProjectId = projectId;
  renamingSessionId = sessionId;
  const session = sidebar.sessions.get(projectId)?.find((candidate) => candidate.id === sessionId);
  const archive = $<HTMLButtonElement>("#session-archive-action");
  archive.disabled = session?.active ?? false;
  archive.title = session?.active ? "A running Session cannot be archived" : "";
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
  if (currentPendingBranch()) return;
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
  closeMessageContextMenu();
  if (!source || currentPendingBranch() || currentNewSessionDraft()?.submitting) return;
  // A draft has no Session to reuse; keep its chosen source explicit.
  const draft = currentNewSessionDraft();
  if (draft) draft.sourceMessageId = source.id;
  inspectedNodeId = source.id;
  branchSourceNodeId = source.id;
  renderTree();
  updateComposerState();
  messageInput.focus();
}

function openRenameSessionDialog(): void {
  const session = sidebar.sessions.get(renamingSessionProjectId ?? "")?.find((candidate) => candidate.id === renamingSessionId);
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
  const session = sidebar.sessions.get(renamingSessionProjectId ?? "")?.find((candidate) => candidate.id === renamingSessionId);
  if (!session) return;
  const mutation = projectViews.beginMutation(session.projectId);
  const button = $<HTMLButtonElement>("#rename-session-submit");
  button.disabled = true;
  try {
    const updated = await window.ait.renameSession({
      projectId: session.projectId,
      sessionId: renamingSessionId,
      name: $<HTMLInputElement>("#rename-session-name").value,
    });
    sidebar.replace(session.projectId, updated.sessions);
    if (selectedProjectId === session.projectId) {
      if (!projectViews.commitMutation(mutation, updated) || !acceptLoadedProjectView()) return;
    } else projectViews.discardMutation(mutation);
    closeRenameSessionDialog();
    renderAll();
    showToast("Session renamed.");
  } catch (error) {
    projectViews.discardMutation(mutation);
    showToast(errorMessage(error), true);
  } finally {
    button.disabled = false;
  }
}

async function archiveSession(): Promise<void> {
  const sessionId = renamingSessionId;
  const projectId = renamingSessionProjectId;
  const session = sidebar.sessions.get(projectId ?? "")?.find((candidate) => candidate.id === sessionId);
  closeSessionContextMenu();
  if (!sessionId || !projectId || !session) return;
  const mutation = projectViews.beginMutation(projectId);
  try {
    const updated = await window.ait.setSessionArchived({ projectId, sessionId, archived: true });
    sidebar.replace(projectId, updated.sessions);
    if (selectedProjectId === projectId) {
      if (!projectViews.commitMutation(mutation, updated) || !acceptLoadedProjectView()) return;
      if (selectedSessionId === sessionId) {
        selectedSessionId = updated.sessions.toSorted((left, right) => right.updatedAt - left.updatedAt)[0]?.id;
        resetTreeView();
      }
    } else projectViews.discardMutation(mutation);
    renderAll();
    showToast("Session archived.");
  } catch (error) {
    projectViews.discardMutation(mutation);
    showToast(errorMessage(error), true);
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
  positionComposerConfig();
  composerAgent.focus();
}

function positionComposerConfig(): void {
  if (!composerConfigPanel.matches(":popover-open")) return;
  const trigger = composerConfigTrigger.getBoundingClientRect();
  const panel = composerConfigPanel.getBoundingClientRect();
  composerConfigPanel.style.left = `${Math.max(16, Math.min(trigger.left, window.innerWidth - panel.width - 16))}px`;
  composerConfigPanel.style.top = `${Math.max(16, trigger.top - panel.height - 8)}px`;
}

async function changeSessionAgent(): Promise<void> {
  const session = currentSession();
  const agentId = composerAgent.value;
  const draft = currentNewSessionDraft();
  if (draft) {
    if (!draft.submission && agentId) draft.agentId = agentId;
    renderAgents();
    return;
  }
  if (!session || session.active || pendingSessions.has(session.id) || !agentId || agentId === session.agentId) return;
  pendingSessions.add(session.id);
  updateComposerState();
  composerAgent.disabled = true;
  sendButton.disabled = true;
  const mutation = projectViews.beginMutation(session.projectId);
  try {
    const updated = await window.ait.setSessionAgent({
      projectId: session.projectId,
      sessionId: session.id,
      agentId,
    });
    if (!projectViews.commitMutation(mutation, updated) || !acceptLoadedProjectView()) return;
    renderAll();
    const agent = view?.agents.find((candidate) => candidate.id === agentId);
    showToast(`Session Agent changed to ${agent ? agentDisplayName(agent) : "Agent"}.`);
  } catch (error) {
    projectViews.discardMutation(mutation);
    renderAll();
    showToast(errorMessage(error), true);
  } finally { pendingSessions.delete(session.id); updateComposerState(); }
}

function updateComposerState(): void {
  const session = currentSession();
  const draft = currentNewSessionDraft();
  const hasTarget = Boolean(session || draft);
  const draftAgentAvailable = !draft || view?.agents.some((agent) => agent.id === draft.agentId && agent.enabled && !agent.ownerSessionId);
  const pendingBranch = currentPendingBranch();
  const deriving = Boolean(branchSourceNodeId);
  const submissionBusy = Boolean(draft?.submitting || session
    && (pendingSessions.has(session.id) || Boolean(pendingBranch) || session.active && !deriving));
  const configBusy = Boolean(draft?.submission || session
    && (session.active || pendingSessions.has(session.id) || Boolean(pendingBranch)));
  if (!hasTarget || session?.active || (configuringSessionId && configuringSessionId !== session?.id)) {
    composerConfigPanel.hidePopover();
  }
  sendButton.disabled = !hasTarget || (!draft?.submission && (!draftAgentAvailable || messageInput.value.trim().length === 0)) || submissionBusy || permissionSaving;
  composerPermission.disabled = !settings || !hasTarget || submissionBusy || Boolean(draft?.submission) || permissionSaving;
  if (!permissionSaving) {
    const sandbox = session?.active && !deriving
      ? view?.runs.find((run) => run.id === session.activeRunId)?.permissionProfile.sandbox
      : settings?.values["permissions.sandbox"];
    composerPermission.value = sandbox === "workspace_write" || sandbox === "full_access" ? sandbox : "read_only";
  }
  messageInput.disabled = !hasTarget || submissionBusy || Boolean(draft?.submission);
  const sendLabel = draft?.receipt ? "Open Session" : draft?.submission ? "Check submission" : "Send message";
  sendButton.textContent = draft?.submitting ? "…" : draft?.submission ? "↻" : "↑";
  sendButton.title = sendLabel;
  sendButton.setAttribute("aria-label", sendLabel);
  composerConfigTrigger.disabled = !hasTarget || configBusy;
  composerAgent.disabled = !hasTarget || configBusy;
  composerModel.disabled = !session || configBusy;
  composerProvider.disabled = !session || configBusy;
  composerReasoning.disabled = composerReasoning.options.length <= 1 || !session || configBusy;
  messageInput.placeholder = !hasTarget
    ? "Create a Session to start…"
    : pendingBranch || draft?.submitting
      ? "Creating the new Session…"
      : branchSourceNodeId
        ? "Write a message derived from this point…"
        : submissionBusy
          ? "This Session is running…"
          : "Send a message to this Session…";
  $("#composer-hint").textContent = draft?.receipt
    ? "Message accepted. Retry opening the Session."
    : draft?.submission && !draft.submitting
      ? "Confirming your first message. Retry to check its status."
      : draft
        ? "Sending your first message creates this Session · ⌘ Enter to send"
        : pendingBranch
          ? "The current Session path stays visible until the new Session is fully generated"
          : branchSourceNodeId
            ? "A current leaf continues this Session when idle; otherwise sending creates a new Session · ⌘ Enter to send"
            : "Send to the current Session · Right-click any Message to derive from it · ⌘ Enter to send";
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
  const mutation = projectViews.beginMutation(session.projectId);
  try {
    const updated = await window.ait.setSessionConfig({ projectId: session.projectId, sessionId: session.id, config });
    replaceAgentCatalog(updated.agents);
    if (!projectViews.commitMutation(mutation, updated.project) || !acceptLoadedProjectView()) return;
  } catch (error) { projectViews.discardMutation(mutation); showToast(errorMessage(error), true); }
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
    closeProjectContextMenu();
    codexImport.close();
    closeSettings();
    closeProviderSettings();
    closeCommandPalette();
    closeProjectDialog();
    closeSessionContextMenu();
    closeMessageContextMenu();
    closeRenameSessionDialog();
    closeProjectSettingsDialog();
  }
}

function closeProjectContextMenu(): void {
  if (projectContextMenu.contains(document.activeElement)) {
    [...projectList.querySelectorAll<HTMLElement>("[data-project-settings-id]")]
      .find((button) => button.dataset.projectSettingsId === projectContextId)?.focus();
  }
  projectContextMenu.classList.add("is-hidden");
  projectContextId = undefined;
}

function openProjectDialog(): void {
  if (!view) return;
  const agent = $<HTMLSelectElement>("#project-create-agent");
  agent.innerHTML = `<option value="">Use global Default Agent</option>${agentOptions()}`;
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
  const availableAgentId = availableProjectDefaultAgentId(project, view?.agents ?? []);
  const unavailableAgent = project.defaultAgentId && !availableAgentId
    ? `<option value="${escapeAttribute(project.defaultAgentId)}" disabled>Unavailable current Agent (kept until changed)</option>`
    : "";
  backend.innerHTML = `${unavailableAgent}<option value="">Use global Default Agent</option>${options}`;
  backend.disabled = false;
  $<HTMLButtonElement>("#project-backend-save").disabled = false;
  $<HTMLInputElement>("#project-settings-name").value = project.name;
  backend.value = availableAgentId ?? project.defaultAgentId ?? "";
  backend.dataset.initialAgentId = backend.value;
  $("#project-settings-title").textContent = project.name;
  $("#project-backend-copy").textContent = `New Sessions in ${project.name} use this Agent by default.`;
  projectSettingsDialog.classList.remove("is-hidden");
}

function closeProjectSettingsDialog(): void {
  if (projectSettingsSaving) return;
  projectSettingsDialog.classList.add("is-hidden");
  configuringProjectId = undefined;
}

async function openSidebarSession(projectId: string, sessionId?: string): Promise<void> {
  const navigation = projectViews.beginMutation(projectId);
  const generation = pageGeneration;
  try {
    const project = await window.ait.project(projectId);
    if (generation !== pageGeneration) { projectViews.discardMutation(navigation); return; }
    if (project.projectId !== projectId || (sessionId && !project.sessions.some((session) => session.id === sessionId))) {
      throw new Error("This Session is no longer available.");
    }
    if (!projectViews.commitMutation(navigation, project) || !acceptLoadedProjectView()) return;
    selectedSessionId = sessionId ?? project.sessions.toSorted((left, right) => right.updatedAt - left.updatedAt)[0]?.id;
    resetTreeView();
    showPage("sessions");
    renderAll();
    scheduleViewRefresh();
  } catch (error) {
    if (!projectViews.discardMutation(navigation)) return;
    sidebar.errors.set(projectId, errorMessage(error));
    renderProjects();
    showToast(errorMessage(error), true);
  }
}

async function selectProject(projectId: string | undefined): Promise<void> {
  if (!view || currentPendingBranch() || !projectId || !view.projects.some((project) => project.id === projectId)) return;
  if (projectId === selectedProjectId && sidebar.expanded.has(projectId)) {
    sidebar.expanded.delete(projectId);
    renderProjects();
    showPage("sessions");
    return;
  }
  sidebar.expanded.add(projectId);
  renderProjects();
  await openSidebarSession(projectId);
}

async function refreshSidebarProject(projectId: string): Promise<void> {
  await sidebar.refresh(projectId, (id) => window.ait.projectSessions(id));
  renderProjects();
}

function refreshSidebarForEvents(updates: RunStreamUpdate[]): void {
  if (!view) return;
  const affected = new Set<string>();
  for (const update of updates) {
    if (update.type === "resync" || (update.type === "event" && update.event.kind === "stream.reset_required")) {
      for (const id of sidebar.expanded) affected.add(id);
    } else if (update.type === "event") {
      const { kind, body } = update.event;
      if (kind.startsWith("project.")) scheduleViewRefresh(true);
      if ((kind.startsWith("session.") || kind === "run.updated" || kind === "run.cancelled")
        && typeof body === "object" && body !== null && "project_id" in body && typeof body.project_id === "string") {
        if (sidebar.expanded.has(body.project_id)) affected.add(body.project_id);
      }
    }
  }
  for (const id of affected) void refreshSidebarProject(id);
}

async function chooseProjectPath(): Promise<void> {
  const path = await window.ait.chooseProjectDirectory();
  if (path) $<HTMLInputElement>("#project-create-path").value = path;
}

async function createProject(): Promise<void> {
  const workdir = $<HTMLInputElement>("#project-create-path").value;
  const enteredName = $<HTMLInputElement>("#project-create-name").value.trim();
  const agentId = $<HTMLSelectElement>("#project-create-agent").value;
  let input;
  try {
    input = projectCreationInput(enteredName, workdir, agentId);
  } catch (error) {
    showToast(errorMessage(error), true);
    return;
  }
  const button = $<HTMLButtonElement>("#project-create-submit");
  button.disabled = true;
  button.textContent = "Creating…";
  try {
    const result = await window.ait.createProject(input);
    selectedSessionId = undefined;
    resetTreeView();
    replaceProjectCatalog(result.catalog);
    replaceProjectView(result.selectedProjectId, result.project);
    sidebar.expanded.add(result.selectedProjectId);
    $<HTMLInputElement>("#project-create-name").value = "";
    $<HTMLInputElement>("#project-create-path").value = "";
    closeProjectDialog();
    showPage("sessions");
    renderAll();
    showToast(`${input.name} created with the selected backend.`);
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    button.disabled = false;
    button.textContent = "Create Project";
  }
}

async function saveProjectSettings(): Promise<void> {
  const project = view?.projects.find((candidate) => candidate.id === configuringProjectId);
  if (!project || projectSettingsSaving) return;
  const backend = $<HTMLSelectElement>("#project-backend");
  const agentId = backend.value;
  const name = $<HTMLInputElement>("#project-settings-name").value.trim();
  if (!name) { showToast("Enter a Project name.", true); return; }
  const controls = Array.from(projectSettingsDialog.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>("input, select, button"));
  const disabled = controls.map((control) => control.disabled);
  projectSettingsSaving = true;
  controls.forEach((control) => { control.disabled = true; });
  try {
    const updated = await window.ait.updateProject({
      projectId: project.id,
      name,
      ...(agentId !== backend.dataset.initialAgentId ? { agentId } : {}),
    });
    replaceProjectCatalog(updated);
    renderAll();
    projectSettingsSaving = false;
    closeProjectSettingsDialog();
    showToast("Project updated.");
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    projectSettingsSaving = false;
    controls.forEach((control, index) => { control.disabled = disabled[index]!; });
  }
}

async function createSession(projectId = selectedProjectId): Promise<void> {
  if (!view || creatingSessionProjectId) return;
  const project = view.projects.find((candidate) => candidate.id === projectId);
  if (!project) {
    openProjectDialog();
    return;
  }
  const configuredAgentId = project.defaultAgentId ?? settings?.values["agents.default_agent"];
  const agentId = typeof configuredAgentId === "string"
    && view.agents.some((agent) => agent.id === configuredAgentId && agent.enabled && !agent.ownerSessionId)
    ? configuredAgentId
    : undefined;
  if (!agentId) {
    showToast(`Set an enabled default Agent for ${project.name} in Project settings or Settings.`, true);
    return;
  }
  creatingSessionProjectId = project.id;
  const mutation = projectViews.beginMutation(project.id);
  const generation = pageGeneration;
  renderProjects();
  try {
    const loaded = await window.ait.project(project.id);
    if (generation !== pageGeneration) {
      projectViews.discardMutation(mutation);
      return;
    }
    if (loaded.projectId !== project.id) throw new Error("The selected Project is unavailable.");
    const source = loaded.messages.find((message) => message.id === project.rootMessageId
      && message.projectId === project.id && message.role === "system" && !message.parentMessageId);
    if (!source) throw new Error("The Project's initial system Message is unavailable.");
    if (!projectViews.commitMutation(mutation, loaded)) return;
    if (!acceptLoadedProjectView()) return;
    selectedSessionId = undefined;
    sidebar.expanded.add(project.id);
    resetTreeView();
    newSessionDraft = { projectId: project.id, sourceMessageId: source.id, agentId, submitting: false };
    branchSourceNodeId = source.id;
    inspectedNodeId = source.id;
    viewedTreeHeadId = source.id;
    messageInput.value = "";
    showPage("sessions");
    renderAll();
    messageInput.focus();
  } catch (error) {
    projectViews.discardMutation(mutation);
    try {
      if (await projectViews.refresh() && acceptLoadedProjectView()) renderAll();
    } catch { /* Keep the last visible view if disconnected. */ }
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
  closeProviderSettings();
  composerConfigPanel.hidePopover();
  settingsDraft = structuredClone(settings.values);
  settingsDialog.classList.remove("is-hidden");
  renderSettings();
  if (settingsCategory === "archived_sessions") void loadArchivedSessions();
}

function openProviderSettings(id: string): void {
  if (!view) return;
  closeSettings();
  disposeProviderSettings?.();
  $("#provider-dialog-body").replaceChildren();
  providerDialog.classList.remove("is-hidden");
  disposeProviderSettings = renderProviderSettings($("#provider-dialog-body"), view, (updated, editorIsCurrent) => {
    replaceAgentCatalog(updated);
    renderAll();
    if (editorIsCurrent) closeProviderSettings();
  }, showToast, id);
}

function closeSettings(): void {
  archivedSessionsGeneration += 1;
  archivedSessionLoadingProjects.clear();
  settingsDialog.classList.add("is-hidden");
}

function closeProviderSettings(): void {
  providerDialog.classList.add("is-hidden");
  disposeProviderSettings?.();
  disposeProviderSettings = undefined;
}

function renderSettings(): void {
  if (!settings) return;
  const configuredCategories = settings.schema.definitions
    .map((definition) => definition.category)
    .filter((category) => category !== "agents" && category !== "network");
  const categories = [...new Set<SettingCategory>([...configuredCategories, "archived_sessions"])];
  if (!categories.includes(settingsCategory)) settingsCategory = categories[0] ?? "archived_sessions";
  const categoryLabel = (category: SettingCategory): string => category === "archived_sessions" ? "Archived sessions" : category;
  $("#settings-nav").innerHTML = categories.map((category) =>
    `<button type="button" data-category="${category}" class="${category === settingsCategory ? "is-active" : ""}">${categoryLabel(category)}</button>`,
  ).join("");
  $("#settings-nav").querySelectorAll<HTMLElement>("[data-category]").forEach((button) => {
    button.addEventListener("click", () => {
      settingsCategory = button.dataset.category as SettingCategory;
      renderSettings();
      if (settingsCategory === "archived_sessions") void loadArchivedSessions();
    });
  });
  if (settingsCategory === "archived_sessions") {
    renderArchivedSessions();
    return;
  }
  const definitions = settings.schema.definitions.filter((definition) => definition.category === settingsCategory);
  $("#settings-fields").innerHTML = definitions.length ? `<header class="settings-section-header"><h3>${categoryLabel(settingsCategory)} preferences</h3></header>${definitions.map(renderSetting).join("")}` : "";
  $("#settings-save").classList.toggle("is-hidden", definitions.length === 0);
  $("#settings-reset").classList.toggle("is-hidden", definitions.length === 0);
  $("#settings-cancel").textContent = "Cancel";
  $("#settings-fields").querySelectorAll<HTMLInputElement | HTMLSelectElement>("input[data-setting-id], select[data-setting-id]").forEach((control) => {
    control.addEventListener("change", () => readSettingControl(control));
    control.addEventListener("input", () => readSettingControl(control));
  });
  $("#settings-fields").querySelectorAll<HTMLButtonElement>("button[data-setting-id]").forEach((control) => {
    control.disabled = selectingSettingPath;
    control.addEventListener("click", () => void chooseSettingPath(control));
  });
  $("#settings-state").textContent = `Schema ${settings.schema.revision} · state ${settings.revision}`;
}

function renderArchivedSessions(): void {
  const fields = $("#settings-fields");
  $("#settings-save").classList.add("is-hidden");
  $("#settings-reset").classList.add("is-hidden");
  $("#settings-cancel").textContent = "Close";
  const projects = view?.projects ?? [];
  if (projects.length === 0) {
    fields.innerHTML = '<header class="settings-section-header"><h3>Archived Sessions</h3><p>No Projects are configured.</p></header>';
    $("#settings-state").textContent = "0 archived Sessions";
    return;
  }
  const groups = projects.map((project) => {
    const failure = archivedSessionErrors.get(project.id);
    const loading = archivedSessionLoadingProjects.has(project.id);
    const sessions = archivedSessions
      .filter((session) => session.projectId === project.id)
      .toSorted((left, right) => right.updatedAt - left.updatedAt);
    const rows = `${loading ? '<p class="archived-empty">Loading archived Sessions…</p>' : ""}
      ${failure ? `<p class="archived-error" role="alert">${escapeHtml(failure)} <button class="secondary-button" type="button" data-archived-retry="${escapeAttribute(project.id)}">Retry</button></p>` : ""}
      ${sessions.map((session) => `<div class="archived-session-row">
        <span><strong>${escapeHtml(session.title)}</strong><small>${relativeTime(session.updatedAt)}</small></span>
        <button class="secondary-button" type="button" data-restore-session="${escapeAttribute(session.id)}" data-restore-project="${escapeAttribute(project.id)}">Restore</button>
      </div>`).join("")}
      ${!loading && !failure && sessions.length === 0 ? '<p class="archived-empty">No archived Sessions.</p>' : ""}`;
    return `<section class="archived-project" data-archived-project="${escapeAttribute(project.id)}"><header><h4>${escapeHtml(project.name)}</h4><small>${sessions.length} archived</small></header>${rows}</section>`;
  }).join("");
  fields.innerHTML = `<header class="settings-section-header"><h3>Archived Sessions</h3><p>Restore a Session to return it to its Project.</p></header><div class="archived-project-list">${groups}</div>`;
  fields.querySelectorAll<HTMLButtonElement>("[data-archived-retry]").forEach((button) => {
    button.addEventListener("click", () => void loadArchivedSessions(button.dataset.archivedRetry));
  });
  fields.querySelectorAll<HTMLButtonElement>("[data-restore-session]").forEach((button) => {
    button.addEventListener("click", () => void restoreSession(button));
  });
  const unavailable = projects.filter((project) => archivedSessionErrors.has(project.id)).length;
  $("#settings-state").textContent = `${archivedSessions.length} archived Session${archivedSessions.length === 1 ? "" : "s"}.${unavailable ? ` ${unavailable} Project${unavailable === 1 ? " is" : "s are"} unavailable.` : ""}`;
}

async function loadArchivedSessions(projectId?: string): Promise<void> {
  const generation = archivedSessionsGeneration;
  const projects = (view?.projects ?? []).filter((project) => !projectId || project.id === projectId);
  const reads = projects.map((project) => ({
    project,
    version: advanceArchivedSessionRead(project.id),
  }));
  for (const { project } of reads) {
    archivedSessionLoadingProjects.add(project.id);
    archivedSessionErrors.delete(project.id);
  }
  if (settingsCategory === "archived_sessions") renderSettings();
  await Promise.all(reads.map(async ({ project, version }) => {
    const isCurrent = (): boolean => generation === archivedSessionsGeneration
      && archivedSessionReadVersions.get(project.id) === version;
    try {
      const sessions = await window.ait.projectSessions(project.id, "archived");
      if (!isCurrent()) return;
      archivedSessions = [
        ...archivedSessions.filter((session) => session.projectId !== project.id),
        ...sessions,
      ];
    } catch (error) {
      if (!isCurrent()) return;
      archivedSessionErrors.set(project.id, errorMessage(error));
    } finally {
      if (isCurrent()) {
        archivedSessionLoadingProjects.delete(project.id);
        if (settingsCategory === "archived_sessions" && !settingsDialog.classList.contains("is-hidden")) {
          renderSettings();
        }
      }
    }
  }));
  if (generation === archivedSessionsGeneration
    && settingsCategory === "archived_sessions"
    && !settingsDialog.classList.contains("is-hidden")) {
    renderSettings();
  }
}

function advanceArchivedSessionRead(projectId: string): number {
  const version = (archivedSessionReadVersions.get(projectId) ?? 0) + 1;
  archivedSessionReadVersions.set(projectId, version);
  return version;
}

function invalidateArchivedSessionRead(projectId: string): void {
  advanceArchivedSessionRead(projectId);
  archivedSessionLoadingProjects.delete(projectId);
}

async function restoreSession(button: HTMLButtonElement): Promise<void> {
  const projectId = button.dataset.restoreProject;
  const sessionId = button.dataset.restoreSession;
  if (!projectId || !sessionId) return;
  const mutation = projectViews.beginMutation(projectId);
  button.disabled = true;
  try {
    const updated = await window.ait.setSessionArchived({ projectId, sessionId, archived: false });
    invalidateArchivedSessionRead(projectId);
    archivedSessions = archivedSessions.filter((session) => session.id !== sessionId);
    sidebar.replace(projectId, updated.sessions);
    if (selectedProjectId === projectId) {
      if (!projectViews.commitMutation(mutation, updated) || !acceptLoadedProjectView()) return;
    } else projectViews.discardMutation(mutation);
    renderAll();
    renderSettings();
    showToast("Session restored.");
  } catch (error) {
    projectViews.discardMutation(mutation);
    button.disabled = false;
    showToast(errorMessage(error), true);
  }
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
  if (definition.kind.type === "agent_reference") {
    const agents = view?.agents.filter((agent) => agent.enabled && !agent.ownerSessionId) ?? [];
    return `<select ${common}><option value="">${definition.id === "agents.small_agent" ? "Use Default Agent" : "Not configured"}</option>${agents.map((agent) => `<option value="${escapeAttribute(agent.id)}"${value === agent.id ? " selected" : ""}>${escapeHtml(agentLabel(agent))}</option>`).join("")}</select>`;
  }
  if (definition.kind.type === "number") {
    return `<input ${common} type="number" min="${definition.kind.min}" max="${definition.kind.max}" value="${escapeAttribute(String(value ?? ""))}"/>`;
  }
  if (definition.kind.type === "path") {
    return `<button ${common} type="button" class="setting-path" aria-haspopup="dialog" title="${escapeAttribute(String(value ?? ""))}"><span>${escapeHtml(String(value ?? ""))}</span><span>Choose…</span></button>`;
  }
  const type = definition.kind.type === "credential_reference" ? "password" : "text";
  return `<input ${common} type="${type}" value="${escapeAttribute(String(value ?? ""))}" autocomplete="off"/>`;
}

async function chooseSettingPath(control: HTMLButtonElement): Promise<void> {
  const id = control.dataset.settingId;
  if (!id || selectingSettingPath) return;
  const draft = settingsDraft;
  selectingSettingPath = true;
  control.disabled = true;
  try {
    const path = await window.ait.chooseProjectDirectory(String(draft[id] ?? ""));
    // Closing, saving or resetting Settings discards this pending draft.
    if (draft !== settingsDraft || settingsDialog.classList.contains("is-hidden")) return;
    if (path) {
      draft[id] = path;
      renderSettings();
    }
  } catch (error) {
    showToast(errorMessage(error), true);
  } finally {
    selectingSettingPath = false;
    $("#settings-fields").querySelectorAll<HTMLButtonElement>("button[data-setting-id]").forEach((button) => {
      button.disabled = false;
      if (button.dataset.settingId === id && draft === settingsDraft && !settingsDialog.classList.contains("is-hidden")) button.focus();
    });
  }
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
    renderAll();
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
    renderAll();
    renderSettings();
    showToast("Core defaults restored.");
  } catch (error) {
    showToast(errorMessage(error), true);
  }
}

async function saveAgentRole(role: AgentRole, agentId: string): Promise<void> {
  if (!settings) throw new Error("Settings are not available.");
  const settingId = role === "default" ? "agents.default_agent" : "agents.small_agent";
  settings = await window.ait.saveSettings(settings.revision, { ...settings.values, [settingId]: agentId });
  settingsDraft = structuredClone(settings.values);
  renderAll();
}

async function changePermission(): Promise<void> {
  if (!settings || permissionSaving) return;
  const sandbox = composerPermission.value;
  permissionSaving = true;
  updateComposerState();
  try {
    settings = await window.ait.saveSettings(settings.revision, { ...settings.values, "permissions.sandbox": sandbox });
    settingsDraft = structuredClone(settings.values);
  } catch (error) {
    // A CAS conflict can mean another window changed the default. Refresh the
    // authoritative value before the user can submit another Run.
    try { settings = await window.ait.settings(); } catch { /* Keep last confirmed settings. */ }
    showToast(errorMessage(error), true);
  } finally {
    permissionSaving = false;
    applyPreferences();
    updateComposerState();
  }
}

function applyPreferences(): void {
  const sandbox = settings?.values["permissions.sandbox"];
  composerPermission.value = sandbox === "workspace_write" || sandbox === "full_access" ? sandbox : "read_only";
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
    { id: "runs", title: "Open Runs", hint: "" },
    { id: "crons", title: "Open Crons", hint: "" },
    { id: "tree", title: "Toggle Session Tree", hint: "" },
  ].filter((command) => command.title.toLowerCase().includes(query) && (command.id !== "tree" || activePage === "sessions"));
  $("#command-results").innerHTML = [
    ...sessions.map((session) => `<button class="command-result" type="button" data-session="${escapeAttribute(session.id)}"><span>⑂</span>${escapeHtml(session.title)}<small>Session</small></button>`),
    ...commands.map((command) => `<button class="command-result" type="button" data-command="${command.id}"><span>›</span>${command.title}<small>${command.hint}</small></button>`),
  ].join("") || '<div class="empty-details"><p>No matching command</p></div>';
  $("#command-results").querySelectorAll<HTMLElement>("[data-session]").forEach((button) => {
    button.addEventListener("click", async () => {
      if (currentPendingBranch()) return;
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
      if (button.dataset.command === "runs") showPage("runs");
      if (button.dataset.command === "crons") showPage("crons");
      if (button.dataset.command === "tree") toggleTree();
    });
  });
}

function renderFatal(error: unknown): void {
  $("#core-status").textContent = " Core unavailable";
  conversation.innerHTML = `<div class="empty-state"><h2>Could not start Ait daemon</h2><p>${escapeHtml(errorMessage(error))}</p><p>Run <code>pnpm run build:daemon</code> and reopen the app.</p></div>`;
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

setInterval(() => expireToolApprovalCards(document), 1_000);
