import assert from "node:assert/strict";
import test from "node:test";
import { messageTextBlocks, renderMessage, renderMessageText, renderMessageTime } from "../src/message-renderer.js";
import { projectMessage, type WorkspaceMessage } from "../src/messages.js";

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

test("does not invent dates for missing or invalid historical timestamps", () => {
  const { created_at: _, ...legacy } = input;
  assert.equal(projectMessage(legacy, null).createdAt, 0);
  for (const timestamp of [0, -1, NaN, Infinity, 1e20]) {
    const html = renderMessageTime(timestamp);
    assert.ok(html.includes("Time unavailable"));
    assert.ok(!html.includes("datetime="));
  }
});
