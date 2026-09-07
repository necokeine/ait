import type { ControlEvent, RunStreamFrame, RunStreamUpdate } from "./types.js";

export const RUN_EVENT_BUFFER_LIMIT = 512;
export const RUN_EVENT_BUFFER_CHARACTER_LIMIT = 1024 * 1024;
export const RUN_EVENT_FRAME_DELAY_MS = 16;

interface DeliveryOptions {
  limit?: number;
  characterLimit?: number;
  delayMs?: number;
  schedule?: (callback: () => void, delayMs: number) => ReturnType<typeof setTimeout>;
  cancel?: (handle: ReturnType<typeof setTimeout>) => void;
}

/** Bounded renderer-side queue used while an authoritative snapshot is in flight. */
export class BoundedRunStreamBacklog {
  private readonly updates: RunStreamUpdate[] = [];
  private resync = false;
  private characters = 0;

  constructor(
    private readonly limit = RUN_EVENT_BUFFER_LIMIT,
    private readonly characterLimit = RUN_EVENT_BUFFER_CHARACTER_LIMIT,
  ) {}

  push(incoming: RunStreamUpdate[]): void {
    if (this.resync) return;
    const incomingCharacters = incoming.reduce((total, update) => total + updateCharacters(update), 0);
    if (incoming.some((update) => update.type === "resync")
      || this.updates.length + incoming.length > this.limit
      || this.characters + incomingCharacters > this.characterLimit) {
      this.updates.length = 0;
      this.characters = 0;
      this.resync = true;
      return;
    }
    this.updates.push(...incoming);
    this.characters += incomingCharacters;
  }

  drain(): { updates: RunStreamUpdate[]; resync: boolean } {
    const result = { updates: this.updates.splice(0), resync: this.resync };
    this.characters = 0;
    this.resync = false;
    return result;
  }

  get size(): number {
    return this.updates.length;
  }

  get characterSize(): number {
    return this.characters;
  }
}

/**
 * Per-renderer delivery with one acknowledged frame in flight and a bounded
 * main-process backlog. Overflow converges through an authoritative snapshot.
 */
export class BoundedRunEventDelivery {
  private readonly limit: number;
  private readonly characterLimit: number;
  private readonly delayMs: number;
  private readonly schedule: NonNullable<DeliveryOptions["schedule"]>;
  private readonly cancel: NonNullable<DeliveryOptions["cancel"]>;
  private readonly pending: RunStreamUpdate[] = [];
  private pendingCharacters = 0;
  private overflowCursor: number | undefined;
  private overflowConnection: boolean | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private inFlight: number | undefined;
  private nextFrameId = 1;

  constructor(
    private readonly send: (frame: RunStreamFrame) => void,
    options: DeliveryOptions = {},
  ) {
    this.limit = options.limit ?? RUN_EVENT_BUFFER_LIMIT;
    this.characterLimit = options.characterLimit ?? RUN_EVENT_BUFFER_CHARACTER_LIMIT;
    this.delayMs = options.delayMs ?? RUN_EVENT_FRAME_DELAY_MS;
    this.schedule = options.schedule ?? setTimeout;
    this.cancel = options.cancel ?? clearTimeout;
  }

  enqueue(update: RunStreamUpdate): void {
    const characters = updateCharacters(update);
    if (this.overflowCursor !== undefined) {
      this.absorbOverflow(update);
    } else if (update.type === "connection") {
      const previous = this.pending.findLastIndex((candidate) => candidate.type === "connection");
      if (previous >= 0) {
        this.pendingCharacters -= updateCharacters(this.pending[previous]!);
        this.pending[previous] = update;
        this.pendingCharacters += characters;
      } else if (this.canBuffer(characters)) {
        this.pending.push(update);
        this.pendingCharacters += characters;
      } else {
        this.startOverflow(update);
      }
    } else if (this.canBuffer(characters)) {
      this.pending.push(update);
      this.pendingCharacters += characters;
    } else {
      this.startOverflow(update);
    }
    if (this.inFlight === undefined) this.ensureScheduled();
  }

  acknowledge(frameId: number): void {
    if (frameId !== this.inFlight) return;
    this.inFlight = undefined;
    if (this.size > 0) this.ensureScheduled();
  }

  close(): void {
    if (this.timer !== undefined) this.cancel(this.timer);
    this.timer = undefined;
    this.inFlight = undefined;
    this.pending.length = 0;
    this.pendingCharacters = 0;
    this.overflowCursor = undefined;
    this.overflowConnection = undefined;
  }

  get size(): number {
    return this.overflowCursor === undefined ? this.pending.length : 1;
  }

  get characterSize(): number {
    return this.pendingCharacters;
  }

  get waitingForAcknowledgement(): boolean {
    return this.inFlight !== undefined;
  }

  private absorbOverflow(update: RunStreamUpdate): void {
    if (update.type === "connection") {
      this.overflowConnection = update.connected;
    } else if (update.type === "event") {
      this.overflowCursor = Math.max(this.overflowCursor ?? 0, update.event.cursor);
    } else {
      this.overflowCursor = Math.max(this.overflowCursor ?? 0, update.cursor);
    }
  }

  private canBuffer(characters: number): boolean {
    return this.pending.length < this.limit
      && this.pendingCharacters + characters <= this.characterLimit;
  }

  private startOverflow(update: RunStreamUpdate): void {
    const cursor = update.type === "event" ? update.event.cursor : update.type === "resync" ? update.cursor : 0;
    this.overflowCursor = maxEventCursor(this.pending, cursor);
    this.overflowConnection = update.type === "connection"
      ? update.connected
      : latestConnection(this.pending);
    this.pending.length = 0;
    this.pendingCharacters = 0;
  }

  private ensureScheduled(): void {
    if (this.timer !== undefined || this.size === 0) return;
    this.timer = this.schedule(() => this.flush(), this.delayMs);
  }

  private flush(): void {
    this.timer = undefined;
    if (this.inFlight !== undefined) return;
    const updates = this.takePending();
    if (updates.length === 0) return;
    const frame = { id: this.nextFrameId++, updates };
    this.inFlight = frame.id;
    this.send(frame);
  }

  private takePending(): RunStreamUpdate[] {
    if (this.overflowCursor !== undefined) {
      const updates: RunStreamUpdate[] = [];
      if (this.overflowConnection !== undefined) {
        updates.push({ type: "connection", connected: this.overflowConnection });
      }
      updates.push({ type: "resync", cursor: this.overflowCursor });
      this.overflowCursor = undefined;
      this.overflowConnection = undefined;
      return updates;
    }
    const pending = this.pending.splice(0);
    this.pendingCharacters = 0;
    return pending;
  }
}

/** A server reset replaces a future local cursor instead of preserving it. */
export function cursorAfterEvent(current: number, event: ControlEvent): number {
  return event.kind === "stream.reset_required"
    ? event.cursor
    : Math.max(current, event.cursor);
}

function maxEventCursor(updates: RunStreamUpdate[], initial: number): number {
  return updates.reduce((cursor, update) => update.type === "event"
    ? Math.max(cursor, update.event.cursor)
    : cursor, initial);
}

function latestConnection(updates: RunStreamUpdate[]): boolean | undefined {
  for (let index = updates.length - 1; index >= 0; index -= 1) {
    const update = updates[index];
    if (update?.type === "connection") return update.connected;
  }
  return undefined;
}

function updateCharacters(update: RunStreamUpdate): number {
  try {
    return JSON.stringify(update).length;
  } catch {
    return RUN_EVENT_BUFFER_CHARACTER_LIMIT + 1;
  }
}
