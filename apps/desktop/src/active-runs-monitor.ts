import type { ActiveRunsCatalog, RunStreamUpdate } from "./types.js";

export interface ActiveRunsState {
  catalog?: ActiveRunsCatalog;
  loading: boolean;
  connected: boolean;
  error?: string;
}

/** Coalesces workspace activity events without coupling them to the selected Project. */
export class ActiveRunsMonitor {
  readonly state: ActiveRunsState = { loading: false, connected: true };
  private active = false;
  private inFlight: Promise<void> | undefined;
  private dirty = false;
  private eventTimer: ReturnType<typeof setTimeout> | undefined;
  private pollTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    private readonly read: () => Promise<ActiveRunsCatalog>,
    private readonly changed: (state: ActiveRunsState) => void,
  ) {}

  setActive(active: boolean): void {
    if (this.active === active) return;
    this.active = active;
    this.clearTimers();
    if (active) void this.refresh();
  }

  handleUpdates(updates: RunStreamUpdate[]): void {
    let invalidated = false;
    for (const update of updates) {
      if (update.type === "connection") {
        invalidated ||= update.connected && !this.state.connected;
        this.state.connected = update.connected;
        if (this.active) this.changed(this.state);
      } else if (update.type === "resync") {
        invalidated = true;
      } else {
        const kind = update.event.kind;
        // Text deltas do not change the catalog. Lifecycle/approval events do.
        invalidated ||= (kind.startsWith("run.") && kind !== "run.progress")
          || kind.startsWith("project.") || kind.startsWith("session.")
          || kind === "stream.reset_required";
      }
    }
    if (!invalidated || !this.active) return;
    if (this.inFlight) this.dirty = true;
    else if (this.eventTimer === undefined) {
      this.eventTimer = setTimeout(() => void this.refresh(), 150);
    }
  }

  refresh(): Promise<void> {
    if (!this.active) return Promise.resolve();
    this.clearTimers();
    if (this.inFlight) {
      this.dirty = true;
      return this.inFlight;
    }
    this.inFlight = this.load().finally(() => { this.inFlight = undefined; });
    return this.inFlight;
  }

  private async load(): Promise<void> {
    this.state.loading = true;
    this.changed(this.state);
    let attempts = 0;
    do {
      attempts += 1;
      this.dirty = false;
      try {
        const catalog = await this.read();
        // Retry an invalidated read once, but keep a busy workspace observable.
        if (!this.dirty || attempts === 2) {
          this.state.catalog = catalog;
          delete this.state.error;
        }
      } catch {
        if (!this.dirty || attempts === 2) this.state.error = "Could not refresh Runs. Displayed results may be out of date.";
      }
    } while (this.active && this.dirty && attempts < 2);
    this.state.loading = false;
    if (this.active) {
      this.changed(this.state);
      // Also recovers missed events and temporarily unavailable Project stores.
      this.pollTimer = setTimeout(() => void this.refresh(), this.dirty ? 150 : 5_000);
    }
  }

  private clearTimers(): void {
    clearTimeout(this.eventTimer);
    clearTimeout(this.pollTimer);
    this.eventTimer = undefined;
    this.pollTimer = undefined;
  }
}
