import assert from "node:assert/strict";
import test from "node:test";

import {
  sanitizeSessionPrompt,
  sessionDisplayTitle,
  temporarySessionTitle,
} from "../src/session-titles.js";

test("strips Markdown and instruction markers before collapsing whitespace", () => {
  assert.equal(
    sanitizeSessionPrompt("<instructions>\n# **Fix** [ABC-123](https://example.test)\n-   keep   the API\n</instructions>"),
    "Fix ABC-123 keep the API",
  );
});

test("limits model input and temporary titles by Unicode characters", () => {
  assert.equal(temporarySessionTitle("你".repeat(80)).length, 60);
  assert.equal(Array.from(sanitizeSessionPrompt("😀".repeat(2_100))).length, 2_000);
});

test("prefers a manual name, then generated title, then the legacy short-id fallback", () => {
  assert.equal(sessionDisplayTitle({ id: "1234567890", name: "Mine", title: "Generated" }), "Mine");
  assert.equal(sessionDisplayTitle({ id: "1234567890", name: "", title: "Generated" }), "Generated");
  assert.equal(sessionDisplayTitle({ id: "1234567890", name: "", title: null }), "Session 12345678");
});
