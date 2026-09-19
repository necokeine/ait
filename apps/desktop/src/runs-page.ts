import { escapeCatalog as escape } from "./agent-settings.js";
import { ActiveRunsMonitor, type ActiveRunsState } from "./active-runs-monitor.js";
import type { ActiveRunsCatalog, ActiveRunSummary, AgentSummary } from "./types.js";
import type { ProjectView } from "./types.js";
import { renderRunCommit } from "./run-commit.js";
import { renderToolApprovals } from "./tool-approval-ui.js";
import { interactionResponse, renderToolInteractions } from "./tool-interaction-ui.js";

interface RunsPageActions {
  read(): Promise<ActiveRunsCatalog>;
  retryCommit(input: { projectId: string; runId: string }): Promise<ProjectView>;
  project(id: string): Promise<ProjectView>;
  resolve(input: { projectId: string; runId: string; approvalId: string; action: "approve" | "deny" | "cancel" }): Promise<ProjectView>;
  resolveInteraction(input: { projectId: string; runId: string; interactionId: string; action: "submit" | "approve" | "deny" | "cancel"; response?: Record<string, string | string[]> }): Promise<ProjectView>;
  agents(): AgentSummary[];
  openSession(projectId: string, sessionId: string): Promise<void>;
  notify(message: string, failure?: boolean): void;
}

const labels: Record<string, string> = {
  queued: "Queued", running: "Running", waiting_approval: "Waiting for approval",
  retry_wait: "Waiting to retry", settling: "Finishing", cancelling: "Cancelling",
  acquiring_session_ref: "Preparing Session", assembling_context: "Preparing context",
  calling_agent: "Generating", persisting_message_and_advancing_session: "Saving message",
  executing_tool: "Running tool", persisting_tool_result: "Saving tool result",
  compacting_context: "Compacting context", checkpointing: "Saving checkpoint",
  recovering: "Recovering", draining_queue: "Processing queued input",
  releasing_session_ref: "Releasing Session", result_persisted: "Result saved",
  completed: "Completed", failed: "Failed", cancelled: "Cancelled",
  integrating: "Integrating changes", reconciling_result: "Reconciling result",
};

export function renderActiveRunRows(runs: ActiveRunSummary[], agents: AgentSummary[]): string {
  return runs.map((run) => {
    const agent = agents.find((candidate) => candidate.id === run.agentId);
    const agentName = agent?.ownerSessionId ? "Session Agent" : agent?.name || `Agent ${run.agentId.slice(0, 8)}`;
    const status = run.pendingApprovals > 0 ? "Waiting for approval" : labels[run.status] ?? run.status;
    const phase = run.phase && run.phase !== run.status ? labels[run.phase] ?? run.phase.replaceAll("_", " ") : "";
    return `<article class="run-row">
      <div class="run-location"><strong>${escape(run.sessionTitle ?? (run.trigger === "cron" ? "Scheduled Run" : "Run without Session"))}</strong><small>${escape(run.projectName)}</small><code title="${escape(run.id)}">Run ${escape(run.id)}</code></div>
      <div class="run-agent"><strong>${escape(agentName)}</strong><small>${escape(run.model)}</small><small>${run.trigger === "cron" ? "Scheduled" : "Manual"}</small></div>
      <div class="run-state"><span class="run-status${run.pendingApprovals > 0 || run.status === "waiting_approval" ? " needs-attention" : ""}"><i aria-hidden="true"></i>${escape(status)}</span>${phase ? `<small>${escape(phase)}</small>` : ""}</div>
      <div class="run-action"><button type="button" class="secondary-button" data-run-detail="${escape(run.id)}" data-run-project="${escape(run.projectId)}">View Run${run.pendingApprovals ? " · Approvals" : ""}</button>${run.sessionId ? `<button class="secondary-button" type="button" data-run-id="${escape(run.id)}" data-run-project="${escape(run.projectId)}" aria-label="Open Session ${escape(run.sessionTitle ?? run.sessionId)} in ${escape(run.projectName)}">Open Session <span aria-hidden="true">↗</span></button>` : '<span class="run-no-session">No Session</span>'}</div>
    </article>`;
  }).join("");
}

