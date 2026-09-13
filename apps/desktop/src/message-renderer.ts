import { messageAuthor } from "./messages.js";
import type { AgentSummary, DesktopMessage, RunProgress } from "./types.js";

export type TextBlock = { type: "text"; text: string } | { type: "code"; text: string; language: string };
export interface FileReference { path: string; line?: number; column?: number }

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

function fileReferenceButton(reference: FileReference, label: string): string {
  const position = reference.line ? ` at line ${reference.line}${reference.column ? `, column ${reference.column}` : ""}` : "";
  return `<button type="button" class="file-reference" data-file-path="${escapeHtml(reference.path)}"${reference.line ? ` data-file-line="${reference.line}"` : ""}${reference.column ? ` data-file-column="${reference.column}"` : ""} title="Open ${escapeHtml(reference.path)}${position}" aria-label="Open ${escapeHtml(reference.path)}${position}"><span aria-hidden="true">⌘</span>${escapeHtml(label)}</button>`;
}

export function parseFileReference(value: string): FileReference | undefined {
  let target = value.trim();
  if (target.startsWith("<") && target.endsWith(">")) target = target.slice(1, -1);
  if (target.startsWith("#")) return undefined;
  const windowsPath = /^[a-z]:[\\/]/i.test(target);
  if (/^https?:\/\//i.test(target) || /^[a-z][a-z\d+.-]*:/i.test(target) && !/^file:/i.test(target) && !windowsPath) {
    return undefined;
  }
  let line: number | undefined;
  let column: number | undefined;
  const fragment = /#L(\d+)(?:C(\d+))?(?:-L?\d+(?:C\d+)?)?$/i.exec(target);
  if (fragment) {
    line = Number(fragment[1]);
    column = fragment[2] ? Number(fragment[2]) : undefined;
    target = target.slice(0, fragment.index);
  } else {
    const suffix = /:(\d+)(?::(\d+))?$/.exec(target);
    if (suffix) {
      line = Number(suffix[1]);
      column = suffix[2] ? Number(suffix[2]) : undefined;
      target = target.slice(0, suffix.index);
    }
  }
  if (!target || target.includes("\0") || target.includes("\n")) return undefined;
  return {
    path: target,
    ...(line && Number.isSafeInteger(line) ? { line } : {}),
    ...(column && Number.isSafeInteger(column) ? { column } : {}),
  };
}

function looksLikeFileReference(value: string, reference: FileReference): boolean {
  return reference.line !== undefined
    || reference.path.startsWith("file:")
    || reference.path.includes("/")
    || reference.path.includes("\\")
    || /^\.?[\w@+~-]+\.[a-z\d]{1,12}$/i.test(value);
}

const automaticFileReference = /(?:\.{0,2}\/|\/)?(?:[\w@.+~-]+\/)+[\w@.+~-]+\.[a-z\d]{1,12}(?::\d+(?::\d+)?|#L\d+(?:C\d+)?)?/gi;

function renderPlainInline(text: string): string {
  let result = "";
  let cursor = 0;
  for (const match of text.matchAll(automaticFileReference)) {
    const value = match[0];
    const index = match.index;
    result += escapeHtml(text.slice(cursor, index));
    const reference = parseFileReference(value);
    result += reference && text[index - 1] !== ":" ? fileReferenceButton(reference, value) : escapeHtml(value);
    cursor = index + value.length;
  }
  return result + escapeHtml(text.slice(cursor));
}

function renderInlineMarkdown(text: string): string {
  let result = "";
  let cursor = 0;
  while (cursor < text.length) {
    const rest = text.slice(cursor);
    const link = /^\[([^\]\n]+)\]\(([^)\n]+)\)/.exec(rest);
    if (link) {
      const label = link[1]!;
      const target = link[2]!.trim();
      let reference = parseFileReference(target);
      const labelPosition = /(?:\(line\s+(\d+)(?:,?\s*column\s+(\d+))?\)|:(\d+)(?::(\d+))?)\s*$/i.exec(label);
      if (reference && reference.line === undefined && labelPosition) {
        const line = Number(labelPosition[1] ?? labelPosition[3]);
        const column = Number(labelPosition[2] ?? labelPosition[4]);
        reference = {
          ...reference,
          ...(Number.isSafeInteger(line) && line > 0 ? { line } : {}),
          ...(Number.isSafeInteger(column) && column > 0 ? { column } : {}),
        };
      }
      if (reference) {
        result += fileReferenceButton(reference, label);
      } else if (/^https:\/\//i.test(target)) {
        result += `<a class="external-link" href="${escapeHtml(target)}" target="_blank" rel="noreferrer">${escapeHtml(label)}</a>`;
      } else {
        result += escapeHtml(label);
      }
      cursor += link[0].length;
      continue;
    }
    const code = /^(`+)([^\n]*?)\1/.exec(rest);
    if (code) {
      const value = code[2]!;
      const reference = parseFileReference(value);
      result += reference && looksLikeFileReference(value, reference)
        ? fileReferenceButton(reference, value)
        : `<code>${escapeHtml(value)}</code>`;
      cursor += code[0].length;
      continue;
    }
    const strong = /^(\*\*|__)(.+?)\1/.exec(rest);
    if (strong) {
      result += `<strong>${renderPlainInline(strong[2]!)}</strong>`;
      cursor += strong[0].length;
      continue;
    }
    const emphasis = /^(\*|_)([^\s].*?)\1/.exec(rest);
    if (emphasis) {
      result += `<em>${renderPlainInline(emphasis[2]!)}</em>`;
      cursor += emphasis[0].length;
      continue;
    }
    // A lone underscore is common in source paths. Keep it in the plain-text
    // segment unless it begins emphasis at the current cursor.
    const next = rest.search(/[\[`*]/);
    if (next > 0) {
      result += renderPlainInline(rest.slice(0, next));
      cursor += next;
    } else {
      result += escapeHtml(rest[0]!);
      cursor += 1;
    }
  }
  return result;
}

