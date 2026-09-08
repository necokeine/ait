import { modelChoices, selectedModels, type ModelChoice } from "./provider-models.js";
import type { AgentProvider, DesktopView, ProviderInput } from "./types.js";

export const escapeCatalog = (value: string): string => value.replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
export const catalogOption = (id: string, name: string, selected = ""): string => `<option value="${escapeCatalog(id)}"${id === selected ? " selected" : ""}>${escapeCatalog(name)}</option>`;
const field = (label: string, control: string): string => `<label class="catalog-field"><span>${label}</span>${control}</label>`;

export function providerChoices(providers: AgentProvider[]): AgentProvider[] {
  return providers.filter((provider) => ["codex", "openai", "deepseek"].includes(provider.kind));
}

export function renderProviderSettings(
  container: Element,
  view: DesktopView,
  update: (view: DesktopView, refreshSettings: boolean) => void,
  notify: (message: string, failure?: boolean) => void,
  initialProviderId?: string,
): () => void {
  const panel = document.createElement("section");
  panel.className = "catalog-editor";
  container.prepend(panel);
  const get = <T extends Element>(selector: string): T => panel.querySelector<T>(selector)!;
  let generation = 0;
  let disposed = false;
  let secret = "";
  const scrollToTop = (): void => {
    const scroll = panel.closest(".settings-form");
    if (scroll) scroll.scrollTop = 0;
  };

  const overview = (): void => {
    generation++;
    secret = "";
    panel.innerHTML = `<header class="catalog-heading"><div><h3>Agent providers</h3><p>Connect an API, then choose the models available to your Agents.</p></div><button id="provider-add" class="primary-button" type="button">Add provider</button></header>
      <div class="provider-settings-list">${view.providers.map((provider) => `<button class="provider-settings-item" type="button" data-provider="${escapeCatalog(provider.id)}"><span><strong>${escapeCatalog(provider.name)}</strong><small>${escapeCatalog(provider.url ?? (provider.kind === "codex" ? "Host sign-in" : provider.kind))}</small></span><span>${provider.models.length} models <span aria-hidden="true">›</span></span></button>`).join("") || '<p>No providers yet. Add a connection to get started.</p>'}</div>`;
    get("#provider-add").addEventListener("click", () => edit());
    scrollToTop();
    panel.querySelectorAll<HTMLElement>("[data-provider]").forEach((button) => {
      button.addEventListener("click", () => edit(view.providers.find((provider) => provider.id === button.dataset.provider)));
    });
  };

  const edit = (existing?: AgentProvider): void => {
    const version = ++generation;
    secret = "";
    let provider = {
      id: existing?.id ?? crypto.randomUUID(), name: existing?.name ?? "",
      kind: existing?.kind ?? "openai", url: existing?.url ?? null,
      models: existing?.models ?? [],
    };
    const remote = !existing || ["openai", "deepseek"].includes(existing.kind);
    let choices: ModelChoice[] = [];
    let query = "";
    const live = (): boolean => !disposed && generation === version;
    const request = (): ProviderInput => ({ provider, ...(secret ? { secret } : {}) });
    const error = (message: string): void => {
      const element = get<HTMLElement>("#provider-error");
      element.textContent = message;
      element.classList.remove("is-hidden");
    };
    const steps = (step: number): string => `<div class="provider-steps" aria-label="Provider setup steps"><span${step === 1 ? ' aria-current="step"' : ""}>1 <strong>Connection</strong></span><span aria-hidden="true">›</span><span${step === 2 ? ' aria-current="step"' : ""}>2 <strong>Choose models</strong></span></div>`;

    const connection = (): void => {
      panel.innerHTML = `<button id="provider-back-list" class="catalog-back" type="button">‹ All providers</button><h3>${existing ? "Configure provider" : "Add provider"}</h3>${steps(1)}
        <form id="provider-connection">
          ${field("Name", `<input id="provider-name" value="${escapeCatalog(provider.name)}" required placeholder="My provider" autocomplete="off"/>`)}
          ${field("API", `<select id="provider-kind"${existing ? " disabled" : ""}>${remote ? catalogOption("openai", "OpenAI", provider.kind) + catalogOption("deepseek", "DeepSeek", provider.kind) : catalogOption(provider.kind, existing!.name, provider.kind)}</select>`)}
          ${remote ? field("API URL", `<input id="provider-url" type="url" value="${escapeCatalog(provider.url ?? "")}" placeholder="Leave blank for the official endpoint" autocomplete="url"/>`)
            + field("Secret", `<input id="provider-secret" type="password" autocomplete="new-password"${existing?.has_secret ? "" : " required"} placeholder="${existing?.has_secret ? "Saved · leave blank to keep" : "Enter API key"}"/>`)
            + '<p class="catalog-help">The next step connects to this API and loads its model list. The connection is saved after you choose models.</p>'
            : `<p>${provider.kind === "codex" ? "Codex uses your existing sign-in on this machine." : "This built-in provider uses its configured model catalog."}</p>`}
          <p id="provider-error" class="catalog-error is-hidden" role="alert"></p>
          <div class="catalog-actions"><button class="secondary-button" id="provider-cancel" type="button">Cancel</button><button class="primary-button" id="provider-next" type="submit">Next: choose models</button></div>
        </form>`;
      get("#provider-back-list").addEventListener("click", overview);
      get("#provider-cancel").addEventListener("click", overview);
      if (remote) get<HTMLInputElement>("#provider-secret").value = secret;
      get<HTMLFormElement>("#provider-connection").addEventListener("submit", (event) => {
        event.preventDefault();
        const next = { ...provider, name: get<HTMLInputElement>("#provider-name").value.trim(), kind: get<HTMLSelectElement>("#provider-kind").value, url: remote ? get<HTMLInputElement>("#provider-url").value.trim() || null : null };
        const nextSecret = remote ? get<HTMLInputElement>("#provider-secret").value.trim() : "";
        if (next.kind !== provider.kind || next.url !== provider.url || nextSecret !== secret) choices = [];
        provider = next;
        if (!provider.name) { error("Enter a provider name."); return; }
        secret = nextSecret;
        void discover();
      });
      scrollToTop();
      get<HTMLInputElement>("#provider-name").focus({ preventScroll: true });
    };

    const discover = async (): Promise<void> => {
      const controls = Array.from(panel.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>("#provider-connection input, #provider-connection select, #provider-next"));
      const disabled = controls.map((control) => control.disabled);
      controls.forEach((control) => { control.disabled = true; });
      get("#provider-next").textContent = "Loading models…";
      get("#provider-error").classList.add("is-hidden");
      try {
        const discovered = remote || provider.kind === "codex"
          ? await window.ait.discoverProviderModels(request())
          : provider.models;
        if (!live()) return;
        choices = modelChoices(discovered, existing?.models ?? [], view.agents.filter((agent) => agent.config.provider_id === provider.id).map((agent) => agent.config), choices);
        selection(discovered.length);
      } catch (failure) {
        if (!live()) return;
        error(`${failure instanceof Error ? failure.message : "Could not load models."} Check the API URL and secret, then try again.`);
        controls.forEach((control, index) => { control.disabled = disabled[index] ?? false; });
        get("#provider-next").textContent = "Retry: load models";
      }
    };

    const selection = (discoveredCount: number): void => {
      panel.innerHTML = `<h3>${escapeCatalog(provider.name)}</h3>${steps(2)}<p>${discoveredCount ? `${discoveredCount} models found. Choose which ones to make available to Agents.` : "This API returned no models. Go back to check the connection or try again."}</p>
        <div class="model-selection-toolbar"><input id="model-search" type="search" aria-label="Search models" placeholder="Search models…"/><button id="models-all" class="secondary-button" type="button">Select all</button><button id="models-clear" class="secondary-button" type="button">Clear</button></div>
        <div id="provider-model-choices" class="model-choices" role="group" aria-label="Available models"></div>
        <p id="model-selection-count" class="catalog-help" aria-live="polite"></p>
        <p id="provider-error" class="catalog-error is-hidden" role="alert"></p>
        <div class="catalog-actions"><button id="provider-back" class="secondary-button" type="button">Back</button><button id="provider-save" class="primary-button" type="button">Save provider</button></div>`;
      const status = (): void => {
        const count = selectedModels(choices).length;
        get("#model-selection-count").textContent = `${count} selected · Only selected models appear in Agent configuration.`;
        get<HTMLButtonElement>("#provider-save").disabled = count === 0;
      };
      const list = (): void => {
        const visible = choices.filter((choice) => `${choice.id} ${choice.name}`.toLowerCase().includes(query));
        get("#provider-model-choices").innerHTML = visible.map((choice) => `<div class="model-choice" data-model="${escapeCatalog(choice.id)}">
          <label><input type="checkbox"${choice.selected ? " checked" : ""}${choice.required ? " disabled" : ""}/><span><strong>${escapeCatalog(choice.name)}</strong><small>${escapeCatalog(choice.id)}</small></span></label>
          ${choice.required ? '<small class="model-note">Used by an Agent</small>' : ""}${!choice.available ? '<small class="model-note">Saved model · not returned by this API</small>' : ""}
          <details><summary>Reasoning levels${choice.reasoning_efforts.length ? ` · ${escapeCatalog(choice.reasoning_efforts.join(", "))}` : " (optional)"}</summary>${field(`Levels for ${escapeCatalog(choice.name)}`, `<input class="model-efforts" value="${escapeCatalog(choice.reasoning_efforts.join(", "))}" placeholder="e.g. low, medium, high"/>`)}<p class="catalog-help">Use levels supported by this model. Leave empty when unavailable.</p></details>
        </div>`).join("") || '<p class="catalog-empty">No matching models.</p>';
        get("#provider-model-choices").querySelectorAll<HTMLElement>("[data-model]").forEach((row) => {
          const choice = choices.find((model) => model.id === row.dataset.model)!;
          row.querySelector<HTMLInputElement>('input[type="checkbox"]')!.addEventListener("change", (event) => { choice.selected = (event.target as HTMLInputElement).checked; status(); });
          row.querySelector<HTMLInputElement>(".model-efforts")!.addEventListener("input", (event) => {
            choice.reasoning_efforts = [...new Set((event.target as HTMLInputElement).value.split(",").map((value) => value.trim()).filter(Boolean))];
          });
        });
        status();
      };
      get<HTMLInputElement>("#model-search").value = query;
      get("#model-search").addEventListener("input", (event) => { query = (event.target as HTMLInputElement).value.toLowerCase(); list(); });
      get("#models-all").addEventListener("click", () => { choices.forEach((choice) => { choice.selected = true; }); list(); });
      get("#models-clear").addEventListener("click", () => { choices.forEach((choice) => { choice.selected = choice.required; }); list(); });
      get("#provider-back").addEventListener("click", connection);
      get("#provider-save").addEventListener("click", () => void save());
      list();
      scrollToTop();
      get<HTMLInputElement>("#model-search").focus({ preventScroll: true });
    };

    const save = async (): Promise<void> => {
      const controls = Array.from(panel.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button"));
      const disabled = controls.map((control) => control.disabled);
      controls.forEach((control) => { control.disabled = true; });
      get("#provider-save").textContent = "Saving…";
      provider = { ...provider, models: selectedModels(choices) };
      try {
        const updated = await window.ait.saveProvider(request());
        secret = "";
        update(updated, live());
        notify("Provider and selected models saved.");
      } catch (failure) {
        if (!live()) return;
        error(failure instanceof Error ? failure.message : "Could not save provider.");
        controls.forEach((control, index) => { control.disabled = disabled[index] ?? false; });
        get("#provider-save").textContent = "Save provider";
      }
    };
    connection();
  };

  if (initialProviderId !== undefined) edit(view.providers.find((provider) => provider.id === initialProviderId));
  else overview();
  return () => { disposed = true; generation++; secret = ""; panel.remove(); };
}
