/** Structural subset of the Paseo transport API; no SDK runtime dependency. */
export interface Transport {
    send(data: string | Uint8Array | ArrayBuffer): void;
    close(code?: number, reason?: string): void;
    onOpen(handler: () => void): () => void;
    onClose(handler: (event?: unknown) => void): () => void;
    onError(handler: (event?: unknown) => void): () => void;
    onMessage(handler: (data: unknown, binary: boolean) => void): () => void;
}

export type TransportFactory = (options: {
    url: string;
    headers?: Record<string, string>;
    protocols?: string[];
}) => Transport;

export type Payload = Record<string, unknown>;

export function object(value: unknown): Payload {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
        throw new Error("Invalid Rust server message object");
    }
    return value as Payload;
}

export function strings(value: unknown): string[] {
    if (
        !Array.isArray(value) ||
        !value.every((item) => typeof item === "string")
    ) {
        throw new Error("Invalid Rust server capabilities");
    }
    return value;
}
