import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("form controls use a theme-aware surface color", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8");

  assert.match(styles, /:root[\s\S]*--control-bg: light-dark\(#fbfcf8, #10120f\);/);
  assert.match(styles, /\.action-form input, \.action-form select \{[^}]*background: var\(--control-bg\);/);
});

test("conversation messages use a chat layout instead of an event chain", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8");

  assert.match(styles, /\.message\.user-input \.message-body \{[^}]*align-items: flex-end;/);
  assert.doesNotMatch(styles, /\.message-avatar/);
  assert.doesNotMatch(styles, /\.message::before/);
});
