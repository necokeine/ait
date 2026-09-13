import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { messageTextBlocks, parseFileReference, renderConversationMessages, renderMessage, renderMessageText, renderMessageTime } from "../src/message-renderer.js";
import { projectMessage, type WorkspaceMessage } from "../src/messages.js";
import { messageText } from "../src/tree.js";
import type { DesktopMessage, MessagePart } from "../src/types.js";

const reasoning = (text: string): MessagePart => ({
  type: "structured", media_type: "application/vnd.ait.provider-reasoning+json", value: JSON.stringify({ reasoning: text }),
});
const call = (id: string): MessagePart => ({ type: "tool_use", call_id: id, tool_name: id, arguments: '{"path":"src/main.rs"}' });
const result = (id: string): MessagePart => ({ type: "tool_result", call_id: id, status: "succeeded", output: `output-${id}`, error: null });
const transcriptMessage = (id: string, parts: MessagePart[], role: DesktopMessage["role"] = "assistant"): DesktopMessage => ({
  id, parentMessageId: null, projectId: "project", role,
  kind: parts[0]?.type === "tool_result" ? "tool_result" : "standard", parts, createdAt: 1_800_000_000_000,
});
const disclosureTitles = (html: string): string[] => Array.from(html.matchAll(/<summary><span>([^<]+)<\/span>/g), (match) => match[1]!);

test("hides only leading system messages without changing history or later system notices", () => {
  const messages = [
    transcriptMessage("root", [{ type: "text", text: "Initial prompt" }], "system"),
    transcriptMessage("context", [{ type: "text", text: "Initial context" }], "system"),
    transcriptMessage("input", [{ type: "text", text: "Hello" }], "user"),
    transcriptMessage("notice", [{ type: "text", text: "Later notice" }], "system"),
  ];
  const snapshot = structuredClone(messages);
  const html = renderConversationMessages(messages, []);
  assert.ok(!html.includes("Initial prompt"));
  assert.ok(!html.includes("Initial context"));
  assert.ok(html.includes("Hello"));
  assert.ok(html.includes("Later notice"));
  assert.deepEqual(messages, snapshot);
  assert.equal(renderConversationMessages(messages.slice(0, 2), []), "");
  assert.equal(renderConversationMessages([], []), "");
});

test("combines consecutive activity types into Events across message boundaries in order", () => {
  const messages = [
    transcriptMessage("thinking-a", [reasoning("Thought A"), reasoning("Thought B")]),
    transcriptMessage("thinking-b", [reasoning("Thought C"), { type: "text", text: "Inspecting files" }, call("read-a"), call("read-b")]),
    transcriptMessage("calls", [call("read-c")]),
    transcriptMessage("result-a", [result("read-a")], "user"),
    transcriptMessage("result-b", [result("read-b")], "user"),
    transcriptMessage("answer", [{ type: "text", text: "Done" }, reasoning("Follow-up"), call("read-d")]),
  ];
  const snapshot = structuredClone(messages);
  const html = renderConversationMessages(messages, [], "result-b");
  assert.deepEqual(disclosureTitles(html), ["Events", "Events", "Events"]);
  const groups = Array.from(html.matchAll(/<details\b[^>]*>(.*?)<\/details>/gs), (match) => match[1]!);
  assert.deepEqual(groups.map((group) => Number(/class="message-event-count">(\d+)/.exec(group)?.[1])), [3, 5, 2]);
  assert.ok(groups.every((group) => !group.includes("Inspecting files") && !group.includes("Done")));
  assert.ok(groups[1]!.includes("read-c") && groups[1]!.includes("output-read-b"));
  assert.ok(groups[2]!.includes("Follow-up") && groups[2]!.includes("read-d"));
  for (const kind of ["Reasoning", "Tool call", "Tool result"]) assert.ok(html.includes(`class="message-event-kind">${kind}</div>`));
  assert.ok(!/<details[^>]*\sopen[\s>]/.test(html));
  assert.ok(!html.includes("message-avatar"));
  for (const message of messages) assert.ok(html.includes(`data-message-id="${message.id}"`));
  assert.match(html, /is-selected[^>]*data-message-id="result-b"/);
  assert.ok(html.includes('datetime="2027-01-15T08:00:00.000Z"'));
  const ordered = ["Thought A", "Thought B", "Thought C", "Inspecting files", "read-a", "read-b", "read-c", "output-read-a", "output-read-b", "Done", "Follow-up", "read-d"];
  for (let index = 1; index < ordered.length; index += 1) assert.ok(html.indexOf(ordered[index - 1]!) < html.indexOf(ordered[index]!));
  assert.deepEqual(messages, snapshot);
});

