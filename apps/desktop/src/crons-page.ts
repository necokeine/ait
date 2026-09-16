import { catalogOption as option, escapeCatalog as escape } from "./agent-settings.js";
import type {
  AgentSummary,
  CronRunSubmission,
  DesktopCron,
  DesktopProject,
  DesktopSession,
} from "./types.js";

interface CronPageContext {
  projects: DesktopProject[];
  agents: AgentSummary[];
  selectedProjectId: string | undefined;
  selectedSessionId: string | undefined;
}

interface CronPageActions {
  read(): Promise<DesktopCron[]>;
  sessions(projectId: string): Promise<DesktopSession[]>;
  create(input: {
    name: string;
    projectId: string;
    baseMessageId: string;
    agentId: string;
    schedule: string;
    timezone: string;
  }): Promise<DesktopCron>;
  setEnabled(cronId: string, enabled: boolean): Promise<DesktopCron>;
  trigger(cronId: string, scheduledAt: number): Promise<CronRunSubmission>;
  openRun(result: CronRunSubmission): Promise<void>;
  notify(message: string, failure?: boolean): void;
}

export function resolveCronBaseMessage(
  target: "session" | "message",
  messageId: string,
  session?: DesktopSession,
): string {
  if (target === "session") {
    if (!session) throw new Error("Choose a Session target.");
    return session.currentMessageId;
  }
  const value = messageId.trim();
  if (!value) throw new Error("Enter a Message ID.");
  return value;
}

export function renderCronRows(
  crons: DesktopCron[],
  projects: DesktopProject[],
  agents: AgentSummary[],
): string {
  return crons.map((cron) => {
    const project = projects.find((candidate) => candidate.id === cron.projectId);
    const agent = agents.find((candidate) => candidate.id === cron.agentId);
    return `<article class="cron-row">
      <div class="cron-identity"><strong>${escape(cron.name)}</strong><small>${escape(project?.name ?? `Project ${cron.projectId.slice(0, 8)}`)}</small></div>
      <div class="cron-schedule"><code>${escape(cron.schedule)}</code><small>${escape(cron.timezone)}</small></div>
      <div class="cron-target"><strong>${escape(agent?.name ?? `Agent ${cron.agentId.slice(0, 8)}`)}</strong><code title="${escape(cron.baseMessageId)}">Message ${escape(cron.baseMessageId.slice(0, 8))}</code></div>
      <span class="catalog-badge${cron.enabled ? "" : " needs-setup"}">${cron.enabled ? "Enabled" : "Disabled"}</span>
      <div class="cron-actions"><button class="secondary-button" type="button" data-cron-toggle="${escape(cron.id)}">${cron.enabled ? "Disable" : "Enable"}</button><button class="primary-button" type="button" data-cron-run="${escape(cron.id)}"${cron.enabled ? "" : " disabled"}>Run now</button></div>
    </article>`;
  }).join("");
}

