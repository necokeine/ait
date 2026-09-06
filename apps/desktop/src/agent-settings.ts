import type { AgentConfiguration, AgentProvider, DesktopSnapshot, ProviderModel } from "./types.js";

const escape = (value: string): string => value.replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
const option = (id: string, name: string, selected = ""): string => `<option value="${escape(id)}"${id === selected ? " selected" : ""}>${escape(name)}</option>`;
const field = (label: string, control: string): string => `<label class="catalog-field"><span>${label}</span>${control}</label>`;

let selectedProviderId: string | null = null;
let selectedPresetId = "";

export function providerChoices(providers: AgentProvider[]): AgentProvider[] {
  return providers.filter((provider) => ["codex", "openai", "deepseek"].includes(provider.kind));
}

export function renderAgentSettings(
  container: Element,
  snapshot: DesktopSnapshot,
  category: "models" | "agents",
  update: (snapshot: DesktopSnapshot) => void,
  notify: (message: string, failure?: boolean) => void,
): void {
  const panel = document.createElement("section");
  panel.className = "catalog-editor";
  container.prepend(panel);
  const input = (id: string): HTMLInputElement => panel.querySelector<HTMLInputElement>(`#${id}`)!;
  const select = (id: string): HTMLSelectElement => panel.querySelector<HTMLSelectElement>(`#${id}`)!;
  const providers = providerChoices(snapshot.providers);

  const perform = async (action: () => Promise<DesktopSnapshot>): Promise<void> => {
    const controls = Array.from(panel.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>("input, select, button"));
    const disabled = controls.map((control) => control.disabled);
    controls.forEach((control) => { control.disabled = true; });
    try { update(await action()); notify("Configuration saved."); }
    catch (error) {
      notify(error instanceof Error ? error.message : "Could not save configuration.", true);
      controls.forEach((control, index) => { control.disabled = disabled[index] ?? false; });
    }
  };

  if (category === "models") {
    const draw = (provider?: AgentProvider): void => {
      selectedProviderId = provider?.id ?? "";
      const remote = provider?.kind !== "codex";
      panel.innerHTML = `<h3>Agent providers</h3><p>Share a connection across Agent presets and Sessions. API keys are saved in your operating system credential store.</p>
        ${field("Connection", `<select id="provider-pick">${option("", "New provider")}${providers.map((p) => option(p.id, p.name, provider?.id)).join("")}</select>`)}
        ${field("Name", `<input id="provider-name" value="${escape(provider?.name ?? "")}" placeholder="My provider"/>`)}
        ${field("API", `<select id="provider-kind"${provider ? " disabled" : ""}>${option("openai", "OpenAI", provider?.kind ?? "openai")}${option("deepseek", "DeepSeek", provider?.kind)}${provider?.kind === "codex" ? option("codex", "Codex", "codex") : ""}</select>`)}
        ${remote ? field("API URL", `<input id="provider-url" value="${escape(provider?.url ?? "")}" placeholder="Leave blank for the official endpoint"/>`) + field("API key", `<input id="provider-secret" type="password" autocomplete="new-password" placeholder="${provider?.has_secret ? "Saved · leave blank to keep" : "Enter API key"}"/>`) : "<p>Codex uses the host's existing sign-in.</p>"}
        <h4>Models and reasoning levels</h4><p>Use the exact model ID and levels supported by this API. Leave reasoning levels empty when unsupported. Discovery preserves levels you have configured.</p>
        <div id="provider-models"></div><div class="catalog-actions"><button type="button" id="model-add">Add model</button><button type="button" id="provider-save">Save provider</button>${provider && remote ? '<button type="button" id="provider-refresh">Discover models</button>' : ""}</div>`;
      const rows = panel.querySelector("#provider-models")!;
      const addModel = (model: ProviderModel = { id: "", name: "", reasoning_efforts: [] }): void => {
        const row = document.createElement("div");
        row.className = "catalog-model";
        row.innerHTML = `${field("Model ID", `<input data-model-id value="${escape(model.id)}"/>`)}${field("Display name", `<input data-model-name value="${escape(model.name)}"/>`)}${field("Reasoning levels (comma separated)", `<input data-model-efforts value="${escape(model.reasoning_efforts.join(", "))}" placeholder="low, medium, high"/>`)}<button type="button" aria-label="Remove model">Remove</button>`;
        row.querySelector("button")!.addEventListener("click", () => row.remove());
        rows.append(row);
      };
      provider?.models.forEach(addModel);
      select("provider-pick").addEventListener("change", () => draw(providers.find((p) => p.id === select("provider-pick").value)));
      panel.querySelector("#model-add")!.addEventListener("click", () => addModel());
      panel.querySelector("#provider-save")!.addEventListener("click", () => {
        const models = Array.from(rows.querySelectorAll(".catalog-model")).map((row) => {
          const read = (selector: string): string => row.querySelector<HTMLInputElement>(selector)!.value.trim();
          return { id: read("[data-model-id]"), name: read("[data-model-name]"), reasoning_efforts: read("[data-model-efforts]").split(",").map((item) => item.trim()).filter(Boolean) };
        });
        const secret = remote ? input("provider-secret").value : "";
        const value = { id: provider?.id ?? crypto.randomUUID(), name: input("provider-name").value.trim(), kind: select("provider-kind").value, url: remote ? input("provider-url").value.trim() || null : null, models };
        selectedProviderId = value.id;
        if (remote) input("provider-secret").value = "";
        void perform(() => window.ait.saveProvider({ provider: value, ...(secret ? { secret } : {}) }));
      });
      panel.querySelector("#provider-refresh")?.addEventListener("click", () => void perform(() => window.ait.refreshProviderModels(provider!.id)));
    };
    draw(selectedProviderId === null ? providers[0] : providers.find((p) => p.id === selectedProviderId));
    return;
  }

  const presets = snapshot.agents.filter((agent) => !agent.ownerSessionId);
  const draw = (id = ""): void => {
    selectedPresetId = id;
    const agent = presets.find((item) => item.id === id);
    const initial = agent?.config;
    panel.innerHTML = `<h3>Named Agent presets</h3><p>Reuse these presets in Projects and Sessions. Session model and reasoning changes create a private configuration.</p>
      ${field("Preset", `<select id="preset-pick">${option("", "New preset")}${presets.map((item) => option(item.id, item.name, id)).join("")}</select>`)}
      ${field("Name", `<input id="preset-name" value="${escape(agent?.name ?? "")}" placeholder="Code review"/>`)}
      ${field("Provider", `<select id="preset-provider">${snapshot.providers.map((p) => option(p.id, p.name, initial?.provider_id)).join("")}</select>`)}
      ${field("Model", '<select id="preset-model"></select>')}
      ${field("Reasoning", '<select id="preset-effort"></select>')}
      <div class="catalog-actions"><button type="button" id="preset-save">Save preset</button></div>`;
    const efforts = (selected: string | null = null): void => {
      const model = snapshot.providers.find((p) => p.id === select("preset-provider").value)?.models.find((m) => m.id === select("preset-model").value);
      select("preset-effort").innerHTML = option("", "Provider default") + (model?.reasoning_efforts.map((effort) => option(effort, effort, selected ?? "")).join("") ?? "");
    };
    const models = (model = ""): void => {
      const provider = snapshot.providers.find((p) => p.id === select("preset-provider").value);
      select("preset-model").innerHTML = provider?.models.map((m) => option(m.id, m.name, model)).join("") ?? "";
      efforts();
    };
    models(initial?.model);
    efforts(initial?.reasoning_effort);
    select("preset-pick").addEventListener("change", () => draw(select("preset-pick").value));
    select("preset-provider").addEventListener("change", () => models());
    select("preset-model").addEventListener("change", () => efforts());
    panel.querySelector("#preset-save")!.addEventListener("click", () => {
      const config: AgentConfiguration = { provider_id: select("preset-provider").value, model: select("preset-model").value, reasoning_effort: select("preset-effort").value || null };
      const name = input("preset-name").value.trim();
      void perform(() => window.ait.saveAgent({ ...(id ? { id } : {}), name, config }));
    });
  };
  draw(selectedPresetId);
}
