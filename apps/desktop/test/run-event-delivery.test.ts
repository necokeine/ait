import assert from "node:assert/strict";
import test from "node:test";

import {
  BoundedRunEventDelivery,
  BoundedRunStreamBacklog,
  RUN_EVENT_BUFFER_CHARACTER_LIMIT,
  RUN_EVENT_BUFFER_LIMIT,
  cursorAfterEvent,
} from "../src/run-event-delivery.js";
import type { ControlEvent, RunStreamFrame } from "../src/types.js";

function event(cursor: number, kind = "run.progress"): ControlEvent {
  return {
    api_version: 1,
    cursor,
    kind,
    entity_id: "run-a",
    body: { run_id: "run-a", seq: cursor, type: "text_delta", delta: "x" },
    created_at: cursor,
  };
}

function controlledDelivery(frames: RunStreamFrame[]) {
  const scheduled: Array<() => void> = [];
  const delivery = new BoundedRunEventDelivery((frame) => frames.push(frame), {
    schedule: (callback) => {
      scheduled.push(callback);
      return {} as ReturnType<typeof setTimeout>;
    },
    cancel: () => {},
  });
  return { delivery, runScheduled: () => scheduled.shift()?.() };
}

test("batches one renderer frame and waits for its acknowledgement", () => {
  const frames: RunStreamFrame[] = [];
  const { delivery, runScheduled } = controlledDelivery(frames);
  for (let cursor = 1; cursor <= 100; cursor += 1) {
    delivery.enqueue({ type: "event", event: event(cursor) });
  }

  assert.equal(frames.length, 0);
  assert.equal(delivery.size, 100);
  runScheduled();
  assert.equal(frames.length, 1);
  assert.equal(frames[0]?.updates.length, 100);
  assert.equal(delivery.waitingForAcknowledgement, true);
});

test("50k replay behind a slow renderer stays bounded and converges by resync", () => {
  const frames: RunStreamFrame[] = [];
  const { delivery, runScheduled } = controlledDelivery(frames);
  delivery.enqueue({ type: "event", event: event(1) });
  runScheduled();
  assert.equal(frames.length, 1);

  for (let cursor = 2; cursor <= 50_000; cursor += 1) {
    delivery.enqueue({ type: "event", event: event(cursor) });
  }
  assert.ok(delivery.size <= RUN_EVENT_BUFFER_LIMIT);
  assert.ok(delivery.characterSize <= RUN_EVENT_BUFFER_CHARACTER_LIMIT);
  assert.equal(frames.length, 1, "no second IPC frame is sent before the slow renderer acknowledges");

  delivery.acknowledge(frames[0]!.id);
  runScheduled();
  assert.equal(frames.length, 2);
  assert.deepEqual(frames[1]!.updates, [{ type: "resync", cursor: 50_000 }]);
  assert.equal(delivery.size, 0);
});

test("renderer backlog stays bounded while a slow snapshot is in flight", () => {
  const backlog = new BoundedRunStreamBacklog();
  for (let cursor = 1; cursor <= 50_000; cursor += 1) {
    backlog.push([{ type: "event", event: event(cursor) }]);
  }

  assert.ok(backlog.size <= RUN_EVENT_BUFFER_LIMIT);
  assert.ok(backlog.characterSize <= RUN_EVENT_BUFFER_CHARACTER_LIMIT);
  assert.deepEqual(backlog.drain(), { updates: [], resync: true });
  assert.deepEqual(backlog.drain(), { updates: [], resync: false });
});

test("future cursor reset replaces the local cursor used by the next reconnect", () => {
  let cursor = 90_000;
  cursor = cursorAfterEvent(cursor, event(42, "stream.reset_required"));
  assert.equal(cursor, 42);
  cursor = cursorAfterEvent(cursor, event(43));
  assert.equal(cursor, 43);
});