test("keeps separated groups separate and leaves unrelated structured content visible", () => {
  const html = renderMessage(transcriptMessage("mixed", [
    call("first"), { type: "text", text: "Between calls" }, call("second"),
    { type: "structured", media_type: "application/json", value: '{"visible":true}' },
  ]), []);
  assert.deepEqual(disclosureTitles(html), ["Events", "Events"]);
  assert.ok(html.indexOf("</details>") < html.indexOf("Between calls"));
  assert.ok(html.lastIndexOf("</details>") < html.indexOf("application/json"));
});

test("collapses detail-free operations together without nesting disclosures", () => {
  const html = renderMessage(transcriptMessage("operations", [
    { type: "operation", id: "read", kind: "read", status: "completed", title: "Read file", paths: [] },
    { type: "operation", id: "write", kind: "fileChange", status: "failed", title: "Write file", detail: "<script>failed</script>", paths: [] },
    { type: "codex_message", id: "final", phase: "final_answer", text: "Final answer" },
  ]), []);
  assert.deepEqual(disclosureTitles(html), ["Events"]);
  assert.equal(html.match(/<details /g)?.length, 1);
  assert.ok(html.includes("&lt;script&gt;failed&lt;/script&gt;"));
  assert.ok(html.indexOf("</details>") < html.indexOf("Final answer"));
});

test("user input ends an Events group even when it contains tool-shaped parts", () => {
  const input = transcriptMessage("input", [{ type: "text", text: "User follow-up" }, call("input-call")], "user");
  const html = renderConversationMessages([
    transcriptMessage("before", [reasoning("Before input")]),
    input,
    transcriptMessage("after", [call("after-input"), result("after-input")]),
  ], []);
  assert.deepEqual(disclosureTitles(html), ["Events", "Events"]);
  const inputStart = html.indexOf('data-message-id="input"');
  assert.ok(html.indexOf("</details>") < inputStart);
  assert.ok(inputStart < html.lastIndexOf("<details "));
  assert.equal(html.match(/class="user-input-bubble"/g)?.length, 1);
});

test("separates prose and multiple code fences while preserving code whitespace", () => {
  assert.deepEqual(messageTextBlocks("Before\n```rust\nfn main() {\n\tprintln!(\"你好\");\n}\n```\nBetween\n~~~\nx < y\n~~~\nAfter"), [
    { type: "text", text: "Before\n" },
    { type: "code", language: "rust", text: "fn main() {\n\tprintln!(\"你好\");\n}\n" },
    { type: "text", text: "Between\n" },
    { type: "code", language: "", text: "x < y\n" },
    { type: "text", text: "After" },
  ]);
});

test("handles longer, indented, CRLF and unfinished fences", () => {
  assert.deepEqual(messageTextBlocks("  ````md\r\n  ```js\r\n  x\r\n  ```\r\n  `````\r\n"), [
    { type: "code", language: "md", text: "```js\r\nx\r\n```\r\n" },
  ]);
  assert.deepEqual(messageTextBlocks("```python\nprint(1)"), [{ type: "code", language: "python", text: "print(1)" }]);
  assert.deepEqual(messageTextBlocks("```\n~~~\n```"), [{ type: "code", language: "", text: "~~~\n" }]);
  assert.deepEqual(messageTextBlocks("```\n```"), [{ type: "code", language: "", text: "" }]);
  assert.deepEqual(messageTextBlocks("inline `code` and ```fence```"), [{ type: "text", text: "inline `code` and ```fence```" }]);
});

test("treats prose, language labels and code as text, including HTML payloads", () => {
  const html = renderMessageText('<img src=x onerror=alert(1)>\n```\"><svg/onload=alert(2)>\n</code><script>alert(3)</script>\n```');
  assert.ok(!html.includes("<img"));
  assert.ok(!html.includes("<script"));
  assert.ok(!html.includes("<svg/onload"));
  assert.ok(html.includes("&lt;/code&gt;&lt;script&gt;"));
  assert.ok(html.includes("&quot;&gt;&lt;svg/onload=alert(2)&gt;"));
  assert.match(renderMessageText("```text\nhello\n```"), /Plain text/);
  assert.match(renderMessageText("```unknown-language\nhello\n```"), /unknown-language/);
});

