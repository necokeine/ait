import { escapeCatalog as escape } from "./agent-settings.js";
import type { AgentProvider, AgentSummary, AitDesktopApi, CodexThreadSummary, DesktopProject } from "./types.js";

interface ImportContext { project: DesktopProject; agents: AgentSummary[]; providers: AgentProvider[] }

/** Explicit history discovery and import; never submits a model turn. */
export function createCodexImportDialog(
  dialog: HTMLElement,
  api: Pick<AitDesktopApi, "codexThreads" | "syncCodexThread">,
  changed: (projectId: string) => void,
) {
  let context: ImportContext;
  let providerId = "";
  let agentId = "";
  let threads: CodexThreadSummary[] = [];
  let selected = new Set<string>();
  let outcomes = new Map<string, { error?: string; done?: string }>();
  let generation = 0;
  let loading = false;
  let busy = false;
  let failure = "";
  let returnFocus: HTMLElement | null = null;
  const get = <T extends HTMLElement>(selector: string) => dialog.querySelector<T>(selector)!;
  const agents = () => context.agents.filter((agent) => agent.enabled && agent.config.provider_id === providerId && !agent.ownerSessionId);
  const rowAgent = (thread: CodexThreadSummary) => thread.sessionId ? thread.agentId : agentId;
  const unavailable = (thread: CodexThreadSummary): string => {
    if (thread.activeRunId) return "An Ait Run is active. Sync after it finishes.";
    const agent = context.agents.find((candidate) => candidate.id === rowAgent(thread));
    if (!agent?.enabled || agent.config.provider_id !== providerId) return thread.sessionId
      ? "The bound Agent is unavailable. Restore it before syncing."
      : "Choose an enabled Codex Agent to import.";
    return "";
  };

  function close(): void {
    if (dialog.classList.contains("is-hidden")) return;
    generation += 1;
    dialog.classList.add("is-hidden");
    returnFocus?.focus();
  }

  function renderRows(): void {
    const list = get("#codex-thread-list");
    list.setAttribute("aria-busy", String(loading));
    list.innerHTML = loading ? '<p class="field-help">Loading Codex sessions…</p>'
      : failure ? `<p role="alert">${escape(failure)}</p>`
      : !providerId ? '<p class="field-help">Configure a Codex provider in Agents to pull sessions.</p>'
      : !threads.length ? '<p class="field-help">No matching Codex sessions. Only sessions with an existing Project binding or an unambiguous working directory match appear here.</p>'
      : threads.map((thread) => {
        const result = outcomes.get(thread.threadId);
        const reason = unavailable(thread);
        const boundAgent = context.agents.find((agent) => agent.id === thread.agentId);
        const status = result?.done ?? (thread.sessionId ? `Imported · ${boundAgent?.name ?? "Unavailable Agent"}` : "New session");
        const date = new Date(thread.updatedAt);
        return `<label class="codex-thread-row" data-codex-thread="${escape(thread.threadId)}">
          <input type="checkbox" data-thread-select="${escape(thread.threadId)}" aria-label="Select ${escape(thread.title)}"${selected.has(thread.threadId) ? " checked" : ""}${busy || !!reason || !!result?.done ? " disabled" : ""}/>
          <span class="codex-thread-details"><strong>${escape(thread.title)}</strong>
            <span class="codex-thread-preview">${escape(thread.preview)}</span>
            <span class="codex-thread-path" title="${escape(thread.cwd)}">${escape(thread.cwd)}</span>
            <span class="codex-thread-meta">${escape(status)}${thread.archived ? " · Archived in Codex" : ""}${Number.isFinite(date.getTime()) ? ` · ${escape(date.toLocaleString())}` : ""}</span>
            ${reason ? `<span class="field-help">${escape(reason)}</span>` : ""}
            ${result?.error ? `<span class="codex-thread-error" role="alert">${escape(result.error)}</span>` : ""}
          </span></label>`;
      }).join("");
    list.querySelectorAll<HTMLInputElement>("[data-thread-select]").forEach((checkbox) => {
      checkbox.addEventListener("change", () => {
        if (checkbox.checked) selected.add(checkbox.dataset.threadSelect!);
        else selected.delete(checkbox.dataset.threadSelect!);
        updateActions();
      });
    });
    updateActions();
  }

  function selectable(): CodexThreadSummary[] {
    return threads.filter((thread) => !unavailable(thread) && !outcomes.get(thread.threadId)?.done);
  }

  function updateActions(): void {
    const candidates = selectable();
    const all = get<HTMLInputElement>("#codex-select-all");
    all.checked = candidates.length > 0 && candidates.every((thread) => selected.has(thread.threadId));
    all.indeterminate = selected.size > 0 && !all.checked;
    all.disabled = loading || busy || candidates.length === 0;
    get<HTMLButtonElement>("#codex-refresh").disabled = loading || busy || !providerId;
    get<HTMLSelectElement>("#codex-provider").disabled = busy || context.providers.length === 0;
    get<HTMLSelectElement>("#codex-agent").disabled = busy || agents().length === 0;
    const submit = get<HTMLButtonElement>("#codex-import-submit");
    submit.disabled = loading || busy || selected.size === 0;
    submit.textContent = busy ? "Importing / syncing…" : "Import / sync selected";
    const done = [...outcomes.values()].filter((result) => result.done).length;
    get("#codex-import-status").textContent = `${selected.size} selected${done ? ` · ${done} completed` : ""}`;
  }

  function renderAgentOptions(): void {
    const choices = agents();
    if (!choices.some((agent) => agent.id === agentId)) {
      agentId = choices.find((agent) => agent.id === context.project.defaultAgentId)?.id ?? choices[0]?.id ?? "";
    }
    const select = get<HTMLSelectElement>("#codex-agent");
    select.innerHTML = choices.map((agent) => `<option value="${escape(agent.id)}">${escape(agent.name)}</option>`).join("")
      || '<option value="">No enabled Codex Agent</option>';
    select.value = agentId;
  }

  async function load(): Promise<void> {
    const token = ++generation;
    threads = [];
    selected.clear();
    outcomes.clear();
    failure = "";
    loading = !!providerId;
    renderRows();
    if (!providerId) return;
    try {
      const result = await api.codexThreads({ projectId: context.project.id, providerId });
      if (token !== generation) return;
      threads = result;
    } catch (error) {
      if (token !== generation) return;
      failure = error instanceof Error ? error.message : String(error);
    } finally {
      if (token === generation) { loading = false; renderRows(); }
    }
  }

  async function submit(): Promise<void> {
    if (busy || loading || selected.size === 0) return;
    const token = generation;
    const projectId = context.project.id;
    const requests = selectable().filter((thread) => selected.has(thread.threadId)).map((thread) => ({
      projectId, providerId, threadId: thread.threadId, agentId: rowAgent(thread)!,
    }));
    busy = true;
    renderRows();
    for (const request of requests) {
      // Closing the dialog stops the remaining batch; an accepted import still refreshes its Project.
      if (token !== generation) break;
      try {
        await api.syncCodexThread(request);
        changed(projectId);
        if (token !== generation) break;
        const thread = threads.find((candidate) => candidate.threadId === request.threadId)!;
        outcomes.set(request.threadId, { done: thread.sessionId ? "Synced" : "Imported" });
        selected.delete(request.threadId);
      } catch (error) {
        if (token !== generation) break;
        outcomes.set(request.threadId, { error: error instanceof Error ? error.message : String(error) });
      }
      renderRows();
    }
    if (token === generation) { busy = false; renderRows(); }
  }

  function open(input: ImportContext): void {
    context = { ...input, providers: input.providers.filter((provider) => provider.kind === "codex") };
    const defaultAgent = context.agents.find((agent) => agent.id === context.project.defaultAgentId);
    providerId = context.providers.find((provider) => provider.id === defaultAgent?.config.provider_id)?.id ?? context.providers[0]?.id ?? "";
    agentId = "";
    busy = false;
    returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.innerHTML = `<section class="action-window codex-import-window">
      <header class="settings-header"><div><span class="eyebrow">${escape(context.project.name)}</span><h2 id="codex-import-title">Pull from Codex</h2></div>
        <button id="codex-import-close" class="icon-button" type="button" aria-label="Close Codex import">×</button></header>
      <div class="codex-import-options">
        <p class="field-help">Choose sessions to import or sync. Existing sessions keep their Agent. This reads history without starting a Run.</p>
        <div class="codex-import-selects"><label class="catalog-field"><span>Codex provider</span><select id="codex-provider">${context.providers.map((provider) => `<option value="${escape(provider.id)}">${escape(provider.name)}</option>`).join("")}</select></label>
          <label class="catalog-field"><span>Agent for new sessions</span><select id="codex-agent"></select></label></div>
        <div class="codex-import-toolbar"><label><input id="codex-select-all" type="checkbox"/> Select all</label><button id="codex-refresh" class="secondary-button" type="button">Refresh</button></div>
      </div>
      <div id="codex-thread-list" class="codex-thread-list"></div>
      <footer class="settings-footer"><span id="codex-import-status" role="status"></span><button id="codex-import-submit" class="primary-button" type="button">Import / sync selected</button></footer>
    </section>`;
    get<HTMLSelectElement>("#codex-provider").value = providerId;
    renderAgentOptions();
    get("#codex-import-close").addEventListener("click", close);
    get("#codex-refresh").addEventListener("click", () => void load());
    get("#codex-import-submit").addEventListener("click", () => void submit());
    get<HTMLSelectElement>("#codex-provider").addEventListener("change", (event) => {
      providerId = (event.target as HTMLSelectElement).value;
      renderAgentOptions();
      void load();
    });
    get<HTMLSelectElement>("#codex-agent").addEventListener("change", (event) => {
      agentId = (event.target as HTMLSelectElement).value;
      selected = new Set([...selected].filter((id) => threads.some((thread) => thread.threadId === id && !unavailable(thread))));
      renderRows();
    });
    get<HTMLInputElement>("#codex-select-all").addEventListener("change", (event) => {
      selected = new Set((event.target as HTMLInputElement).checked ? selectable().map((thread) => thread.threadId) : []);
      renderRows();
    });
    dialog.classList.remove("is-hidden");
    get("#codex-import-close").focus();
    void load();
  }

  dialog.addEventListener("click", (event) => { if (event.target === dialog) close(); });
  dialog.addEventListener("keydown", (event) => {
    if (event.key === "Escape") { event.stopPropagation(); close(); }
    if (event.key !== "Tab") return;
    const controls = [...dialog.querySelectorAll<HTMLElement>("button:not(:disabled), select:not(:disabled), input:not(:disabled)")];
    const first = controls[0], last = controls.at(-1);
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
  });
  return { open, close };
}
