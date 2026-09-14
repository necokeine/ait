import assert from "node:assert/strict";
import test from "node:test";
import { ActiveRunsMonitor } from "../src/active-runs-monitor.js";
import type { ActiveRunsCatalog, RunStreamUpdate } from "../src/types.js";

const empty: ActiveRunsCatalog = { runs: [], unavailableProjects: [] };
const settle = () => new Promise<void>((resolve) => setImmediate(resolve));
const event = (kind: string): RunStreamUpdate => ({ type: "event", event: {
  api_version: 1, cursor: 1, kind, entity_id: "run", body: { project_id: "another-project" }, created_at: 1,
} });

test("global lifecycle events coalesce, while text deltas do not reread the catalog", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let reads = 0;
  const monitor = new ActiveRunsMonitor(async () => { reads += 1; return empty; }, () => {});
  t.after(() => monitor.setActive(false));
  monitor.setActive(true);
  await settle();
  monitor.handleUpdates([event("run.progress"), event("run.progress")]);
  t.mock.timers.tick(200);
  assert.equal(reads, 1);
  monitor.handleUpdates([event("run.updated"), event("run.approval_requested"), event("run.cancelled")]);
  t.mock.timers.tick(150);
  await settle();
  assert.equal(reads, 2);
});

test("events arriving during a read trigger one fresh read and never publish the stale snapshot", async (t) => {
  const first = Promise.withResolvers<ActiveRunsCatalog>();
  let reads = 0;
  const published: ActiveRunsCatalog[] = [];
  const monitor = new ActiveRunsMonitor(async () => ++reads === 1 ? first.promise : empty,
    (state) => { if (state.catalog) published.push(state.catalog); });
  t.after(() => monitor.setActive(false));
  monitor.setActive(true);
  monitor.handleUpdates([event("run.updated")]);
  monitor.handleUpdates([event("run.cancelled")]);
  const stale = { ...empty, unavailableProjects: [{ projectId: "stale", projectName: "Stale", message: "stale" }] };
  first.resolve(stale);
  await settle();
  assert.equal(reads, 2);
  assert.equal(monitor.state.catalog, empty);
  assert.ok(!published.includes(stale));
});

test("reconnect and stream resync refresh activity even without a selected Project", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let reads = 0;
  const monitor = new ActiveRunsMonitor(async () => { reads += 1; return empty; }, () => {});
  t.after(() => monitor.setActive(false));
  monitor.setActive(true);
  await settle();
  monitor.handleUpdates([{ type: "connection", connected: false }]);
  assert.equal(monitor.state.connected, false);
  monitor.handleUpdates([{ type: "connection", connected: true }, { type: "resync", cursor: 8 }]);
  t.mock.timers.tick(150);
  await settle();
  assert.equal(reads, 2);
  assert.equal(monitor.state.connected, true);
});

test("polling recovers missed events only while Runs is visible", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let reads = 0;
  const monitor = new ActiveRunsMonitor(async () => { reads += 1; return empty; }, () => {});
  t.after(() => monitor.setActive(false));
  monitor.setActive(true);
  await settle();
  t.mock.timers.tick(5_000);
  await settle();
  assert.equal(reads, 2);
  monitor.setActive(false);
  monitor.handleUpdates([event("run.updated")]);
  t.mock.timers.tick(20_000);
  assert.equal(reads, 2);
  monitor.setActive(true);
  await settle();
  assert.equal(reads, 3);
});

test("refresh failures preserve the last snapshot with an error, then recover on retry", async (t) => {
  let failing = false;
  const monitor = new ActiveRunsMonitor(async () => {
    if (failing) throw new Error("offline");
    return empty;
  }, () => {});
  t.after(() => monitor.setActive(false));
  monitor.setActive(true);
  await settle();
  failing = true;
  await monitor.refresh();
  assert.equal(monitor.state.catalog, empty);
  assert.match(monitor.state.error ?? "", /out of date/);
  assert.equal(monitor.state.loading, false);
  failing = false;
  await monitor.refresh();
  assert.equal(monitor.state.error, undefined);
});

test("leaving and reopening Runs during a read requests fresh data without overlapping requests", async (t) => {
  const pending = Promise.withResolvers<ActiveRunsCatalog>();
  let reads = 0;
  const monitor = new ActiveRunsMonitor(async () => ++reads === 1 ? pending.promise : empty, () => {});
  t.after(() => monitor.setActive(false));
  monitor.setActive(true);
  monitor.setActive(false);
  monitor.setActive(true);
  assert.equal(reads, 1);
  pending.resolve(empty);
  await settle();
  assert.equal(reads, 2);
  assert.equal(monitor.state.loading, false);
});

test("continuous lifecycle changes cannot keep a busy workspace in its initial loading state", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let reads = 0;
  const monitor = new ActiveRunsMonitor(async () => {
    reads += 1;
    await Promise.resolve();
    monitor.handleUpdates([event("run.updated")]);
    return empty;
  }, () => {});
  t.after(() => monitor.setActive(false));
  monitor.setActive(true);
  await settle();
  assert.equal(reads, 2);
  assert.equal(monitor.state.catalog, empty);
  assert.equal(monitor.state.loading, false);
  t.mock.timers.tick(150);
  await settle();
  assert.equal(reads, 4);
});
