import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("places Projects above the bottom navigation group", async () => {
  const html = await readFile(new URL("../src/index.html", import.meta.url), "utf8");
  const sidebar = html.slice(html.indexOf('<aside id="sidebar"'), html.indexOf("</aside>"));
  const projectsHeading = sidebar.indexOf("<span>Projects</span>");
  const projectList = sidebar.indexOf('id="project-list"');
  const footer = sidebar.indexOf('class="sidebar-footer"');

  assert.ok(projectsHeading >= 0, "Projects heading should be present");
  assert.ok(projectsHeading < projectList, "Projects heading should precede its list");
  assert.ok(projectList < footer, "Projects should appear above the footer navigation");

  const footerHtml = sidebar.slice(footer);
  const sessions = footerHtml.indexOf("Sessions");
  const runs = footerHtml.indexOf("Runs");
  const agents = footerHtml.indexOf("Agents");
  const settings = footerHtml.indexOf("Settings");

  assert.ok(sessions >= 0, "Sessions should be in the footer navigation");
  assert.ok(sessions < runs && runs < agents && agents < settings);
  assert.match(footerHtml, /<nav class="primary-nav" aria-label="Workspace">[\s\S]*id="settings-trigger"[\s\S]*<\/nav>/);
});

test("offers Session rename from a right-click action menu", async () => {
  const [html, renderer] = await Promise.all([
    readFile(new URL("../src/index.html", import.meta.url), "utf8"),
    readFile(new URL("../src/renderer.ts", import.meta.url), "utf8"),
  ]);
  assert.match(html, /id="session-context-menu"[\s\S]*id="session-rename-action"/);
  assert.match(html, /id="rename-session-dialog"[\s\S]*id="rename-session-name"/);
  assert.match(renderer, /addEventListener\("contextmenu"/);
  assert.match(renderer, /window\.ait\.renameSession/);
});

test("creates a Session directly with the Project default Agent", async () => {
  const [html, renderer] = await Promise.all([
    readFile(new URL("../src/index.html", import.meta.url), "utf8"),
    readFile(new URL("../src/renderer.ts", import.meta.url), "utf8"),
  ]);

  assert.doesNotMatch(html, /id="session-dialog"/);
  assert.match(renderer, /availableProjectDefaultAgentId\(project, snapshot\.agents\)/);
  assert.match(renderer, /window\.ait\.createSession\(\{ projectId: project\.id, agentId \}\)/);
});

test("uses the titlebar toggle as the persistent Session tree state", async () => {
  const [html, renderer, styles] = await Promise.all([
    readFile(new URL("../src/index.html", import.meta.url), "utf8"),
    readFile(new URL("../src/renderer.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ]);

  assert.match(html, /id="tree-toggle"[^>]*aria-pressed="true"/);
  assert.doesNotMatch(html, /id="tree-close"|Session timeline|legend-selected|>Selected</);
  assert.match(renderer, /setTreeExpanded[\s\S]*setAttribute\("aria-pressed", String\(expanded\)\)/);
  assert.match(styles, /\.icon-button\[aria-pressed="true"\]/);
});

test("starts branches only from the non-leaf Message context menu", async () => {
  const [html, renderer] = await Promise.all([
    readFile(new URL("../src/index.html", import.meta.url), "utf8"),
    readFile(new URL("../src/renderer.ts", import.meta.url), "utf8"),
  ]);

  assert.match(html, /id="message-context-menu"[\s\S]*id="message-start-session-action"/);
  assert.doesNotMatch(html, /Branch from here|select a tree node to branch/i);
  assert.match(renderer, /addEventListener\("contextmenu"[\s\S]*node\?\.children\.length/);
  assert.match(renderer, /directMessageChildren[\s\S]*branchSourceNodeId/);
});

test("shows Message provenance and child-path navigation in details", async () => {
  const renderer = await readFile(new URL("../src/renderer.ts", import.meta.url), "utf8");

  assert.match(renderer, /<dt>Source<\/dt>/);
  assert.match(renderer, /<dt>Created<\/dt>/);
  assert.match(renderer, /<dt>Git revision<\/dt>/);
  assert.match(renderer, /data-child-root-id/);
  assert.match(renderer, /sessionForBranch\(messages, sessions, branchRootId\)/);
});