export function createRunsPage(container: Element, actions: RunsPageActions) {
  container.innerHTML = `<header class="agents-page-header"><div><span class="eyebrow">Workspace</span><h1 id="runs-page-title" tabindex="-1">Runs</h1><p>Active Runs across all Projects, including scheduled work.</p></div><button id="runs-refresh" class="secondary-button" type="button">Refresh</button></header>
    <div class="agents-page-scroll"><div class="runs-overview"><strong id="runs-count" aria-live="polite">Loading Runs…</strong><span id="runs-connection" class="runs-connection" role="status"></span></div><div id="runs-notice" class="runs-notice is-hidden" role="status"></div><div id="runs-list" class="runs-list" aria-label="Active Runs"></div><section id="run-detail" aria-live="polite"></section></div>`;
  const get = <T extends Element>(selector: string): T => container.querySelector<T>(selector)!;
  const list = get<HTMLElement>("#runs-list");
  const refreshButton = get<HTMLButtonElement>("#runs-refresh");

  let selected: { runId: string; projectId: string } | undefined;
  let detailGeneration = 0;
  let deciding = false;
  const detail = get<HTMLElement>("#run-detail");
  const refreshDetail = async (): Promise<void> => {
    if (!selected || deciding) return;
    const generation = ++detailGeneration;
    const target = selected;
    try {
      const project = await actions.project(target.projectId);
      if (generation !== detailGeneration || selected !== target) return;
      const run = project.runs.find((run) => run.id === target.runId);
      if (!run) { detail.textContent = "Run unavailable."; return; }
      const result = project.messages.find((message) => message.id === run.lastMessageId);
      const text = result?.parts.map((part) => part.type === "text" ? part.text : part.type === "tool_result" ? `Tool result: ${part.status}` : "").join("\n") ?? "";
      detail.innerHTML = `<header><h2>Run ${escape(run.id)}</h2><p class="run-detail-status">${escape(labels[run.status] ?? run.status)}</p></header>${renderRunCommit(run)}${renderToolApprovals(run)}${renderToolInteractions(run)}<pre class="run-detail-result">${escape(text)}</pre>`;
    } catch { if (generation === detailGeneration) detail.textContent = "Could not refresh this Run. Use Refresh to try again."; }
  };
  const render = (state: ActiveRunsState): void => {
    if (!state.loading) void refreshDetail();
    const { catalog, loading, connected, error } = state;
    const runs = catalog?.runs ?? [];
    get("#runs-count").textContent = catalog
      ? `${runs.length} active ${runs.length === 1 ? "Run" : "Runs"}${catalog.unavailableProjects.length ? " shown" : ""}`
      : loading ? "Loading Runs…" : "Runs unavailable";
    get("#runs-connection").textContent = !connected ? "Reconnecting to live updates…"
      : loading ? "Refreshing…" : error ? "Refresh failed" : "Live updates";
    refreshButton.disabled = loading;
    const notices = [
      ...(!connected ? ["Live updates are disconnected. Results are refreshed every 5 seconds."] : []),
      ...(error ? [error] : []),
      ...(catalog?.unavailableProjects.map((project) => `${project.projectName}: ${project.message}`) ?? []),
    ];
    const notice = get("#runs-notice");
    notice.classList.toggle("is-hidden", notices.length === 0);
    notice.innerHTML = notices.map((message) => `<p>${escape(message)}</p>`).join("");
    list.setAttribute("aria-busy", String(loading));
    const focused = document.activeElement instanceof HTMLButtonElement && list.contains(document.activeElement)
      ? { ...document.activeElement.dataset } : undefined;
    list.innerHTML = runs.length ? renderActiveRunRows(runs, actions.agents())
      : `<div class="runs-empty"><span aria-hidden="true">⌁</span><h2>${!catalog ? loading ? "Loading active Runs…" : "Could not load Runs" : catalog.unavailableProjects.length || error ? "Activity is unavailable" : "No active Runs"}</h2><p>${!catalog || catalog.unavailableProjects.length || error ? "Use Refresh to try again." : "Runs appear here when you send a message or scheduled work starts."}</p></div>`;
    if (focused) {
      Array.from(list.querySelectorAll<HTMLButtonElement>("[data-run-id], [data-run-detail]"))
        .find((button) => button.dataset.runId === focused.runId && button.dataset.runDetail === focused.runDetail && button.dataset.runProject === focused.runProject)?.focus();
    }
  };
  const monitor = new ActiveRunsMonitor(actions.read, render);
  refreshButton.addEventListener("click", () => void monitor.refresh());
  detail.addEventListener("click", (event) => {
    const commitButton = event.target instanceof Element ? event.target.closest<HTMLButtonElement>("[data-retry-commit]") : null;
    if (commitButton && selected && !deciding) {
      deciding = true;
      commitButton.disabled = true;
      void actions.retryCommit(selected)
        .catch((error: unknown) => actions.notify(error instanceof Error ? error.message : "Git commit retry failed.", true))
        .finally(() => { deciding = false; void refreshDetail(); void monitor.refresh(); });
      return;
    }
    const interactionButton = event.target instanceof Element ? event.target.closest<HTMLButtonElement>("[data-interaction-action]") : null;
    const interactionCard = interactionButton?.closest<HTMLElement>("[data-interaction-id]");
    const interactionAction = interactionButton?.dataset.interactionAction;
    if (interactionCard && selected && !deciding && ["submit", "approve", "deny", "cancel"].includes(interactionAction ?? "")) {
      let response: Record<string, string | string[]> | undefined;
      try {
        if (interactionAction === "submit") response = interactionResponse(interactionCard);
      } catch (error) {
        actions.notify(error instanceof Error ? error.message : "Answer is invalid.", true);
        return;
      }
      deciding = true;
      ++detailGeneration;
      detail.querySelectorAll<HTMLButtonElement>("button").forEach((button) => { button.disabled = true; });
      void actions.resolveInteraction({ ...selected, interactionId: interactionCard.dataset.interactionId!, action: interactionAction as "submit" | "approve" | "deny" | "cancel", ...(response ? { response } : {}) })
        .catch((error: unknown) => actions.notify(error instanceof Error ? error.message : "Response failed.", true))
        .finally(() => { deciding = false; void refreshDetail(); void monitor.refresh(); });
      return;
    }
    const button = event.target instanceof Element ? event.target.closest<HTMLButtonElement>("[data-approval-action]") : null;
    const card = button?.closest<HTMLElement>("[data-approval-id]");
    const action = button?.dataset.approvalAction;
    if (!card || !selected || deciding || !["approve", "deny", "cancel"].includes(action ?? "")) return;
    deciding = true;
    ++detailGeneration;
    detail.querySelectorAll<HTMLButtonElement>("button").forEach((button) => { button.disabled = true; });
    void actions.resolve({ ...selected, approvalId: card.dataset.approvalId!, action: action as "approve" | "deny" | "cancel" })
      .catch((error: unknown) => actions.notify(error instanceof Error ? error.message : "Approval failed.", true))
      .finally(() => { deciding = false; void refreshDetail(); void monitor.refresh(); });
  });
  list.addEventListener("click", (event) => {
    const detailButton = event.target instanceof Element ? event.target.closest<HTMLButtonElement>("[data-run-detail]") : null;
    if (detailButton) {
      selected = { runId: detailButton.dataset.runDetail!, projectId: detailButton.dataset.runProject! };
      detail.textContent = "Loading Run…";
      void refreshDetail();
      return;
    }
    const button = event.target instanceof Element ? event.target.closest<HTMLButtonElement>("[data-run-id]") : null;
    if (!button) return;
    const run = monitor.state.catalog?.runs.find((candidate) =>
      candidate.id === button.dataset.runId && candidate.projectId === button.dataset.runProject);
    if (!run?.sessionId) return;
    void actions.openSession(run.projectId, run.sessionId).catch((error: unknown) => {
      actions.notify(error instanceof Error ? error.message : "Could not open Session.", true);
    });
  });
  return monitor;
}
