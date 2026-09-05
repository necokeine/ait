import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("form controls use a theme-aware surface color", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8");

  assert.match(styles, /:root[\s\S]*--control-bg: #10120f;/);
  assert.match(styles, /:root\[data-theme="light"\][\s\S]*--control-bg: #fbfcf8;/);
  assert.match(styles, /\.action-form input, \.action-form select \{[^}]*background: var\(--control-bg\);/);
});

test("conversation messages use a chat layout instead of an event chain", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8");

  assert.match(styles, /\.message\.user \{[^}]*flex-direction: row-reverse;/);
  assert.match(styles, /\.message\.assistant \.message-avatar/);
  assert.doesNotMatch(styles, /\.message::before/);
});
