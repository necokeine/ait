import type { ControlEvent, RunStreamFrame, RunStreamUpdate } from "./types.js";

export const RUN_EVENT_BUFFER_LIMIT = 512;
export const RUN_EVENT_BUFFER_CHARACTER_LIMIT = 1024 * 1024;
export const RUN_EVENT_FRAME_DELAY_MS = 16;
export const RUN_EVENT_ACK_TIMEOUT_MS = 5_000;

interface DeliveryOptions {
  limit?: number;
  characterLimit?: number;
  delayMs?: number;
  ackTimeoutMs?: number;
  schedule?: (callback: () => void, delayMs: number) => ReturnType<typeof setTimeout>;
  cancel?: (handle: ReturnType<typeof setTimeout>) => void;
}

/** Bounded renderer-side queue used while an authoritative view is in flight. */
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
 * main-process backlog. Overflow converges through an authoritative view.
 */
export class BoundedRunEventDelivery {
  private readonly limit: number;
  private readonly characterLimit: number;
  private readonly delayMs: number;
  private readonly ackTimeoutMs: number;
  private readonly schedule: NonNullable<DeliveryOptions["schedule"]>;
  private readonly cancel: NonNullable<DeliveryOptions["cancel"]>;
  private readonly pending: RunStreamUpdate[] = [];
  private pendingCharacters = 0;
  private overflowCursor: number | undefined;
  private overflowConnection: boolean | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private ackTimer: ReturnType<typeof setTimeout> | undefined;
  private inFlight: RunStreamFrame | undefined;
  private nextFrameId = 1;

  constructor(
    private readonly generation: string,
    private readonly send: (frame: RunStreamFrame) => void,
    options: DeliveryOptions = {},
  ) {
    this.limit = options.limit ?? RUN_EVENT_BUFFER_LIMIT;
    this.characterLimit = options.characterLimit ?? RUN_EVENT_BUFFER_CHARACTER_LIMIT;
    this.delayMs = options.delayMs ?? RUN_EVENT_FRAME_DELAY_MS;
    this.ackTimeoutMs = options.ackTimeoutMs ?? RUN_EVENT_ACK_TIMEOUT_MS;
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
    if (!this.inFlight) this.ensureScheduled();
  }

  acknowledge(generation: string, frameId: number): void {
    if (generation !== this.generation || frameId !== this.inFlight?.id) return;
    if (this.ackTimer !== undefined) this.cancel(this.ackTimer);
    this.ackTimer = undefined;
    this.inFlight = undefined;
    if (this.size > 0) this.ensureScheduled();
  }

  close(): void {
    if (this.timer !== undefined) this.cancel(this.timer);
    if (this.ackTimer !== undefined) this.cancel(this.ackTimer);
    this.timer = undefined;
    this.ackTimer = undefined;
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
      this.overflowCursor = cursorAfterEvent(this.overflowCursor ?? 0, update.event);
    } else {
      this.overflowCursor = update.cursor;
    }
  }

  private canBuffer(characters: number): boolean {
    return this.pending.length < this.limit
      && this.pendingCharacters + characters <= this.characterLimit;
  }

  private startOverflow(update: RunStreamUpdate): void {
    const buffered = [...this.pending, update];
    this.overflowCursor = streamCursor(buffered);
    this.overflowConnection = latestConnection(buffered);
    this.pending.length = 0;
    this.pendingCharacters = 0;
  }

  private ensureScheduled(): void {
    if (this.timer !== undefined || this.size === 0) return;
    this.timer = this.schedule(() => this.flush(), this.delayMs);
  }

  private flush(): void {
    this.timer = undefined;
    if (this.inFlight) return;
    const updates = this.takePending();
    if (updates.length === 0) return;
    const frame = { generation: this.generation, id: this.nextFrameId++, updates };
    this.inFlight = frame;
    try {
      this.send(frame);
    } catch {
      this.recoverLostFrame(frame.id);
      return;
    }
    if (this.ackTimeoutMs > 0) {
      this.ackTimer = this.schedule(() => this.recoverLostFrame(frame.id), this.ackTimeoutMs);
    }
  }

  private recoverLostFrame(frameId: number): void {
    if (frameId !== this.inFlight?.id) return;
    const lost = this.inFlight.updates;
    this.inFlight = undefined;
    this.ackTimer = undefined;
    const buffered = this.overflowCursor === undefined
      ? [...lost, ...this.pending]
      : [
        ...lost,
        ...(this.overflowConnection === undefined ? [] : [{
          type: "connection" as const,
          connected: this.overflowConnection,
        }]),
        { type: "resync" as const, cursor: this.overflowCursor },
      ];
    this.pending.length = 0;
    this.pendingCharacters = 0;
    this.overflowCursor = streamCursor(buffered);
    this.overflowConnection = latestConnection(buffered);
    this.ensureScheduled();
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

/**
 * One BrowserWindow document generation. Events are sent only after that
 * document's preload has installed its listener and announced readiness.
 */
export class ReadyRunEventDelivery {
  private delivery: BoundedRunEventDelivery | undefined;
  private generation: string | undefined;

  constructor(
    private readonly send: (frame: RunStreamFrame) => void,
    private readonly options: DeliveryOptions = {},
  ) {}

  ready(generation: string, initial: RunStreamUpdate[]): void {
    this.delivery?.close();
    this.generation = generation;
    this.delivery = new BoundedRunEventDelivery(generation, this.send, this.options);
    for (const update of initial) this.delivery.enqueue(update);
  }

  suspend(): void {
    this.delivery?.close();
    this.delivery = undefined;
    this.generation = undefined;
  }

  enqueue(update: RunStreamUpdate): void {
    this.delivery?.enqueue(update);
  }

  acknowledge(generation: string, frameId: number): void {
    if (generation === this.generation) this.delivery?.acknowledge(generation, frameId);
  }

  close(): void {
    this.suspend();
  }

  get readyGeneration(): string | undefined {
    return this.generation;
  }

  get waitingForAcknowledgement(): boolean {
    return this.delivery?.waitingForAcknowledgement ?? false;
  }
}

/** A server reset replaces a future local cursor instead of preserving it. */
export function cursorAfterEvent(current: number, event: ControlEvent): number {
  return event.kind === "stream.reset_required"
    ? event.cursor
    : Math.max(current, event.cursor);
}

function streamCursor(updates: RunStreamUpdate[]): number {
  return updates.reduce((cursor, update) => {
    if (update.type === "event") return cursorAfterEvent(cursor, update.event);
    if (update.type === "resync") return update.cursor;
    return cursor;
  }, 0);
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