test("renders safe Markdown tables, external links, and project file references", () => {
  const html = renderMessageText([
    "## Files",
    "",
    "| File | Purpose |",
    "| :--- | ---: |",
    "| [main](src/main.rs#L12C4) | **entry** |",
    "",
    "See `crates/domain/src/message.rs:42`, crates/agent_adapters/src/provider_gateway.rs:70, [main.rs (line 8)](src/main.rs), and [docs](https://example.com/docs).",
  ].join("\n"));
  assert.ok(html.includes("<table>"));
  assert.ok(html.includes('class="align-right"'));
  assert.ok(html.includes('data-file-path="src/main.rs"'));
  assert.ok(html.includes('data-file-line="12"'));
  assert.ok(html.includes('data-file-column="4"'));
  assert.ok(html.includes('data-file-path="crates/domain/src/message.rs"'));
  assert.ok(html.includes('data-file-path="crates/agent_adapters/src/provider_gateway.rs"'));
  assert.ok(html.includes('title="Open src/main.rs at line 8"'));
  assert.ok(html.includes('href="https://example.com/docs"'));
  assert.ok(html.includes("<strong>entry</strong>"));
  assert.deepEqual(parseFileReference("src/main.rs:12:4"), { path: "src/main.rs", line: 12, column: 4 });
  assert.deepEqual(parseFileReference("C:\\project\\src\\main.rs:12"), { path: "C:\\project\\src\\main.rs", line: 12 });
  assert.equal(parseFileReference("https://example.com/file.rs#L1"), undefined);
  assert.equal(parseFileReference("#section"), undefined);
  const unsafe = renderMessageText("[unsafe](javascript:alert(1)) https://example.com/file.rs");
  assert.ok(!unsafe.includes("javascript:"));
  assert.ok(!unsafe.includes('data-file-path="//example.com/file.rs"'));
});

const timestamp = Date.UTC(2026, 8, 7, 3, 7, 42);
const input: WorkspaceMessage = {
  id: "message", project_id: "project", parent_message_id: null, role: "user", kind: "standard",
  text: "system prompt是怎么构建的？\n```literal input```", created_at: timestamp,
};

test("groups all input parts in one plain-text bubble without a user avatar", () => {
  const message = projectMessage(input, null);
  message.parts.push({ type: "text", text: "<b>more text</b>" });
  const html = renderMessage(message, []);
  assert.equal(html.match(/class="user-input-bubble"/g)?.length, 1);
  assert.ok(html.includes("```literal input```"));
  assert.ok(html.includes("&lt;b&gt;more text&lt;/b&gt;"));
  assert.ok(!html.includes('class="message-avatar"'));
  assert.ok(!html.includes('class="code-block"'));
  assert.ok(html.includes(`datetime="${new Date(timestamp).toISOString()}"`));
});

test("carries persisted times through projection for every role and tool result", () => {
  for (const role of ["system", "user", "assistant"] as const) {
    for (const kind of ["standard", "tool_result"] as const) {
      const message = projectMessage({ ...input, role, kind, text: null, data: { output: "ok" } }, "agent");
      assert.equal(message.createdAt, timestamp);
      const html = renderMessage(message, []);
      assert.ok(html.includes(`datetime="${new Date(timestamp).toISOString()}"`));
      if (kind === "tool_result") assert.ok(!html.includes('class="user-input-bubble"'));
    }
  }
  const tool = projectMessage({ ...input, role: "assistant", text: null, data: { tool_use: { tool_name: "read", arguments: {} } } }, "agent");
  assert.equal(tool.parts[0]?.type, "tool_use");
  assert.ok(renderMessage(tool, []).includes(`datetime="${new Date(timestamp).toISOString()}"`));
});

test("projects and renders persisted Codex operation records with expandable detail", () => {
  const message = projectMessage({
    ...input,
    role: "assistant",
    text: "Done.",
    data: {
      codex: {
        operations: [{
          id: "operation-1",
          kind: "read",
          status: "completed",
          title: "Read file",
          detail: "$ sed -n '1,20p' src/main.rs\nfn main() {}",
          paths: ["src/main.rs"],
        }],
      },
    },
  }, "agent");
  assert.equal(message.parts.length, 2);
  assert.equal(message.parts[0]?.type, "operation");
  assert.equal(message.parts[1]?.type, "codex_message");
  const html = renderMessage(message, []);
  assert.ok(html.includes('<summary><span>Events</span>'));
  assert.ok(html.includes('class="codex-final-answer" data-codex-final-answer'));
  assert.ok(html.indexOf("Read file") < html.indexOf("Done."));
  assert.ok(html.includes('class="operation-record"'));
  assert.ok(html.includes("<summary>"));
  assert.ok(html.includes("Read file"));
  assert.ok(html.includes('data-file-path="src/main.rs"'));
  assert.ok(html.includes("fn main() {}"));
});

