import assert from "node:assert/strict";
import test from "node:test";
import { messageTextBlocks, parseFileReference, renderMessage, renderMessageText, renderMessageTime } from "../src/message-renderer.js";
import { projectMessage, type WorkspaceMessage } from "../src/messages.js";
import { messageText } from "../src/tree.js";

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
  assert.ok(html.includes('<details class="codex-process">'));
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
  assert.ok(html.includes('<details class="codex-process">'));
  assert.ok(!html.includes('<details class="codex-process" open'));
  assert.ok(html.includes('class="codex-final-answer" data-codex-final-answer'));
  assert.ok(html.indexOf("Inspecting the repository.") < html.indexOf("Read file"));
  assert.ok(html.indexOf("Read file") < html.indexOf("Implemented and verified."));
});

test("uses the last phased Codex message as the final answer for compatible snapshots", () => {
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