export function createCronsPage(container: Element, actions: CronPageActions) {
  container.innerHTML = `<header class="agents-page-header"><div><span class="eyebrow">Workspace</span><h1 id="crons-page-title" tabindex="-1">Crons</h1><p>Run a fixed Message with a named Agent on a recurring schedule.</p></div><button id="cron-create" class="primary-button" type="button">New Cron</button></header>
    <div class="agents-page-scroll"><section id="cron-editor" class="agent-editor cron-editor is-hidden" aria-label="New Cron">
      <form id="cron-form"><header class="catalog-heading"><div><h2>New Cron</h2><p>Choose a Session head or enter an exact Message ID.</p></div><button id="cron-editor-close" class="small-icon-button" type="button" aria-label="Close Cron editor">×</button></header>
        <div class="cron-fields">
          <label class="catalog-field"><span>Name</span><input id="cron-name" required autocomplete="off" placeholder="Daily summary"/></label>
          <label class="catalog-field"><span>Project</span><select id="cron-project" required></select></label>
          <label class="catalog-field"><span>Target source</span><select id="cron-target-kind"><option value="session">Session head</option><option value="message">Message ID</option></select></label>
          <label id="cron-session-field" class="catalog-field"><span>Session</span><select id="cron-session" required></select></label>
          <label id="cron-message-field" class="catalog-field is-hidden"><span>Message ID</span><input id="cron-message" autocomplete="off" placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"/></label>
          <label class="catalog-field"><span>Agent</span><select id="cron-agent" required></select></label>
          <label class="catalog-field"><span>Schedule</span><input id="cron-schedule" required autocomplete="off" value="0 9 * * *"/></label>
          <label class="catalog-field"><span>Timezone</span><input id="cron-timezone" required autocomplete="off"/></label>
        </div>
        <p id="cron-error" class="catalog-error is-hidden" role="alert"></p>
        <div class="catalog-actions"><button id="cron-cancel" class="secondary-button" type="button">Cancel</button><button id="cron-save" class="primary-button" type="submit">Save Cron</button></div>
      </form>
    </section><div id="cron-list" class="cron-list" aria-live="polite"></div></div>`;

  const get = <T extends Element>(selector: string): T => container.querySelector<T>(selector)!;
  const editor = get<HTMLElement>("#cron-editor");
  const list = get<HTMLElement>("#cron-list");
  const projectSelect = get<HTMLSelectElement>("#cron-project");
  const sessionSelect = get<HTMLSelectElement>("#cron-session");
  const targetSelect = get<HTMLSelectElement>("#cron-target-kind");
  const agentSelect = get<HTMLSelectElement>("#cron-agent");
  let context: CronPageContext = {
    projects: [], agents: [], selectedProjectId: undefined, selectedSessionId: undefined,
  };
  let crons: DesktopCron[] = [];
  let sessions: DesktopSession[] = [];
  let active = false;
  let loaded = false;
  let busy = false;
  let sessionGeneration = 0;

  const error = (message = ""): void => {
    const element = get("#cron-error");
    element.textContent = message;
    element.classList.toggle("is-hidden", !message);
  };
  const setBusy = (value: boolean): void => {
    busy = value;
    container.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>("input, select, button")
      .forEach((control) => {
        if (value) {
          control.disabled = true;
          return;
        }
        const cron = control.dataset.cronRun
          ? crons.find((candidate) => candidate.id === control.dataset.cronRun)
          : undefined;
        control.disabled = (control.id === "cron-save" && !context.projects.length)
          || (control.id === "cron-session" && sessions.length === 0)
          || cron?.enabled === false;
      });
  };
  const renderList = (): void => {
    list.innerHTML = crons.length
      ? renderCronRows(crons, context.projects, context.agents)
      : '<div class="runs-empty"><span aria-hidden="true">◷</span><h2>No Crons</h2><p>Create a schedule from a Session head or an exact Message ID.</p></div>';
  };
  const load = async (): Promise<void> => {
    try {
      crons = await actions.read();
      loaded = true;
      renderList();
    } catch (failure) {
      actions.notify(failure instanceof Error ? failure.message : "Could not load Crons.", true);
    }
  };
  const replaceCron = (updated: DesktopCron): void => {
    const index = crons.findIndex((cron) => cron.id === updated.id);
    if (index < 0) crons.push(updated);
    else crons[index] = updated;
    renderList();
  };
  const updateTargetFields = (): void => {
    const sessionTarget = targetSelect.value === "session";
    get("#cron-session-field").classList.toggle("is-hidden", !sessionTarget);
    get("#cron-message-field").classList.toggle("is-hidden", sessionTarget);
    sessionSelect.required = sessionTarget;
    get<HTMLInputElement>("#cron-message").required = !sessionTarget;
  };
  const loadSessions = async (preferred?: string): Promise<void> => {
    const projectId = projectSelect.value;
    const generation = ++sessionGeneration;
    sessionSelect.disabled = true;
    sessionSelect.innerHTML = option("", "Loading Sessions…");
    try {
      const updated = projectId ? await actions.sessions(projectId) : [];
      if (generation !== sessionGeneration || projectSelect.value !== projectId) return;
      sessions = updated;
      sessionSelect.innerHTML = sessions.map((session) => option(session.id, session.title, preferred)).join("")
        || option("", "No Sessions available");
      sessionSelect.disabled = busy || sessions.length === 0;
      const session = sessions.find((candidate) => candidate.id === sessionSelect.value);
      if (session && context.agents.some((agent) => agent.id === session.agentId && !agent.ownerSessionId)) {
        agentSelect.value = session.agentId;
      }
    } catch (failure) {
      if (generation === sessionGeneration) {
        sessions = [];
        sessionSelect.innerHTML = option("", "Sessions unavailable");
        error(failure instanceof Error ? failure.message : "Could not load Sessions.");
      }
    } finally {
      if (generation === sessionGeneration) sessionSelect.disabled = busy || sessions.length === 0;
    }
  };
  const closeEditor = (): void => {
    if (busy) return;
    editor.classList.add("is-hidden");
    error();
  };
  const openEditor = (): void => {
    if (!context.projects.length || busy) {
      if (!context.projects.length) actions.notify("Create a Project before adding a Cron.", true);
      return;
    }
    const projectId = context.projects.some((project) => project.id === context.selectedProjectId)
      ? context.selectedProjectId! : context.projects[0]!.id;
    projectSelect.innerHTML = context.projects.map((project) => option(project.id, project.name, projectId)).join("");
    const namedAgents = context.agents.filter((agent) => agent.enabled && !agent.ownerSessionId);
    const project = context.projects.find((candidate) => candidate.id === projectId);
    agentSelect.innerHTML = namedAgents.map((agent) => option(agent.id, agent.name, project?.defaultAgentId ?? "")).join("");
    targetSelect.value = "session";
    get<HTMLInputElement>("#cron-message").value = "";
    get<HTMLInputElement>("#cron-timezone").value = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
    updateTargetFields();
    error();
    editor.classList.remove("is-hidden");
    void loadSessions(context.selectedProjectId === projectId ? context.selectedSessionId : undefined);
    get<HTMLInputElement>("#cron-name").focus();
  };

  get("#cron-create").addEventListener("click", openEditor);
  get("#cron-editor-close").addEventListener("click", closeEditor);
  get("#cron-cancel").addEventListener("click", closeEditor);
  projectSelect.addEventListener("change", () => void loadSessions());
  targetSelect.addEventListener("change", updateTargetFields);
  sessionSelect.addEventListener("change", () => {
    const session = sessions.find((candidate) => candidate.id === sessionSelect.value);
    if (session && context.agents.some((agent) => agent.id === session.agentId && !agent.ownerSessionId)) {
      agentSelect.value = session.agentId;
    }
  });
  get<HTMLFormElement>("#cron-form").addEventListener("submit", (event) => {
    event.preventDefault();
    if (busy) return;
    void (async () => {
      try {
        const session = sessions.find((candidate) => candidate.id === sessionSelect.value);
        const target = targetSelect.value === "message" ? "message" : "session";
        const baseMessageId = resolveCronBaseMessage(target, get<HTMLInputElement>("#cron-message").value, session);
        const input = {
          name: get<HTMLInputElement>("#cron-name").value.trim(),
          projectId: projectSelect.value,
          baseMessageId,
          agentId: agentSelect.value,
          schedule: get<HTMLInputElement>("#cron-schedule").value.trim(),
          timezone: get<HTMLInputElement>("#cron-timezone").value.trim(),
        };
        if (!input.name || !input.agentId || !input.schedule || !input.timezone) throw new Error("Complete every Cron field.");
        setBusy(true);
        get("#cron-save").textContent = "Saving…";
        replaceCron(await actions.create(input));
        setBusy(false);
        closeEditor();
        get<HTMLFormElement>("#cron-form").reset();
        actions.notify("Cron saved.");
      } catch (failure) {
        error(failure instanceof Error ? failure.message : "Could not save Cron.");
      } finally {
        setBusy(false);
        get("#cron-save").textContent = "Save Cron";
      }
    })();
  });
  list.addEventListener("click", (event) => {
    const button = event.target instanceof Element ? event.target.closest<HTMLButtonElement>("button") : null;
    if (!button || busy) return;
    const toggleId = button.dataset.cronToggle;
    const runId = button.dataset.cronRun;
    if (toggleId) {
      const cron = crons.find((candidate) => candidate.id === toggleId);
      if (!cron) return;
      setBusy(true);
      void actions.setEnabled(cron.id, !cron.enabled)
        .then((updated) => { replaceCron(updated); actions.notify(`Cron ${updated.enabled ? "enabled" : "disabled"}.`); })
        .catch((failure: unknown) => actions.notify(failure instanceof Error ? failure.message : "Could not update Cron.", true))
        .finally(() => setBusy(false));
    }
    if (runId) {
      setBusy(true);
      button.textContent = "Running…";
      void actions.trigger(runId, Date.now())
        .then(async (result) => {
          actions.notify("Scheduled Run completed in a new Session.");
          if (active) await actions.openRun(result);
        })
        .catch((failure: unknown) => actions.notify(failure instanceof Error ? failure.message : "Could not run Cron.", true))
        .finally(() => {
          button.textContent = "Run now";
          setBusy(false);
        });
    }
  });

  renderList();
  return {
    render(updated: CronPageContext): void {
      context = updated;
      renderList();
    },
    setActive(value: boolean): void {
      active = value;
      if (active && !loaded) void load();
    },
    refresh(): void {
      if (active) void load();
      else loaded = false;
    },
  };
}
