import assert from "node:assert/strict";
import test from "node:test";
import { modelChoices, selectedModels } from "../src/provider-models.js";

const model = (id: string, reasoning_efforts: string[] = []) => ({ id, name: id, reasoning_efforts });

test("new discovery requires an explicit model selection and saves only that subset", () => {
  const choices = modelChoices([model("chat"), model("reasoner")], [], []);
  assert.deepEqual(selectedModels(choices), []);
  choices.find((choice) => choice.id === "reasoner")!.selected = true;
  assert.deepEqual(selectedModels(choices), [model("reasoner")]);
});

test("rediscovery preserves selected models and declared reasoning without enabling new models", () => {
  const saved = [model("chat", ["low", "high"])];
  const choices = modelChoices([model("chat"), model("new")], saved, []);
  assert.deepEqual(selectedModels(choices), saved);
  choices[0]!.reasoning_efforts.push("max");
  assert.deepEqual(saved[0]!.reasoning_efforts, ["low", "high"]);
});

test("a model in use remains selected even when absent from discovery", () => {
  const saved = [model("old", ["high"])];
  const choices = modelChoices([model("new")], saved, [{ provider_id: "p", model: "old", reasoning_effort: "high" }]);
  const old = choices.find((choice) => choice.id === "old")!;
  assert.equal(old.available, false);
  assert.equal(old.required, true);
  old.selected = false;
  assert.deepEqual(selectedModels(choices), saved);
});

test("returning to model selection preserves unsaved selections and reasoning levels", () => {
  const draft = modelChoices([model("chat"), model("reasoner")], [model("chat")], []);
  draft.find((choice) => choice.id === "chat")!.selected = false;
  const reasoner = draft.find((choice) => choice.id === "reasoner")!;
  reasoner.selected = true;
  reasoner.reasoning_efforts = ["high"];
  const reloaded = modelChoices([model("chat"), model("reasoner"), model("new")], [model("chat")], [], draft);
  assert.deepEqual(selectedModels(reloaded), [model("reasoner", ["high"])]);
});
