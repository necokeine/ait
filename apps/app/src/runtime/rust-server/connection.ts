import { createWebSocketTransportFactory } from "@getpaseo/client/internal/daemon-client-websocket-transport";
import { buildDaemonWebSocketUrl } from "@/utils/daemon-endpoints";
import { isWeb } from "@/constants/platform";
import { createAppWebSocketFactory } from "../websocket-factory";
import { createRustServerTransportFactory } from "./transport";
import { createBrowserRustTransportFactory } from "./browser-transport";
import type { TransportFactory } from "./types";

/** Shared by the host probe and the long-lived runtime; password is the Rust Bearer token. */
export function buildRustClientConfig(
  connection: { endpoint: string; useTls?: boolean; password?: string },
  desktopTransport: TransportFactory | null,
) {
  const url = new URL(
    buildDaemonWebSocketUrl(connection.endpoint, {
      useTls: connection.useTls ?? false,
    }),
  );
  url.pathname = "/v1/ws";
  const nativeTransport = createWebSocketTransportFactory(createAppWebSocketFactory());
  const base: TransportFactory =
    desktopTransport ?? (isWeb ? createBrowserRustTransportFactory() : nativeTransport);
  return {
    url: url.toString(),
    ...(connection.password ? { password: connection.password } : {}),
    transportFactory: createRustServerTransportFactory(base),
  };
}
