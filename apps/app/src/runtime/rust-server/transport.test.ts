import { describe, expect, it, vi } from "vitest";
import {
    CHANNEL_CAPABILITIES,
    createRustServerTransportFactory,
} from "./transport";
import { METHODS } from "./methods";
import type { Payload, TransportFactory } from "./types";

function harness(
    implemented = Object.values(METHODS).map((spec) => spec.method),
) {
    const sockets: {
        send: ReturnType<typeof vi.fn>;
        close: ReturnType<typeof vi.fn>;
        open(): void;
        message(value: unknown, binary?: boolean): void;
        error(): void;
        end(): void;
    }[] = [];
    const factory: TransportFactory = () => {
        let onOpen = () => {};
        let onMessage = (_value: unknown, _binary: boolean) => {};
        let onError = () => {};
        let onClose = () => {};
        const socket = {
            send: vi.fn(),
            close: vi.fn(),
            open: () => onOpen(),
            message: (value: unknown, binary = false) =>
                onMessage(binary ? value : JSON.stringify(value), binary),
            error: () => onError(),
            end: () => onClose(),
        };
        sockets.push(socket);
        return {
            ...socket,
            onOpen: (fn) => {
                onOpen = fn;
                return () => {
                    onOpen = () => {};
                };
            },
            onMessage: (fn) => {
                onMessage = fn;
                return () => {
                    onMessage = () => {};
                };
            },
            onError: (fn) => {
                onError = fn;
                return () => {
                    onError = () => {};
                };
            },
            onClose: (fn) => {
                onClose = fn;
                return () => {
                    onClose = () => {};
                };
            },
        };
    };
    const base = vi.fn(factory);
    const transport = createRustServerTransportFactory(base)({
        url: "ws://127.0.0.1:7316/v1/ws",
        headers: { Authorization: "Bearer test" },
        protocols: ["paseo.bearer.test"],
    });
    const received: Payload[] = [];
    const errors = vi.fn();
    const closed = vi.fn();
    transport.onMessage((value, binary) => {
        if (!binary) received.push(JSON.parse(String(value)));
    });
    transport.onError(errors);
    transport.onClose(closed);
    transport.onOpen(() =>
        transport.send(JSON.stringify({ type: "hello", clientId: "test" })),
    );
    function ready() {
        for (const socket of sockets) socket.open();
        for (const [index, socket] of sockets.entries())
            socket.message({
                type: "server_info",
                info: {
                    server_id: "server",
                    instance_id: "instance",
                    protocol: { major: 1, minor: 0 },
                    implemented_capabilities: implemented,
                },
                negotiated_capabilities: CHANNEL_CAPABILITIES[index],
            });
    }
    function send(message: Payload) {
        transport.send(JSON.stringify({ type: "session", message }));
    }
    function last(channel: number) {
        return JSON.parse(sockets[channel].send.mock.lastCall![0]);
    }
    return {
        transport,
        sockets,
        ready,
        send,
        last,
        received,
        base,
        errors,
        closed,
    };
}

