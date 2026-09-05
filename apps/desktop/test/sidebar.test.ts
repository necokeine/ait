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
