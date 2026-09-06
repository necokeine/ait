import { catalogOption as option, escapeCatalog as escape } from "./agent-settings.js";
import type { DesktopSnapshot } from "./types.js";

interface AgentsPageActions {
  update(snapshot: DesktopSnapshot): void;
  notify(message: string, failure?: boolean): void;
  configureProvider(id: string): void;
}

export function createAgentsPage(container: Element, actions: AgentsPageActions) {
  let snapshot: DesktopSnapshot | undefined;
  container.innerHTML = `<header class="agents-page-header"><div><span class="eyebrow">Workspace</span><h1 id="agents-page-title" tabindex="-1">Agents</h1><p>Connections and reusable configurations for your Projects and Sessions.</p></div><button id="agent-create" class="primary-button" type="button">New Agent</button></header>
    <div class="agents-page-scroll">
      <section class="agents-catalog-section" aria-labelledby="providers-title"><header class="catalog-heading"><div><h2 id="providers-title">Agent providers</h2><p>Shared connections and the models you have enabled.</p></div><button id="agents-add-provider" class="secondary-button" type="button">Add provider</button></header><div id="agents-provider-list" class="provider-cards"></div></section>
      <section class="agents-catalog-section" aria-labelledby="named-agents-title"><header class="catalog-heading"><div><h2 id="named-agents-title">Named Agent configurations</h2><p>Choose a preset in any Project or Session. Session-specific configurations stay in their Session.</p></div></header><div id="agent-editor" class="agent-editor is-hidden"></div><div id="named-agent-list" class="named-agent-list"></div></section>
    </div>`;
  const get = <T extends Element>(selector: string): T => container.querySelector<T>(selector)!;
  const editor = get<HTMLElement>("#agent-editor");
  let saving = false;

  const closeEditor = (): void => {
    if (saving) return;
    editor.classList.add("is-hidden");
    editor.replaceChildren();
  };

  const edit = (id?: string): void => {
    if (!snapshot || saving) return;
    const agent = snapshot.agents.find((item) => item.id === id && !item.ownerSessionId);
    const providers = snapshot.providers.filter((provider) => provider.models.length || provider.id === agent?.config.provider_id);
    const initial = agent?.config;
    editor.classList.remove("is-hidden");
    editor.innerHTML = `<form id="agent-config-form" aria-label="${agent ? "Edit Agent" : "New Agent"}"><header class="catalog-heading"><h3>${agent ? `Edit ${escape(agent.name)}` : "New Agent"}</h3><button class="small-icon-button" type="button" id="agent-editor-close" aria-label="Close Agent editor">×</button></header>
      <div class="agent-config-fields"><label class="catalog-field"><span>Name</span><input id="agent-config-name" value="${escape(agent?.name ?? "")}" placeholder="e.g. Code review" required autocomplete="off"/></label>
      <label class="catalog-field"><span>Provider</span><select id="agent-config-provider">${providers.map((provider) => option(provider.id, provider.name, initial?.provider_id)).join("")}</select></label>
      <label class="catalog-field"><span>Model</span><select id="agent-config-model" required></select></label>
      <label class="catalog-field"><span>Reasoning effort</span><select id="agent-config-effort"></select></label></div>
      <p class="catalog-help">${agent ? "Changes apply the next time this named Agent is used." : "Save a configuration to reuse across Projects and Sessions."}</p>
      <p id="agent-config-error" class="catalog-error${providers.length ? " is-hidden" : ""}" role="alert">${providers.length ? "" : "Add a provider and select its models first."}</p>
      <div class="catalog-actions"><button class="secondary-button" type="button" id="agent-config-cancel">Cancel</button><button class="primary-button" type="submit" id="agent-config-save"${providers.length ? "" : " disabled"}>Save Agent</button></div></form>`;
    const select = (name: string): HTMLSelectElement => get(`#agent-config-${name}`);
    const efforts = (selected = ""): void => {
      const model = snapshot?.providers.find((provider) => provider.id === select("provider").value)?.models.find((model) => model.id === select("model").value);
      select("effort").innerHTML = option("", "Provider default") + (model?.reasoning_efforts.map((effort) => option(effort, effort, selected)).join("") ?? "");
      select("effort").disabled = !model?.reasoning_efforts.length;
    };
    const models = (selected = ""): void => {
      const provider = snapshot?.providers.find((item) => item.id === select("provider").value);
      select("model").innerHTML = provider?.models.map((model) => option(model.id, model.name, selected)).join("") ?? "";
      efforts();
    };
    models(initial?.model);
    efforts(initial?.reasoning_effort ?? "");
    select("provider").addEventListener("change", () => models());
    select("model").addEventListener("change", () => efforts());
    get("#agent-editor-close").addEventListener("click", closeEditor);
    get("#agent-config-cancel").addEventListener("click", closeEditor);
    get<HTMLFormElement>("#agent-config-form").addEventListener("submit", (event) => {
      event.preventDefault();
      void save();
    });
    const save = async (): Promise<void> => {
      if (saving) return;
      const name = get<HTMLInputElement>("#agent-config-name").value.trim();
      const config = { provider_id: select("provider").value, model: select("model").value, reasoning_effort: select("effort").value || null };
      if (!name || !config.model) {
        get("#agent-config-error").textContent = !name ? "Enter an Agent name." : "Select a model.";
        get("#agent-config-error").classList.remove("is-hidden");
        return;
      }
      saving = true;
      const controls = Array.from(editor.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>("input, select, button"));
      const disabled = controls.map((control) => control.disabled);
      controls.forEach((control) => { control.disabled = true; });
      get("#agent-config-save").textContent = "Saving…";
      get("#agent-config-error").classList.add("is-hidden");
      try {
        const updated = await window.ait.saveAgent({ ...(agent ? { id: agent.id } : {}), name, config });
        saving = false;
        closeEditor();
        actions.update(updated);
        actions.notify("Named Agent saved.");
      } catch (failure) {
        const error = get("#agent-config-error");
        error.textContent = failure instanceof Error ? failure.message : "Could not save Agent.";
        error.classList.remove("is-hidden");
        controls.forEach((control, index) => { control.disabled = disabled[index] ?? false; });
        get("#agent-config-save").textContent = "Save Agent";
      } finally { saving = false; }
    };
    editor.scrollIntoView({ block: "nearest" });
    get<HTMLInputElement>("#agent-config-name").focus();
  };

  get("#agent-create").addEventListener("click", () => edit());
  get("#agents-add-provider").addEventListener("click", () => actions.configureProvider(""));

  const render = (updated: DesktopSnapshot): void => {
    snapshot = updated;
    get("#agents-provider-list").innerHTML = snapshot.providers.map((provider) => {
      const remote = ["openai", "deepseek"].includes(provider.kind);
      return `<article class="provider-card"><header><strong>${escape(provider.name)}</strong><span class="catalog-badge${remote && !provider.has_secret ? " needs-setup" : ""}">${remote ? provider.has_secret ? "Secret saved" : "Needs secret" : "Built-in"}</span></header>
        <p class="provider-endpoint">${escape(provider.url ?? (remote ? "Official API endpoint" : provider.kind === "codex" ? "Host sign-in" : "Built-in provider"))}</p>
        <details><summary>${provider.models.length} enabled models</summary><ul>${provider.models.map((model) => `<li><strong>${escape(model.name)}</strong><code>${escape(model.id)}</code><small>${model.reasoning_efforts.length ? escape(model.reasoning_efforts.join(" · ")) : "Default reasoning"}</small></li>`).join("") || "<li>No models selected.</li>"}</ul></details>
        <button class="secondary-button" type="button" data-configure-provider="${escape(provider.id)}" aria-label="Configure ${escape(provider.name)} provider">Configure</button></article>`;
    }).join("") || '<div class="catalog-empty">Add your first provider to choose its models.</div>';
    get("#named-agent-list").innerHTML = snapshot.agents.filter((agent) => !agent.ownerSessionId).map((agent) => {
      const provider = snapshot!.providers.find((provider) => provider.id === agent.config.provider_id);
      const sessions = snapshot!.sessions.filter((session) => session.agentId === agent.id).length;
      const projects = snapshot!.projects.filter((project) => project.defaultAgentId === agent.id).length;
      return `<article class="named-agent-row"><div class="named-agent-identity"><span class="named-agent-icon" aria-hidden="true">◇</span><div><strong>${escape(agent.name)}</strong><small>${escape(provider?.name ?? "Unavailable provider")}${agent.enabled ? "" : " · Disabled"}</small></div></div><div class="named-agent-model"><strong>${escape(agent.config.model)}</strong><small>${escape(agent.config.reasoning_effort ?? "Provider default")}</small></div><small class="named-agent-usage">${projects} Projects · ${sessions} Sessions</small><button class="secondary-button" type="button" data-edit-agent="${escape(agent.id)}" aria-label="Edit ${escape(agent.name)} Agent">Edit</button></article>`;
    }).join("") || '<div class="catalog-empty">No named Agents yet. Create one to reuse a model and reasoning configuration.</div>';
    container.querySelectorAll<HTMLElement>("[data-configure-provider]").forEach((button) => button.addEventListener("click", () => actions.configureProvider(button.dataset.configureProvider!)));
    container.querySelectorAll<HTMLElement>("[data-edit-agent]").forEach((button) => button.addEventListener("click", () => edit(button.dataset.editAgent!)));
  };
  return { render, closeEditor };
}
