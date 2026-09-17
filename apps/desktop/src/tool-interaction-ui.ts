import { escapeCatalog as escape } from "./agent-settings.js";
import type { DesktopRun, ToolInteraction } from "./types.js";

interface Question {
  id: string;
  question: string;
  header?: string;
  multi_select?: boolean;
  options?: Array<{ label: string; description?: string }>;
}

function questions(interaction: ToolInteraction): Question[] {
  const value = interaction.request.questions;
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is Question => {
    if (!item || typeof item !== "object") return false;
    const question = item as Partial<Question>;
    return typeof question.id === "string" && typeof question.question === "string";
  });
}

export function renderToolInteractions(run: DesktopRun): string {
  return (run.toolInteractions ?? []).filter((interaction) => interaction.status === "pending").map((interaction) => {
    const live = interaction.expiresAt > Date.now() && ["running", "waiting_approval"].includes(run.status);
    if (interaction.toolName === "plan_exit") {
      const plan = typeof interaction.request.plan === "string" ? interaction.request.plan : "Plan unavailable.";
      return `<section class="native-approval tool-interaction" data-run-id="${escape(run.id)}" data-interaction-id="${escape(interaction.id)}">
        <header><strong>Review implementation plan</strong><span>${live ? "Response required" : "expired"}</span></header>
        <pre>${escape(plan)}</pre>
        ${live ? '<footer><button type="button" data-interaction-action="cancel">Cancel Run</button><button type="button" data-interaction-action="deny">Request changes</button><button type="button" class="primary" data-interaction-action="approve">Approve plan</button></footer>' : ""}
      </section>`;
    }
    const fields = questions(interaction).map((question, questionIndex) => {
      const options = Array.isArray(question.options) ? question.options : [];
      const control = options.length
        ? options.map((option, optionIndex) => `<label><input type="${question.multi_select ? "checkbox" : "radio"}" name="question-${questionIndex}" value="${escape(option.label)}"> <span><strong>${escape(option.label)}</strong>${option.description ? `<small>${escape(option.description)}</small>` : ""}</span></label>`).join("")
        : `<input type="text" name="question-${questionIndex}" maxlength="4096" autocomplete="off">`;
      return `<fieldset data-question-id="${escape(question.id)}" data-multi-select="${question.multi_select === true}"><legend>${escape(question.header ?? "Question")}</legend><p>${escape(question.question)}</p>${control}</fieldset>`;
    }).join("");
    return `<section class="native-approval tool-interaction" data-run-id="${escape(run.id)}" data-interaction-id="${escape(interaction.id)}">
      <header><strong>Agent question</strong><span>${live ? "Response required" : "expired"}</span></header>
      ${fields || "<p>Question unavailable.</p>"}
      ${live && fields ? '<footer><button type="button" data-interaction-action="cancel">Cancel Run</button><button type="button" class="primary" data-interaction-action="submit">Submit answer</button></footer>' : ""}
    </section>`;
  }).join("");
}

export function interactionResponse(card: HTMLElement): Record<string, string | string[]> {
  const response: Record<string, string | string[]> = {};
  for (const field of card.querySelectorAll<HTMLElement>("[data-question-id]")) {
    const id = field.dataset.questionId!;
    const inputs = Array.from(field.querySelectorAll<HTMLInputElement>("input"));
    if (field.dataset.multiSelect === "true") {
      const selected = inputs.filter((input) => input.checked).map((input) => input.value.trim()).filter(Boolean);
      if (!selected.length) throw new Error("Answer every question before submitting.");
      response[id] = selected;
    } else {
      const selected = inputs.find((input) => input.type === "text" || input.checked)?.value.trim();
      if (!selected) throw new Error("Answer every question before submitting.");
      response[id] = selected;
    }
  }
  if (!Object.keys(response).length) throw new Error("This question cannot be answered.");
  return response;
}
