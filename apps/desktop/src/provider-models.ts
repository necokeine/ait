import type { AgentConfiguration, ProviderModel } from "./types.js";

export interface ModelChoice extends ProviderModel {
  selected: boolean;
  available: boolean;
  required: boolean;
}

export function modelChoices(discovered: ProviderModel[], saved: ProviderModel[], configs: AgentConfiguration[], draft: ModelChoice[] = []): ModelChoice[] {
  const existing = new Map(saved.map((model) => [model.id, model]));
  const previous = new Map(draft.map((model) => [model.id, model]));
  const available = new Set(discovered.map((model) => model.id));
  const required = new Set(configs.map((config) => config.model));
  const models = new Map([...saved, ...discovered].map((model) => [model.id, model]));
  return Array.from(models.values(), (model) => ({
    ...model,
    reasoning_efforts: [...(previous.get(model.id)?.reasoning_efforts ?? existing.get(model.id)?.reasoning_efforts ?? model.reasoning_efforts)],
    selected: required.has(model.id) || (previous.get(model.id)?.selected ?? existing.has(model.id)),
    available: available.has(model.id),
    required: required.has(model.id),
  })).sort((left, right) => left.name.localeCompare(right.name));
}

export function selectedModels(choices: ModelChoice[]): ProviderModel[] {
  return choices.filter((choice) => choice.selected || choice.required)
    .map(({ id, name, reasoning_efforts }) => ({ id, name, reasoning_efforts: [...reasoning_efforts] }));
}
