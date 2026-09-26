import {
  createWebSocketTransportFactory,
  defaultWebSocketFactory,
} from "@getpaseo/client/internal/daemon-client-websocket-transport";
import type { Transport, TransportFactory } from "./types";

/** Exchange the Bearer token for a single-use, origin-bound WebSocket ticket. */
export function createBrowserRustTransportFactory(
  fetchTicket: (url: string, options: RequestInit) => Promise<Response> = globalThis.fetch.bind(
    globalThis,
  ),
  connect: TransportFactory = createWebSocketTransportFactory(defaultWebSocketFactory),
): TransportFactory {
  return ({ url, headers }) => {
    const controller = new AbortController();
    const open = new Set<() => void>();
    const close = new Set<(event?: unknown) => void>();
    const error = new Set<(event?: unknown) => void>();
    const message = new Set<(data: unknown, binary: boolean) => void>();
    const cleanup: (() => void)[] = [];
    let socket: Transport | null = null;
    let disposed = false;

    async function start() {
      try {
        const endpoint = new URL(url);
        endpoint.protocol = endpoint.protocol === "wss:" ? "https:" : "http:";
        endpoint.pathname = "/v1/auth/ws-ticket";
        const authorization = headers?.Authorization;
        if (!authorization) throw new Error("Server token required");
        const response = await fetchTicket(endpoint.toString(), {
          method: "POST",
          headers: { Authorization: authorization },
          credentials: "omit",
          cache: "no-store",
          redirect: "error",
          signal: controller.signal,
        });
        if (!response.ok) {
          throw new Error(
            response.status === 401
              ? "Incorrect server token (401)"
              : `Server authentication failed (${response.status})`,
          );
        }
        const result: unknown = await response.json();
        const ticket =
          result && typeof result === "object" && "ticket" in result ? result.ticket : null;
        if (typeof ticket !== "string" || !/^[a-f0-9]{32}$/.test(ticket)) {
          throw new Error("Invalid server authentication response");
        }
        if (disposed) return;
        socket = connect({ url, protocols: [`ait.ticket.${ticket}`] });
        cleanup.push(
          socket.onOpen(() => {
            for (const handler of open) handler();
          }),
          socket.onClose((event) => {
            for (const handler of close) handler(event);
          }),
          socket.onError((event) => {
            for (const handler of error) handler(event);
          }),
          socket.onMessage((data, binary) => {
            for (const handler of message) handler(data, binary);
          }),
        );
      } catch (cause) {
        if (disposed) return;
        const failure =
          cause instanceof Error && cause.name !== "TypeError"
            ? cause
            : new Error(
                "Cannot authenticate with server; check its address and allowed web origin",
              );
        for (const handler of error) handler(failure);
      }
    }

    // Register transport observers before reporting validation or network failures.
    void Promise.resolve().then(start);
    return {
      send(data) {
        if (disposed || !socket) throw new Error("Server connection is not open");
        socket.send(data);
      },
      close(code, reason) {
        if (disposed) return;
        disposed = true;
        controller.abort();
        for (const remove of cleanup) remove();
        socket?.close(code, reason);
      },
      onOpen(handler) {
        open.add(handler);
        return () => {
          open.delete(handler);
        };
      },
      onClose(handler) {
        close.add(handler);
        return () => {
          close.delete(handler);
        };
      },
      onError(handler) {
        error.add(handler);
        return () => {
          error.delete(handler);
        };
      },
      onMessage(handler) {
        message.add(handler);
        return () => {
          message.delete(handler);
        };
      },
    };
  };
}