function splitTableRow(line: string): string[] {
  let source = line.trim();
  if (source.startsWith("|")) source = source.slice(1);
  if (source.endsWith("|")) source = source.slice(0, -1);
  const cells: string[] = [];
  let cell = "";
  let escaped = false;
  let codeTicks = 0;
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index]!;
    if (escaped) {
      cell += character;
      escaped = false;
    } else if (character === "\\") {
      escaped = true;
    } else if (character === "`") {
      const start = index;
      while (source[index + 1] === "`") index += 1;
      const length = index - start + 1;
      codeTicks = codeTicks === length ? 0 : codeTicks === 0 ? length : codeTicks;
      cell += "`".repeat(length);
    } else if (character === "|" && codeTicks === 0) {
      cells.push(cell.trim());
      cell = "";
    } else {
      cell += character;
    }
  }
  cells.push(cell.trim());
  return cells;
}

function tableAlignments(line: string): Array<"left" | "center" | "right"> | undefined {
  const cells = splitTableRow(line);
  if (cells.length < 2 || cells.some((cell) => !/^:?-{3,}:?$/.test(cell))) return undefined;
  return cells.map((cell) => cell.startsWith(":") && cell.endsWith(":")
    ? "center"
    : cell.endsWith(":") ? "right" : "left");
}

function renderTable(lines: string[], start: number, alignments: Array<"left" | "center" | "right">): { html: string; next: number } {
  const headers = splitTableRow(lines[start]!);
  const rows: string[][] = [];
  let cursor = start + 2;
  while (cursor < lines.length && lines[cursor]!.includes("|") && lines[cursor]!.trim()) {
    rows.push(splitTableRow(lines[cursor]!));
    cursor += 1;
  }
  const cells = (row: string[], tag: "th" | "td") => headers.map((_, index) => {
    const alignment = alignments[index] ?? "left";
    return `<${tag} class="align-${alignment}">${renderInlineMarkdown(row[index] ?? "")}</${tag}>`;
  }).join("");
  return {
    html: `<div class="markdown-table-wrap" tabindex="0"><table><thead><tr>${cells(headers, "th")}</tr></thead><tbody>${rows.map((row) => `<tr>${cells(row, "td")}</tr>`).join("")}</tbody></table></div>`,
    next: cursor,
  };
}

function isList(line: string): boolean {
  return /^\s{0,3}(?:[-+*]|\d+[.)])\s+/.test(line);
}