test("renders ordered Codex progress collapsed before an independent final answer", () => {
  const message = projectMessage({
    ...input,
    role: "assistant",
    text: "Implemented and verified.",
    data: {
      codex: {
        operations: [{
          id: "operation-1", kind: "read", status: "completed", title: "Read file",
          paths: ["src/main.rs"],
        }],
        output_items: [
          { type: "message", id: "commentary-1", phase: "commentary", text: "Inspecting the repository." },
          { type: "operation", id: "operation-1" },
          { type: "message", id: "final-1", phase: "final_answer", text: "Implemented and verified." },
        ],
      },
    },
  }, "agent");

  assert.deepEqual(message.parts.map((part) => part.type), ["codex_message", "operation", "codex_message"]);
  assert.equal(messageText(message), "Implemented and verified.");
  const html = renderMessage(message, []);
  assert.deepEqual(disclosureTitles(html), ["Events"]);
  assert.ok(html.includes('class="message-event-kind">Process</div>'));
  assert.ok(html.includes('class="message-event-kind">Tool call</div>'));
  assert.ok(!/<details[^>]*\sopen[\s>]/.test(html));
  assert.ok(html.includes('class="codex-final-answer" data-codex-final-answer'));
  assert.ok(html.indexOf("Inspecting the repository.") < html.indexOf("Read file"));
  assert.ok(html.indexOf("Read file") < html.indexOf("Implemented and verified."));
});

test("uses the last phased Codex message as the final answer for compatible views", () => {
  const message = projectMessage({
    ...input,
    role: "assistant",
    text: "Final text",
    data: { codex: { output_items: [
      { type: "message", id: "message-1", phase: "commentary", text: "Progress" },
      { type: "message", id: "message-2", text: "Final text" },
    ] } },
  }, "agent");

  assert.equal(message.parts[1]?.type, "codex_message");
  if (message.parts[1]?.type === "codex_message") assert.equal(message.parts[1].phase, "final_answer");
  assert.equal(messageText(message), "Final text");
});

test("does not invent dates for missing or invalid historical timestamps", () => {
  const { created_at: _, ...legacy } = input;
  assert.equal(projectMessage(legacy, null).createdAt, 0);
  for (const timestamp of [0, -1, NaN, Infinity, 1e20]) {
    const html = renderMessageTime(timestamp);
    assert.ok(html.includes("Time unavailable"));
    assert.ok(!html.includes("datetime="));
  }
});

test("projects native API tool results as collapsed records without exposing their storage envelope", () => {
  for (const status of ["succeeded", "denied", "failed", "cancelled"]) {
    const message = projectMessage({
      id: "result", project_id: "project", parent_message_id: "call", role: "user", kind: "tool_result", text: null,
      data: { agent_revision: 9, native_message: { tool_result: { call_id: "call", status, output: '{"stdout":"<script>output</script>"}', error: status === "denied" ? "Permission denied" : null } } },
    }, null);
    const html = renderMessage(message, []);
    assert.ok(html.includes('<summary><span>Events</span>'));
    assert.ok(!/<details[^>]*\sopen[\s>]/.test(html));
    assert.ok(html.includes("Tool result"));
    assert.ok(html.includes(status));
    assert.ok(html.includes(`class="operation-status status-${status}"`));
    assert.ok(!html.includes("agent_revision"));
    assert.ok(!html.includes("<script>output"));
    assert.ok(!html.includes("<strong>You</strong>"));
  }
});

test("denied and cancelled tool results have danger and neutral status colors", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8");
  assert.match(styles, /\.operation-status\.status-denied\s*\{[^}]*color: var\(--danger\);/);
  assert.match(styles, /\.operation-status\.status-cancelled\s*\{[^}]*color: var\(--text-muted\);/);
});

test("preserves ordered API assistant text and tool uses from native submessages", () => {
  const message = projectMessage({
    id: "call", project_id: "project", parent_message_id: "input", role: "assistant", kind: "standard", text: "Inspecting",
    data: { native_message: { sub_messages: [
      { type: "text", text: "Inspecting" },
      { type: "tool_use", call_id: "call-1", tool_name: "grep", arguments: '{"pattern":"^"}' },
    ] } },
  }, null);
  assert.deepEqual(message.parts.map((part) => part.type), ["text", "tool_use"]);
  const html = renderMessage(message, []);
  assert.ok(html.includes("Inspecting"));
  assert.ok(html.includes("grep"));
});
