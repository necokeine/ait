import { describe, expect, it, vi } from "vitest";
import { createBrowserRustTransportFactory } from "./browser-transport";
import type { Transport } from "./types";

const url = "ws://127.0.0.1:7316/v1/ws";
const headers = { Authorization: "Bearer test-secret" };
const ticket = "a".repeat(32);

function setup(
  fetcher = vi
    .fn<(url: string, options: RequestInit) => Promise<Response>>()
    .mockResolvedValue(Response.json({ ticket })),
) {
  const events = new Map<string, (...args: any[]) => void>();
  const detach = vi.fn();
  const base: Transport = {
    send: vi.fn(),
    close: vi.fn(),
    onOpen: (handler) => {
      events.set("open", handler);
      return detach;
    },
    onClose: (handler) => {
      events.set("close", handler);
      return detach;
    },
    onError: (handler) => {
      events.set("error", handler);
      return detach;
    },
    onMessage: (handler) => {
      events.set("message", handler);
      return detach;
    },
  };
  const connect = vi.fn().mockReturnValue(base);
  const transport = createBrowserRustTransportFactory(fetcher, connect)({ url, headers });
  const error = vi.fn();
  transport.onError(error);
  return { transport, fetcher, connect, base, error, events, detach };
}

describe("Rust browser transport", () => {
  it("exchanges Bearer over HTTP and only sends the one-use ticket on WebSocket", async () => {
    const fixture = setup();
    const opened = vi.fn();
    const message = vi.fn();
    const closed = vi.fn();
    fixture.transport.onOpen(opened);
    fixture.transport.onMessage(message);
    fixture.transport.onClose(closed);
    await vi.waitFor(() => expect(fixture.connect).toHaveBeenCalledOnce());
    expect(fixture.fetcher).toHaveBeenCalledWith(
      "http://127.0.0.1:7316/v1/auth/ws-ticket",
      expect.objectContaining({
        method: "POST",
        headers,
        credentials: "omit",
        cache: "no-store",
        redirect: "error",
      }),
    );
    expect(fixture.connect).toHaveBeenCalledWith({ url, protocols: [`ait.ticket.${ticket}`] });
    fixture.events.get("open")!();
    expect(opened).toHaveBeenCalledOnce();
    const bytes = new Uint8Array([1, 2, 3]);
    fixture.events.get("message")!(bytes, true);
    expect(message).toHaveBeenCalledWith(bytes, true);
    fixture.transport.send(bytes);
    expect(fixture.base.send).toHaveBeenCalledWith(bytes);
    fixture.events.get("close")!({ code: 1000 });
    expect(closed).toHaveBeenCalledWith({ code: 1000 });
    fixture.transport.close(1000, "done");
    fixture.transport.close();
    expect(fixture.detach).toHaveBeenCalledTimes(4);
    expect(fixture.base.close).toHaveBeenCalledTimes(1);
    expect(() => fixture.transport.send("late")).toThrow("not open");
  });

  it.each([401, 403, 429])(
    "reports authentication failure %i without opening sockets",
    async (status) => {
      const fixture = setup(
        vi
          .fn<(url: string, options: RequestInit) => Promise<Response>>()
          .mockResolvedValue(new Response(null, { status })),
      );
      await vi.waitFor(() => expect(fixture.error).toHaveBeenCalledOnce());
      expect(fixture.error.mock.calls[0][0].message).toContain(String(status));
      expect(fixture.connect).not.toHaveBeenCalled();
      fixture.transport.close();
    },
  );

  it("rejects invalid tickets and gives actionable CORS/network errors", async () => {
    for (const response of [
      Response.json({ ticket: "invalid" }),
      Response.json({}),
      new TypeError("Failed to fetch"),
    ]) {
      const fetcher = vi.fn<(url: string, options: RequestInit) => Promise<Response>>();
      if (response instanceof Error) fetcher.mockRejectedValue(response);
      else fetcher.mockResolvedValue(response);
      const fixture = setup(fetcher);
      await vi.waitFor(() => expect(fixture.error).toHaveBeenCalledOnce());
      expect(fixture.connect).not.toHaveBeenCalled();
      fixture.transport.close();
    }
  });

  it("aborts authentication on close and never opens a late socket", async () => {
    let resolve!: (response: Response) => void;
    const fixture = setup(
      vi.fn<(url: string, options: RequestInit) => Promise<Response>>().mockReturnValue(
        new Promise((done) => {
          resolve = done;
        }),
      ),
    );
    await vi.waitFor(() => expect(fixture.fetcher).toHaveBeenCalledOnce());
    const signal = fixture.fetcher.mock.calls[0][1]!.signal!;
    expect(() => fixture.transport.send("early")).toThrow("not open");
    fixture.transport.close();
    expect(signal.aborted).toBe(true);
    resolve(Response.json({ ticket }));
    await new Promise((done) => setTimeout(done, 0));
    expect(fixture.connect).not.toHaveBeenCalled();
    expect(fixture.error).not.toHaveBeenCalled();
  });

  it("reports a missing token after observers are attached", async () => {
    const fetcher = vi.fn<(url: string, options: RequestInit) => Promise<Response>>();
    const transport = createBrowserRustTransportFactory(fetcher)({ url });
    const error = vi.fn();
    transport.onError(error);
    await vi.waitFor(() => expect(error).toHaveBeenCalledOnce());
    expect(error.mock.calls[0][0].message).toBe("Server token required");
    expect(fetcher).not.toHaveBeenCalled();
    transport.close();
  });
});