function renderMarkdown(text: string): string {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const blocks: string[] = [];
  let cursor = 0;
  while (cursor < lines.length) {
    const line = lines[cursor]!;
    if (!line.trim()) {
      cursor += 1;
      continue;
    }
    const alignments = cursor + 1 < lines.length ? tableAlignments(lines[cursor + 1]!) : undefined;
    if (line.includes("|") && alignments) {
      const table = renderTable(lines, cursor, alignments);
      blocks.push(table.html);
      cursor = table.next;
      continue;
    }
    const heading = /^\s{0,3}(#{1,6})\s+(.+?)\s*#*\s*$/.exec(line);
    if (heading) {
      const level = heading[1]!.length;
      blocks.push(`<h${level}>${renderInlineMarkdown(heading[2]!)}</h${level}>`);
      cursor += 1;
      continue;
    }
    const unordered = /^\s{0,3}[-+*]\s+/.test(line);
    const ordered = /^\s{0,3}\d+[.)]\s+/.test(line);
    if (unordered || ordered) {
      const items: string[] = [];
      const pattern = unordered ? /^\s{0,3}[-+*]\s+(.+)$/ : /^\s{0,3}\d+[.)]\s+(.+)$/;
      while (cursor < lines.length) {
        const item = pattern.exec(lines[cursor]!);
        if (!item) break;
        items.push(`<li>${renderInlineMarkdown(item[1]!)}</li>`);
        cursor += 1;
      }
      const tag = unordered ? "ul" : "ol";
      blocks.push(`<${tag}>${items.join("")}</${tag}>`);
      continue;
    }
    if (/^\s{0,3}>\s?/.test(line)) {
      const quote: string[] = [];
      while (cursor < lines.length) {
        const quoted = /^\s{0,3}>\s?(.*)$/.exec(lines[cursor]!);
        if (!quoted) break;
        quote.push(quoted[1]!);
        cursor += 1;
      }
      blocks.push(`<blockquote>${quote.map(renderInlineMarkdown).join("<br>")}</blockquote>`);
      continue;
    }
    const paragraph = [line];
    cursor += 1;
    while (cursor < lines.length && lines[cursor]!.trim()) {
      if (/^\s{0,3}(?:#{1,6}\s+|>\s?|[-+*]\s+|\d+[.)]\s+)/.test(lines[cursor]!)) break;
      if (cursor + 1 < lines.length && lines[cursor]!.includes("|") && tableAlignments(lines[cursor + 1]!)) break;
      paragraph.push(lines[cursor]!);
      cursor += 1;
    }
    blocks.push(`<p>${paragraph.map(renderInlineMarkdown).join("<br>")}</p>`);
  }
  return blocks.join("");
}

const icon = (path: string) => `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${path}</svg>`;
const codeIcon = icon('<path d="m7 7-5 5 5 5m10-10 5 5-5 5m-4-12-2 14"/>');
const copyIcon = icon('<rect x="8" y="8" width="12" height="13" rx="3"/><path d="M16 8V6a3 3 0 0 0-3-3H6a3 3 0 0 0-3 3v7a3 3 0 0 0 3 3h2"/>');
const wrapIcon = icon('<path d="M3 6h18M3 11h14a4 4 0 0 1 0 8h-4m3-3-3 3 3 3M3 16h4"/>');

export function renderMessageText(text: string): string {
  return messageTextBlocks(text).map((block) => {
    if (block.type === "text") return `<div class="message-content markdown">${renderMarkdown(block.text)}</div>`;
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

type DisclosureKind = "Reasoning" | "Tool call" | "Tool result" | "Process";
interface MessageSection {
  key: string;
  kind: DisclosureKind | undefined;
  parts: DesktopMessage["parts"];
  message: DesktopMessage | undefined;
}

function disclosureKind(part: DesktopMessage["parts"][number]): DisclosureKind | undefined {
  if (part.type === "structured" && part.media_type === "application/vnd.ait.provider-reasoning+json") return "Reasoning";
  if (part.type === "tool_use") return "Tool call";
  if (part.type === "tool_result") return "Tool result";
  if (part.type === "operation") return part.kind === "reasoning" ? "Reasoning"
    : part.kind === "tool_result" ? "Tool result" : "Tool call";
  if (part.type === "codex_message" && part.phase !== "final_answer") return "Process";
  return undefined;
}

function messageSections(parts: DesktopMessage["parts"], key: string, message?: DesktopMessage, live = false): MessageSection[] {
  const sections: MessageSection[] = [];
  const fallbackFinalIndex = live || parts.some((part) => part.type === "codex_message" && part.phase === "final_answer")
    ? -1 : parts.findLastIndex((part) => part.type === "codex_message");
  parts.forEach((source, index) => {
    const part = source.type === "codex_message" && index === fallbackFinalIndex
      ? { ...source, phase: "final_answer" } : source;
    const kind = message?.role === "user" && message.kind !== "tool_result" ? undefined : disclosureKind(part);
    const previous = sections.at(-1);
    if (previous && previous.kind === kind) previous.parts.push(part);
    else sections.push({ key: `${key}:${index}`, kind, parts: [part], message });
  });
  return sections;
}

// Group presentation sections without changing the immutable Message path. In
// particular, adjacent ToolResult Messages still have separate ids and times.
function renderSections(sections: MessageSection[], agents: AgentSummary[], selectedId?: string): string {
  const groups: MessageSection[][] = [];
  for (const section of sections) {
    const previous = groups.at(-1);
    if (section.kind && previous?.[0]?.kind === section.kind) previous.push(section);
    else groups.push([section]);
  }
  return groups.map((group) => {
    const first = group[0]!;
    const content = group.map((section) => {
      const isInput = section.message?.role === "user" && section.message.kind !== "tool_result";
      const html = section.parts.map((part) => renderPart(part, isInput)).join("");
      return section.message ? renderMessageShell(section.message, agents, html, section.message.id === selectedId) : html;
    }).join("");
    return first.kind
      ? `<details class="message-disclosure" data-disclosure-id="${escapeHtml(first.key)}"><summary><span>${first.kind}</span><span class="operation-chevron" aria-hidden="true">⌄</span></summary><div class="message-disclosure-content">${content}</div></details>`
      : content;
  }).join("");
}

export function renderConversationMessages(messages: DesktopMessage[], agents: AgentSummary[], selectedId?: string): string {
  const firstVisible = messages.findIndex((message) => message.role !== "system");
  if (firstVisible < 0) return "";
  return renderSections(messages.slice(firstVisible).flatMap((message) => messageSections(message.parts, message.id, message)), agents, selectedId);
}

export function renderMessage(message: DesktopMessage, agents: AgentSummary[], selected = false): string {
  return renderSections(messageSections(message.parts, message.id, message), agents, selected ? message.id : undefined);
}

// Keep an explicitly opened disclosure open as streamed content is repainted.
export function replaceConversationContent(container: Element, html: string, preserveOpen: boolean): void {
  const openIds = new Set(preserveOpen
    ? Array.from(container.querySelectorAll<HTMLDetailsElement>("details[data-disclosure-id][open]"), (element) => element.dataset.disclosureId)
    : []);
  container.innerHTML = html;
  container.querySelectorAll<HTMLDetailsElement>("details[data-disclosure-id]").forEach((element) => {
    element.open = openIds.has(element.dataset.disclosureId);
  });
}

function renderMessageShell(message: DesktopMessage, agents: AgentSummary[], content: string, selected: boolean): string {
  const author = messageAuthor(message, agents);
  const isInput = message.role === "user" && message.kind !== "tool_result";
  return `<article class="message ${message.role}${isInput ? " user-input" : ""}${selected ? " is-selected" : ""}" data-message-id="${escapeHtml(message.id)}" tabindex="0" aria-current="${selected}" aria-label="${escapeHtml(author)} message">
    <div class="message-body"><div class="message-heading">${isInput ? "" : `<strong>${escapeHtml(author)}</strong>`}${renderMessageTime(message.createdAt)}</div>
      <div class="${isInput ? "user-input-bubble" : "message-parts"}">${content}</div>
    </div>
  </article>`;
}

export function renderRunProgress(progress: RunProgress | undefined, author: string, connected: boolean): string {
  const latestWarning = progress?.warnings.at(-1);
  const status = !connected
    ? "Connection interrupted — reconnecting without stopping the Run."
    : latestWarning?.retrying
      ? `Retrying — ${latestWarning.message}`
      : progress?.status === "completed" || progress?.status === "settling"
        ? `${author} finished; Ait is saving the result…`
        : `${author} is working…`;
  const content = progress?.items.length
    ? renderSections(messageSections(progress.items, `run:${progress.runId}`, undefined, true), [])
    : `<div class="live-run-placeholder"><span class="live-run-spinner" aria-hidden="true"></span>Waiting for ${escapeHtml(author)} output</div>`;
  return `<article class="message assistant live-run" data-run-id="${escapeHtml(progress?.runId ?? "")}" aria-live="polite">
    <div class="message-body"><div class="message-heading"><strong>${escapeHtml(author)}</strong><small class="live-run-status">${escapeHtml(status)}</small></div>
      <div class="message-parts">${content}</div>
    </div>
  </article>`;
}

export function renderRunTerminal(
  status: string,
  message: string | undefined,
  author: string,
): string {
  const cancelled = status === "cancelled";
  const heading = cancelled ? "Run cancelled" : status === "limit_exceeded" ? "Run limit reached" : "Run failed";
  const detail = message?.trim() || (cancelled
    ? "The Run was cancelled before a final answer was saved."
    : "The Run ended before a final answer was saved.");
  return `<article class="message assistant run-terminal status-${escapeHtml(status)}" aria-live="polite">
    <div class="message-body"><div class="message-heading"><strong>${escapeHtml(author)}</strong></div>
      <div class="run-terminal-card"><strong>${escapeHtml(heading)}</strong><p>${escapeHtml(detail)}</p></div>
    </div>
  </article>`;
}

function renderPart(part: DesktopMessage["parts"][number], isInput = false): string {
  if (part.type === "text") return isInput
    ? `<div class="message-content">${escapeHtml(part.text)}</div>`
    : renderMessageText(part.text);
  if (part.type === "codex_message") {
    const final = part.phase === "final_answer";
    return `<section class="${final ? "codex-final-answer" : "codex-output-message"}"${final ? " data-codex-final-answer" : ""} data-codex-item-id="${escapeHtml(part.id)}" data-codex-phase="${escapeHtml(part.phase)}">${renderMessageText(part.text)}</section>`;
  }
  if (part.type === "tool_use") return `<div class="tool-card"><header><strong>${escapeHtml(part.tool_name)}</strong></header><pre>${escapeHtml(prettyJson(part.arguments))}</pre></div>`;
  if (part.type === "tool_result") return renderOperation({
    type: "operation", id: part.call_id, kind: "tool_result", status: part.status,
    title: "Tool result", paths: [],
    detail: [part.error, part.output === null ? undefined : prettyJson(part.output)].filter((value) => value !== undefined && value !== null).join("\n") || "No output",
  });
  if (part.type === "operation") return renderOperation(part);
  if (part.type === "file") return `<div class="tool-card"><header><strong>${escapeHtml(part.name)}</strong><small>${escapeHtml(part.media_type)}</small></header></div>`;
  if (part.type === "structured") return `<div class="tool-card"><header><strong>${escapeHtml(part.media_type)}</strong></header><pre>${escapeHtml(prettyJson(part.value))}</pre></div>`;
  return '<div class="message-content">Content redacted</div>';
}

function renderOperation(part: Extract<DesktopMessage["parts"][number], { type: "operation" }>): string {
  const paths = part.paths.map((path) => {
    const reference = parseFileReference(path);
    return reference ? fileReferenceButton(reference, path) : escapeHtml(path);
  }).join("");
  const status = part.status.replace(/[^a-z\d_-]/gi, "-").toLowerCase();
  const summary = [part.summary ? `<span>${escapeHtml(part.summary)}</span>` : "", paths ? `<span class="operation-paths">${paths}</span>` : ""].filter(Boolean).join("");
  const heading = `<span class="operation-copy"><strong>${escapeHtml(part.title)}</strong>${summary ? `<small>${summary}</small>` : ""}</span><span class="operation-status status-${status}">${escapeHtml(part.status)}</span>`;
  return `<div class="operation-record"><header>${heading}</header>${part.detail ? `<pre>${escapeHtml(part.detail)}</pre>` : ""}</div>`;
}

function prettyJson(value: string): string {
  try { return JSON.stringify(JSON.parse(value), null, 2); }
  catch { return value; }
}

export function bindCodeBlockActions(
  container: Element,
  notify: (message: string, error?: boolean) => void,
  openFile?: (reference: FileReference) => Promise<{ positioned: boolean }>,
): void {
  container.addEventListener("click", (event) => {
    const file = (event.target as Element).closest<HTMLButtonElement>("button[data-file-path]");
    if (file && container.contains(file)) {
      event.stopPropagation();
      const line = Number(file.dataset.fileLine);
      const column = Number(file.dataset.fileColumn);
      if (!openFile) {
        notify("File opening is unavailable.", true);
        return;
      }
      void openFile({
        path: file.dataset.filePath ?? "",
        ...(Number.isSafeInteger(line) && line > 0 ? { line } : {}),
        ...(Number.isSafeInteger(column) && column > 0 ? { column } : {}),
      }).then(
        ({ positioned }) => notify(positioned ? "Opened file at the referenced line." : "Opened project path."),
        () => notify("Could not open this path inside the current Project.", true),
      );
      return;
    }
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
