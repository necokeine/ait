import { messageAuthor } from "./messages.js";
import type { AgentSummary, DesktopMessage } from "./types.js";

export type TextBlock = { type: "text"; text: string } | { type: "code"; text: string; language: string };

// Recognize fenced code without interpreting model output as HTML. An unfinished
// fence remains a code block, so the same renderer also works for partial output.
export function messageTextBlocks(text: string): TextBlock[] {
  const blocks: TextBlock[] = [];
  let prose = "";
  let fence: { marker: string; length: number; indent: number; language: string; text: string } | undefined;
  for (const line of text.match(/[^\n]*\n|[^\n]+$/g) ?? []) {
    if (fence) {
      const closing = /^ {0,3}(`+|~+)[ \t]*(?:\r?\n)?$/.exec(line);
      if (closing?.[1]?.[0] === fence.marker && closing[1].length >= fence.length) {
        blocks.push({ type: "code", text: fence.text, language: fence.language });
        fence = undefined;
      } else {
        fence.text += line.replace(new RegExp(`^ {0,${fence.indent}}`), "");
      }
      continue;
    }
    const opening = /^( {0,3})(`{3,}|~{3,})([^\r\n]*)(?:\r?\n)?$/.exec(line);
    if (opening && !(opening[2]!.startsWith("`") && opening[3]!.includes("`"))) {
      if (prose) blocks.push({ type: "text", text: prose });
      prose = "";
      fence = {
        marker: opening[2]![0]!, length: opening[2]!.length, indent: opening[1]!.length,
        language: opening[3]!.trim().split(/\s+/)[0] ?? "", text: "",
      };
    } else {
      prose += line;
    }
  }
  if (fence) blocks.push({ type: "code", text: fence.text, language: fence.language });
  if (prose) blocks.push({ type: "text", text: prose });
  return blocks;
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  })[character]!);
}

const icon = (path: string) => `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${path}</svg>`;
const codeIcon = icon('<path d="m7 7-5 5 5 5m10-10 5 5-5 5m-4-12-2 14"/>');
const copyIcon = icon('<rect x="8" y="8" width="12" height="13" rx="3"/><path d="M16 8V6a3 3 0 0 0-3-3H6a3 3 0 0 0-3 3v7a3 3 0 0 0 3 3h2"/>');
const wrapIcon = icon('<path d="M3 6h18M3 11h14a4 4 0 0 1 0 8h-4m3-3-3 3 3 3M3 16h4"/>');

export function renderMessageText(text: string): string {
  return messageTextBlocks(text).map((block) => {
    if (block.type === "text") return `<div class="message-content">${escapeHtml(block.text)}</div>`;
    const language = /^(text|txt|plaintext)$/i.test(block.language) || !block.language ? "Plain text" : block.language;
    return `<section class="code-block" aria-label="${escapeHtml(language)} code block">
      <header class="code-block-header"><span class="code-block-language">${codeIcon}<span>${escapeHtml(language)}</span></span>
        <div class="code-block-actions">
          <button type="button" data-code-action="wrap" aria-label="Wrap code" title="Wrap code" aria-pressed="false">${wrapIcon}</button>
          <button type="button" data-code-action="copy" aria-label="Copy code" title="Copy code">${copyIcon}</button>
        </div>
      </header>
      <pre tabindex="0" aria-label="${escapeHtml(language)} code"><code>${escapeHtml(block.text)}</code></pre>
    </section>`;
  }).join("");
}

export function renderMessageTime(timestamp: number): string {
  const date = new Date(timestamp);
  if (!Number.isFinite(timestamp) || timestamp <= 0 || !Number.isFinite(date.getTime())) {
    return '<span class="message-time" title="This message was saved without a timestamp">Time unavailable</span>';
  }
  const label = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "medium" }).format(date);
  return `<time class="message-time" datetime="${date.toISOString()}" title="${escapeHtml(date.toLocaleString())}">${escapeHtml(label)}</time>`;
}

export function renderMessage(message: DesktopMessage, agents: AgentSummary[], selected = false): string {
  const author = messageAuthor(message, agents);
  const isInput = message.role === "user" && message.kind !== "tool_result";
  const avatar = message.role === "assistant" ? author.slice(0, 2).toUpperCase() : message.role === "user" ? "U" : "S";
  const content = message.parts.map((part) => {
    if (part.type === "text") return isInput
      ? `<div class="message-content">${escapeHtml(part.text)}</div>`
      : renderMessageText(part.text);
    if (part.type === "tool_use") return `<div class="tool-card"><header><span>◇</span><strong>${escapeHtml(part.tool_name)}</strong><small>tool call</small></header><pre>${escapeHtml(prettyJson(part.arguments))}</pre></div>`;
    if (part.type === "file") return `<div class="tool-card"><header><span>＋</span><strong>${escapeHtml(part.name)}</strong><small>${escapeHtml(part.media_type)}</small></header></div>`;
    if (part.type === "structured") return `<div class="tool-card"><header><span>{ }</span><strong>${escapeHtml(part.media_type)}</strong></header><pre>${escapeHtml(part.value)}</pre></div>`;
    return '<div class="message-content">Content redacted</div>';
  }).join("");
  return `<article class="message ${message.role}${isInput ? " user-input" : ""}${selected ? " is-selected" : ""}" data-message-id="${escapeHtml(message.id)}" tabindex="0" aria-current="${selected}" aria-label="${escapeHtml(author)} message">
    ${isInput ? "" : `<div class="message-avatar" aria-hidden="true">${escapeHtml(avatar)}</div>`}
    <div class="message-body"><div class="message-heading">${isInput ? "" : `<strong>${escapeHtml(author)}</strong>`}${renderMessageTime(message.createdAt)}</div>
      <div class="${isInput ? "user-input-bubble" : "message-parts"}">${content}</div>
    </div>
  </article>`;
}

function prettyJson(value: string): string {
  try { return JSON.stringify(JSON.parse(value), null, 2); }
  catch { return value; }
}

export function bindCodeBlockActions(container: Element, notify: (message: string, error?: boolean) => void): void {
  container.addEventListener("click", (event) => {
    const button = (event.target as Element).closest<HTMLButtonElement>("button[data-code-action]");
    if (!button || !container.contains(button)) return;
    const block = button.closest<HTMLElement>(".code-block");
    if (!block) return;
    event.stopPropagation();
    if (button.dataset.codeAction === "wrap") {
      const wrapped = block.classList.toggle("is-wrapped");
      button.setAttribute("aria-pressed", String(wrapped));
    } else if (button.dataset.codeAction === "copy") {
      const text = block.querySelector("code")?.textContent ?? "";
      void navigator.clipboard.writeText(text).then(
        () => notify("Code copied"),
        () => notify("Could not copy code. Select the code and copy it manually.", true),
      );
    }
  });
}
