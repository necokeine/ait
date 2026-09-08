import assert from "node:assert/strict";
import test from "node:test";

import {
  BoundedRunEventDelivery,
  BoundedRunStreamBacklog,
  ReadyRunEventDelivery,
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
  const scheduler = controlledScheduler();
  const delivery = new BoundedRunEventDelivery(
    "generation-a",
    (frame) => frames.push(frame),
    scheduler.options,
  );
  return { delivery, runScheduled: scheduler.runScheduled };
}

function controlledScheduler() {
  const scheduled = new Map<ReturnType<typeof setTimeout>, () => void>();
  let nextHandle = 1;
  return {
    options: {
      schedule: (callback: () => void) => {
        const handle = { id: nextHandle++ } as unknown as ReturnType<typeof setTimeout>;
        scheduled.set(handle, callback);
        return handle;
      },
      cancel: (handle: ReturnType<typeof setTimeout>) => { scheduled.delete(handle); },
    },
    runScheduled: () => {
      const entry = scheduled.entries().next().value as
        | [ReturnType<typeof setTimeout>, () => void]
        | undefined;
      if (!entry) return;
      scheduled.delete(entry[0]);
      entry[1]();
    },
  };
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

  delivery.acknowledge("generation-a", frames[0]!.id);
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

test("a lost acknowledgement times out and converges through resync", () => {
  const frames: RunStreamFrame[] = [];
  const { delivery, runScheduled } = controlledDelivery(frames);
  delivery.enqueue({ type: "event", event: event(1) });
  runScheduled();
  delivery.enqueue({ type: "event", event: event(2) });

  runScheduled(); // ACK timeout for frame 1.
  assert.equal(delivery.waitingForAcknowledgement, false);
  runScheduled(); // Recovery frame.
  assert.deepEqual(frames[1], {
    generation: "generation-a",
    id: 2,
    updates: [{ type: "resync", cursor: 2 }],
  });
});

test("refresh waits for the new ready generation and rejects a stale ACK", () => {
  const frames: RunStreamFrame[] = [];
  const scheduler = controlledScheduler();
  const delivery = new ReadyRunEventDelivery((frame) => frames.push(frame), scheduler.options);

  delivery.ready("generation-old", [
    { type: "connection", connected: true },
    { type: "resync", cursor: 10 },
  ]);
  scheduler.runScheduled();
  delivery.acknowledge("generation-old", 1);
  delivery.suspend();
  delivery.enqueue({ type: "event", event: event(11) });
  assert.equal(frames.length, 1, "events during reload are not sent to an unready document");

  delivery.ready("generation-new", [
    { type: "connection", connected: false },
    { type: "resync", cursor: 11 },
  ]);
  scheduler.runScheduled();
  assert.deepEqual(frames[1], {
    generation: "generation-new",
    id: 1,
    updates: [
      { type: "connection", connected: false },
      { type: "resync", cursor: 11 },
    ],
  });
  delivery.acknowledge("generation-old", 1);
  assert.equal(delivery.waitingForAcknowledgement, true);
  delivery.acknowledge("generation-new", 1);
  assert.equal(delivery.waitingForAcknowledgement, false);
});
