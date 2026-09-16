import assert from "node:assert/strict";
import test from "node:test";
import { ProjectSidebar } from "../src/project-sidebar.js";
import type { DesktopSession } from "../src/types.js";

test("a delayed summary cannot overwrite a newer accepted Project view or reopen a collapsed Project", async () => {
  const sidebar = new ProjectSidebar();
  sidebar.expanded.add("a");
  let resolve!: (sessions: DesktopSession[]) => void;
  const pending = sidebar.refresh("a", () => new Promise((done) => { resolve = done; }));
  sidebar.replace("a", []);
  sidebar.expanded.delete("a");
  resolve([{ id: "stale", projectId: "a" } as DesktopSession]);
  await pending;
  assert.deepEqual(sidebar.sessions.get("a"), []);
  assert.equal(sidebar.expanded.has("a"), false);
});

test("summary failures preserve cached Sessions and reject cross-Project responses", async () => {
  const sidebar = new ProjectSidebar();
  const saved = [{ id: "a-session", projectId: "a" } as DesktopSession];
  sidebar.replace("a", saved);
  await sidebar.refresh("a", async () => [{ id: "b-session", projectId: "b" } as DesktopSession]);
  assert.equal(sidebar.sessions.get("a"), saved);
  assert.match(sidebar.errors.get("a")!, /another Project/);
  await sidebar.refresh("a", async () => []);
  assert.equal(sidebar.errors.has("a"), false);
  assert.deepEqual(sidebar.sessions.get("a"), []);
});