describe("Rust protocol adapter", () => {
    it("maps the scoped pinned surface and stays within Rust's per-connection limits", () => {
        expect(Object.keys(METHODS)).toHaveLength(171);
        expect(
            new Set(Object.values(METHODS).map((spec) => spec.method)).size,
        ).toBe(168);
        for (const capabilities of CHANNEL_CAPABILITIES) {
            expect(capabilities.length).toBeLessThanOrEqual(64);
            expect(
                capabilities.some((name) =>
                    /^(hub|chat|loop|plugin)[./]/.test(name),
                ),
            ).toBe(false);
        }
        expect(
            Object.keys(METHODS).some((name) =>
                /^(hub|chat|loop|plugin)[./]/.test(name),
            ),
        ).toBe(false);
    });

    it("waits for every handshake and keeps credentials out of subprotocols and URLs", () => {
        const h = harness();
        try {
            h.ready();
            expect(h.received).toHaveLength(1);
            expect(h.received[0]).toMatchObject({
                type: "session",
                message: {
                    type: "status",
                    payload: {
                        serverId: "server",
                        features: { directorySync: false },
                    },
                },
            });
            expect(
                h.base.mock.calls.every(
                    ([options]) => options.protocols === undefined,
                ),
            ).toBe(true);
            expect(h.last(0)).toMatchObject({
                type: "hello",
                client_id: "test",
                protocol: { major: 1 },
            });
        } finally {
            h.transport.close();
        }
    });

    it("correlates concurrent replies and sends params without envelope fields", () => {
        const h = harness();
        try {
            h.ready();
            h.send({ type: "project.list.request", requestId: "projects" });
            const first = h.last(1);
            h.send({
                type: "fetch_workspaces_request",
                requestId: "workspaces",
            });
            const second = h.last(1);
            expect(first.params).toEqual({});
            expect(second.method).toBe("workspace.list.request");
            h.sockets[1].message({
                type: "response",
                request_id: second.request_id,
                result: { entries: [] },
            });
            h.sockets[1].message({
                type: "response",
                request_id: first.request_id,
                result: { projects: [] },
            });
            expect(h.received.at(-1)).toMatchObject({
                message: {
                    type: "project.list.response",
                    payload: { requestId: "projects" },
                },
            });
            expect(h.received.at(-2)).toMatchObject({
                message: {
                    type: "fetch_workspaces_response",
                    payload: { requestId: "workspaces" },
                },
            });
        } finally {
            h.transport.close();
        }
    });

    it("releases each subscription on the socket that owns it", () => {
        const h = harness();
        try {
            h.ready();
            h.send({
                type: "workspace.label.list.request",
                requestId: "labels",
                subscribe: {},
            });
            h.sockets[1].message({
                type: "response",
                request_id: h.last(1).request_id,
                result: { subscriptionId: "label-sub", labels: [] },
            });
            h.send({
                type: "subscription.release.request",
                requestId: "release",
                subscriptionId: "label-sub",
            });
            expect(h.last(1)).toMatchObject({
                method: "subscription.release.request",
                params: { subscriptionId: "label-sub" },
            });
        } finally {
            h.transport.close();
        }
    });

    it("delivers server errors as correlated SDK errors and exposes unavailable methods immediately", () => {
        const h = harness(["project.list.request", "connection.ping"]);
        try {
            h.ready();
            h.send({ type: "project.list.request", requestId: "list" });
            h.sockets[1].message({
                type: "error",
                request_id: h.last(1).request_id,
                code: "registry_io",
                message: "Registry failed",
            });
            expect(h.received.at(-1)).toMatchObject({
                message: {
                    type: "rpc_error",
                    payload: { requestId: "list", code: "registry_io" },
                },
            });
            h.send({ type: "schedule/list", requestId: "schedule" });
            expect(h.received.at(-1)).toMatchObject({
                message: {
                    type: "rpc_error",
                    payload: { requestId: "schedule", code: "not_implemented" },
                },
            });
        } finally {
            h.transport.close();
        }
    });

    it("preserves permission identity and voice stream ownership", () => {
        const h = harness();
        try {
            h.ready();
            h.send({
                type: "agent_permission_response",
                requestId: "permission",
                agentId: "agent",
                response: { behavior: "allow" },
            });
            expect(h.last(0).params.requestId).toBe("permission");
            h.send({
                type: "dictation_stream_start",
                dictationId: "dictation",
                format: "audio/wav",
            });
            expect(h.last(0)).toMatchObject({
                type: "event",
                method: "dictation.stream.start",
                params: { dictationId: "dictation" },
            });
            h.sockets[0].message({
                type: "event",
                method: "dictation.stream.ack",
                params: { dictationId: "dictation", ackSeq: -1 },
            });
            expect(h.received.at(-1)).toMatchObject({
                message: { type: "dictation_stream_ack" },
            });
        } finally {
            h.transport.close();
        }
    });

    it("routes terminal/file binary frames without corrupting them", () => {
        const h = harness();
        try {
            h.ready();
            const terminal = new Uint8Array([1, 2, 3]);
            const file = new Uint8Array([0x10, 2, 3]);
            h.transport.send(terminal);
            h.transport.send(file);
            expect(h.sockets[1].send).toHaveBeenLastCalledWith(terminal);
            expect(h.sockets[2].send).toHaveBeenLastCalledWith(file);
        } finally {
            h.transport.close();
        }
    });

    it("translates the SDK liveness ping and cleans every socket on partial failure", () => {
        const h = harness();
        h.ready();
        h.transport.send(JSON.stringify({ type: "ping" }));
        const ping = h.last(0);
        h.sockets[0].message({
            type: "response",
            request_id: ping.request_id,
            result: { nonce: ping.params.nonce },
        });
        expect(h.received.at(-1)).toEqual({ type: "pong" });
        h.sockets[2].end();
        expect(
            h.sockets.every((socket) => socket.close.mock.calls.length === 1),
        ).toBe(true);
        expect(h.closed).toHaveBeenCalledTimes(1);
        expect(() => h.send({ type: "project.list.request" })).toThrow(
            "closed",
        );
    });

    it("rejects mixed server instances and never reports a successful connection", () => {
        const h = harness();
        for (const socket of h.sockets) socket.open();
        for (const index of [0, 1])
            h.sockets[index].message({
                type: "server_info",
                info: {
                    server_id: "server",
                    instance_id: String(index),
                    protocol: { major: 1, minor: 0 },
                    implemented_capabilities: [],
                },
                negotiated_capabilities: [],
            });
        expect(h.received).toHaveLength(0);
        expect(h.errors).toHaveBeenCalledTimes(1);
        expect(
            h.sockets.every((socket) => socket.close.mock.calls.length === 1),
        ).toBe(true);
    });

    it("closes every socket when only part of the handshake completes", () => {
        vi.useFakeTimers();
        const h = harness();
        try {
            h.sockets[0].open();
            vi.advanceTimersByTime(10_001);
            expect(h.errors).toHaveBeenCalledTimes(1);
            expect(h.received).toHaveLength(0);
            expect(
                h.sockets.every(
                    (socket) => socket.close.mock.calls.length === 1,
                ),
            ).toBe(true);
        } finally {
            h.transport.close();
            vi.useRealTimers();
        }
    });

    it("expires pending RPCs and ignores late replies", () => {
        vi.useFakeTimers();
        const h = harness();
        try {
            h.ready();
            h.send({ type: "project.list.request", requestId: "expired" });
            const id = h.last(1).request_id;
            vi.advanceTimersByTime(300_001);
            expect(h.received.at(-1)).toMatchObject({
                message: {
                    type: "rpc_error",
                    payload: { requestId: "expired", code: "timeout" },
                },
            });
            const count = h.received.length;
            h.sockets[1].message({
                type: "response",
                request_id: id,
                result: { projects: [] },
            });
            expect(h.received).toHaveLength(count);
        } finally {
            h.transport.close();
            vi.useRealTimers();
        }
    });

    it("preserves server identity and ownership in lifecycle status events", () => {
        const h = harness();
        try {
            h.ready();
            h.sockets[0].message({
                type: "event",
                method: "status.server_info",
                params: {
                    subscriptionId: "lifecycle",
                    info: { server_id: "server", lifecycle: "draining" },
                },
            });
            expect(h.received.at(-1)).toMatchObject({
                message: {
                    type: "status",
                    payload: {
                        status: "server_info",
                        serverId: "server",
                        subscriptionId: "lifecycle",
                    },
                },
            });
        } finally {
            h.transport.close();
        }
    });
});

