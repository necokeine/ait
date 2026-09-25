import { METHODS, type MethodSpec } from "./methods";
import {
    eventMessage,
    responseMessage,
    rpcError,
    serverInfo,
} from "./messages";
import {
    object,
    strings,
    type Payload,
    type Transport,
    type TransportFactory,
} from "./types";

export const CHANNEL_CAPABILITIES = Array.from({ length: 4 }, (_, channel) => [
    ...new Set([
        "connection.ping",
        "subscription.release.request",
        ...Object.values(METHODS)
            .filter((spec) => spec.channel === channel)
            .map((spec) => spec.method),
    ]),
]);

interface Pending {
    sourceId: string;
    request: Payload;
    spec: MethodSpec;
    channel: number;
    rawPing: boolean;
    timer: ReturnType<typeof setTimeout>;
}

/** Adapt the existing UI SDK to Rust v1.0, preserving physical subscription ownership. */
export function createRustServerTransportFactory(
    baseFactory: TransportFactory,
): TransportFactory {
    return ({ url, headers }) => {
        const parsed = new URL(url);
        if (
            !/^(ws|wss):$/.test(parsed.protocol) ||
            parsed.pathname !== "/v1/ws" ||
            parsed.username ||
            parsed.password ||
            parsed.search ||
            parsed.hash
        ) {
            throw new Error("Invalid Rust server WebSocket endpoint");
        }
        const openHandlers = new Set<() => void>();
        const closeHandlers = new Set<(event?: unknown) => void>();
        const errorHandlers = new Set<(event?: unknown) => void>();
        const messageHandlers = new Set<
            (data: unknown, binary: boolean) => void
        >();
        const channels: Transport[] = [];
        const cleanup: (() => void)[] = [];
        const opened = new Set<number>();
        const negotiated = new Map<number, Set<string>>();
        const subscriptions = new Map<string, number>();
        const pending = new Map<string, Pending>();
        let disposed = false;
        let ready = false;
        let helloSent = false;
        let sequence = 0;
        let info: Payload | null = null;
        let implemented = new Set<string>();
        const setupTimer = setTimeout(
            () => fail(new Error("Rust server handshake timed out")),
            10_000,
        );

        function emit(value: Payload): void {
            if (!disposed)
                for (const handler of messageHandlers)
                    handler(JSON.stringify(value), false);
        }

        function dispose(code = 1000, reason = "Client closed"): void {
            if (disposed) return;
            disposed = true;
            clearTimeout(setupTimer);
            for (const item of pending.values()) clearTimeout(item.timer);
            pending.clear();
            subscriptions.clear();
            for (const remove of cleanup) remove();
            for (const channel of channels) channel.close(code, reason);
        }

        function fail(error: Error): void {
            if (disposed) return;
            dispose(1000, "Connection failed");
            for (const handler of errorHandlers) handler(error);
            for (const handler of closeHandlers)
                handler({ code: 1006, reason: error.message });
        }

        function receive(
            channel: number,
            data: unknown,
            binary: boolean,
        ): void {
            if (disposed) return;
            if (binary) {
                if (!ready)
                    throw new Error(
                        "Binary data received before Rust handshake",
                    );
                for (const handler of messageHandlers) handler(data, true);
                return;
            }
            const message = object(JSON.parse(String(data)));
            if (message.type === "server_info") {
                if (!helloSent || negotiated.has(channel))
                    throw new Error("Unexpected Rust server hello");
                const nextInfo = object(message.info);
                const protocol = object(nextInfo.protocol);
                if (
                    protocol.major !== 1 ||
                    protocol.minor !== 0 ||
                    typeof nextInfo.server_id !== "string"
                ) {
                    throw new Error("Unsupported Rust server protocol");
                }
                if (
                    info &&
                    (info.server_id !== nextInfo.server_id ||
                        info.instance_id !== nextInfo.instance_id)
                ) {
                    throw new Error("Rust server restarted during handshake");
                }
                info = nextInfo;
                implemented = new Set(
                    strings(nextInfo.implemented_capabilities),
                );
                negotiated.set(
                    channel,
                    new Set(strings(message.negotiated_capabilities)),
                );
                if (negotiated.size === CHANNEL_CAPABILITIES.length) {
                    ready = true;
                    clearTimeout(setupTimer);
                    emit(serverInfo(info, implemented));
                }
                return;
            }
            if (!ready)
                throw new Error(
                    `Rust handshake rejected: ${String(message.code ?? message.type)}`,
                );
            if (message.type === "event") {
                if (message.method === "status.server_info") {
                    const params = object(message.params);
                    const update = serverInfo(object(params.info), implemented);
                    const payload = object(object(update.message).payload);
                    if (typeof params.subscriptionId === "string")
                        payload.subscriptionId = params.subscriptionId;
                    emit(update);
                    return;
                }
                emit(eventMessage(String(message.method), message.params));
                return;
            }
            if (message.type !== "response" && message.type !== "error") {
                throw new Error("Unexpected Rust server envelope");
            }
            const id = message.request_id;
            if (typeof id !== "string") {
                // Uncorrelated event failures must be visible, never converted into a successful reply.
                for (const handler of errorHandlers)
                    handler(new Error(`Rust server: ${String(message.code)}`));
                return;
            }
            const item = pending.get(id);
            if (!item) return; // A timed-out request can complete after its SDK waiter is gone.
            if (item.channel !== channel)
                throw new Error("Rust response came from the wrong connection");
            pending.delete(id);
            clearTimeout(item.timer);
            if (message.type === "error") {
                emit(
                    rpcError(
                        item.sourceId,
                        String(item.request.type),
                        String(message.code),
                        String(message.message),
                    ),
                );
                return;
            }
            const result = object(message.result);
            if (typeof result.subscriptionId === "string")
                subscriptions.set(result.subscriptionId, channel);
            if (
                item.spec.method === "subscription.release.request" &&
                typeof item.request.subscriptionId === "string"
            ) {
                subscriptions.delete(item.request.subscriptionId);
            }
            if (item.rawPing) emit({ type: "pong" });
            else if (item.spec.response)
                emit(
                    responseMessage(
                        item.spec.response,
                        item.sourceId,
                        result,
                        item.request,
                    ),
                );
        }

        function request(message: Payload, rawPing = false): void {
            const name = String(message.type);
            const spec = METHODS[name];
            if (!spec)
                throw new Error(`No Rust server method mapping for ${name}`);
            const callback =
                spec.kind === "response" ? object(message.payload) : undefined;
            const sourceId =
                typeof callback?.requestId === "string"
                    ? callback.requestId
                    : typeof message.requestId === "string"
                      ? message.requestId
                      : `adapter-${++sequence}`;
            const channel =
                spec.method === "subscription.release.request" &&
                typeof message.subscriptionId === "string"
                    ? (subscriptions.get(message.subscriptionId) ??
                      spec.channel)
                    : spec.channel;
            if (
                !negotiated.get(channel)?.has(spec.method) ||
                !implemented.has(spec.method)
            ) {
                const error = `Rust server does not implement ${spec.method}`;
                if (typeof message.requestId === "string") {
                    emit(rpcError(sourceId, name, "not_implemented", error));
                    return;
                }
                throw new Error(error);
            }
            if (name === "ping" && !rawPing) {
                emit(
                    rpcError(
                        sourceId,
                        name,
                        "unsupported_capability",
                        "Rust server does not provide server-side ping timestamps",
                    ),
                );
                return;
            }
            const { type: _type, requestId: _requestId, ...params } = message;
            // This requestId identifies a provider permission, not just the UI RPC waiter.
            if (name === "agent_permission_response")
                params.requestId = message.requestId;
            if (rawPing) params.nonce = sourceId;
            if (spec.kind !== "request") {
                channels[channel].send(
                    JSON.stringify({
                        type: spec.kind,
                        method: spec.method,
                        params: callback ?? params,
                        ...(spec.kind === "response"
                            ? { request_id: sourceId }
                            : {}),
                    }),
                );
                return;
            }
            if (pending.size >= 256)
                throw new Error("Too many pending Rust server requests");
            const id = `rust-${++sequence}`;
            const timer = setTimeout(() => {
                if (!pending.delete(id) || disposed) return;
                emit(
                    rpcError(
                        sourceId,
                        name,
                        "timeout",
                        "Rust server request timed out",
                    ),
                );
            }, 300_000);
            pending.set(id, {
                sourceId,
                request: message,
                spec,
                channel,
                rawPing,
                timer,
            });
            try {
                channels[channel].send(
                    JSON.stringify({
                        type: "request",
                        request_id: id,
                        method: spec.method,
                        params,
                    }),
                );
            } catch (error) {
                pending.delete(id);
                clearTimeout(timer);
                throw error;
            }
        }

        try {
            for (
                let index = 0;
                index < CHANNEL_CAPABILITIES.length;
                index += 1
            ) {
                // No subprotocol or token-bearing URL: Electron/native sends only the Bearer header.
                const channel = baseFactory({ url, headers });
                channels.push(channel);
                cleanup.push(
                    channel.onOpen(() => {
                        opened.add(index);
                        if (
                            opened.size === CHANNEL_CAPABILITIES.length &&
                            !disposed
                        ) {
                            for (const handler of openHandlers) handler();
                        }
                    }),
                    channel.onMessage((data, binary) => {
                        try {
                            receive(index, data, binary);
                        } catch (error) {
                            fail(
                                error instanceof Error
                                    ? error
                                    : new Error("Invalid Rust server response"),
                            );
                        }
                    }),
                    channel.onError(() =>
                        fail(
                            new Error(
                                "Rust server transport failed; check address and Bearer token",
                            ),
                        ),
                    ),
                    channel.onClose((event) => {
                        if (disposed) return;
                        dispose();
                        for (const handler of closeHandlers) handler(event);
                    }),
                );
            }
        } catch (error) {
            dispose();
            throw error;
        }

        return {
            send(data) {
                if (disposed)
                    throw new Error("Rust server connection is closed");
                if (typeof data !== "string") {
                    if (!ready) throw new Error("Rust server is not ready");
                    const bytes =
                        data instanceof ArrayBuffer
                            ? new Uint8Array(data)
                            : data;
                    if (!bytes.length)
                        throw new Error("Empty Rust binary frame");
                    // Rust preserves Paseo's binary opcode families: terminal < 0x10, files >= 0x10.
                    channels[bytes[0] < 0x10 ? 1 : 2].send(data);
                    return;
                }
                const message = object(JSON.parse(data));
                if (message.type === "hello") {
                    if (
                        helloSent ||
                        opened.size !== CHANNEL_CAPABILITIES.length
                    )
                        throw new Error("Unexpected client hello");
                    if (typeof message.clientId !== "string")
                        throw new Error("Client ID is required");
                    helloSent = true;
                    for (const [index, channel] of channels.entries()) {
                        channel.send(
                            JSON.stringify({
                                type: "hello",
                                client_id: message.clientId,
                                protocol: {
                                    major: 1,
                                    min_minor: 0,
                                    max_minor: 0,
                                },
                                capabilities: CHANNEL_CAPABILITIES[index],
                                required_capabilities: [],
                            }),
                        );
                    }
                    return;
                }
                if (!ready) throw new Error("Rust server is not ready");
                if (message.type === "ping") request({ type: "ping" }, true);
                else if (message.type === "session")
                    request(object(message.message));
                else
                    throw new Error(
                        `Unsupported client envelope: ${String(message.type)}`,
                    );
            },
            close: dispose,
            onOpen: (handler) => {
                openHandlers.add(handler);
                return () => {
                    openHandlers.delete(handler);
                };
            },
            onClose: (handler) => {
                closeHandlers.add(handler);
                return () => {
                    closeHandlers.delete(handler);
                };
            },
            onError: (handler) => {
                errorHandlers.add(handler);
                return () => {
                    errorHandlers.delete(handler);
                };
            },
            onMessage: (handler) => {
                messageHandlers.add(handler);
                return () => {
                    messageHandlers.delete(handler);
                };
            },
        };
    };
}