describe("Browser host bridge", () => {
    it("preserves server request fields and uses the payload requestId for callbacks", () => {
        const h = harness();
        try {
            h.ready();
            const channel = METHODS["browser.host.register.request"].channel;
            h.send({
                type: "browser.host.register.request",
                requestId: "register",
                hostKind: "desktop",
                supportedCommands: ["list_tabs"],
            });
            const registered = h.last(channel);
            h.sockets[channel].message({
                type: "response",
                request_id: registered.request_id,
                result: { subscriptionId: "host-lease" },
            });
            h.sockets[channel].message({
                type: "event",
                method: "browser.automation.execute.request",
                params: {
                    subscriptionId: "host-lease",
                    requestId: "browser-call",
                    command: { command: "list_tabs", args: {} },
                    workspaceId: "workspace",
                },
            });
            expect(h.received.at(-1)).toEqual({
                type: "session",
                message: {
                    type: "browser.automation.execute.request",
                    subscriptionId: "host-lease",
                    requestId: "browser-call",
                    command: { command: "list_tabs", args: {} },
                    workspaceId: "workspace",
                },
            });
            const payload = {
                requestId: "browser-call",
                ok: true,
                result: { command: "list_tabs", tabs: [] },
            };
            h.send({ type: "browser.automation.execute.response", payload });
            expect(h.last(channel)).toEqual({
                type: "response",
                method: "browser.automation.execute.response",
                request_id: "browser-call",
                params: payload,
            });
            h.send({
                type: "subscription.release.request",
                requestId: "release",
                subscriptionId: "host-lease",
            });
            expect(h.last(channel).method).toBe("subscription.release.request");
        } finally {
            h.transport.close();
        }
    });
});
