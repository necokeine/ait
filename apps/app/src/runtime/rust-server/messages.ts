import { object, type Payload } from "./types";

export function session(type: string, payload: Payload): Payload {
    return { type: "session", message: { type, payload } };
}

export function rpcError(
    requestId: string,
    requestType: string,
    code: string,
    error: string,
): Payload {
    return session("rpc_error", { requestId, requestType, code, error });
}

// The SDK uses these flags to choose wire shapes, not just to show UI controls.
// Only advertise behavior supported by this adapter AND the Rust implementation.
export function serverInfo(info: Payload, implemented: Set<string>): Payload {
    const has = (method: string) => implemented.has(method);
    return session("status", {
        status: "server_info",
        serverId: info.server_id,
        hostname: null,
        version: null,
        desktopManaged: false,
        features: {
            ownedSubscriptions: has("subscription.release.request"),
            explicitEventSubscriptions: has(
                "session.events.set_subscription.request",
            ),
            providersSnapshot: has("provider.snapshot.get.request"),
            workspaceLabels: has("workspace.label.list.request"),
            agentConfigApply: has("agent.config.apply.request"),
            daemonConfigReload: has("daemon.config.reload.request"),
            daemonStatusRpc: has("daemon.get_status.request"),
            daemonDiagnostics: has("diagnostics.request"),
            skillManagement: has("agent.skills.get_status.request"),
            pushTokenRevocation: has("push.unregister.request"),
            workspaceFileEditing: has("fs.file.write.request"),
            projectAdd: has("project.add.request"),
            projectRemove: has("project.remove.request"),
            workspaceRecovery: has("workspace.recovery.inspect.request"),
            workspaceSetupRun: has("workspace.setup.run.request"),
            workspaceTerminals: has("terminal.list.request"),
            forgeSearch: has("forge.search.request"),
            checkoutRefresh: has("checkout.refresh.request"),
            providerUsageList: has("provider.usage.list.request"),
            agentHistorySearch: has("agent.history.get.request"),
            agentForkContext: has("agent.fork_context.request"),
            agentDetach: has("agent.detach.request"),
            agentTimelinePromptIndex: has(
                "agent.timeline.list_prompts.request",
            ),
            rewind: has("agent.rewind.request"),
            directorySync: false,
            creationLifecycle: false,
            agentRequestReceipts: false,
            workspaceRequestReceipts: false,
            projectedSubagentTimeline: has(
                "agent.provider_subagents.timeline.get.request",
            ),
        },
    });
}

const EVENTS: Readonly<Record<string, string>> = {
    "provider.snapshot.update": "providers_snapshot_update",
    "agent.attention.required": "agent_attention_required",
    "agent.permission.request": "agent_permission_request",
    "agent.permission.resolved": "agent_permission_resolved",
    "agent.stream": "agent_stream",
    "checkout.diff.update": "checkout_diff_update",
    "checkout.status.update": "checkout_status_update",
    "terminal.stream.exit": "terminal_stream_exit",
    "terminal.list.changed": "terminals_changed",
    "terminal.attention.required": "terminal_attention_required",
    "voice.audio.output": "audio_output",
    "voice.input.state": "voice_input_state",
    "voice.transcription.result": "transcription_result",
    "voice.assistant.chunk": "assistant_chunk",
    "voice.error": "error",
    "dictation.stream.ack": "dictation_stream_ack",
    "dictation.stream.partial": "dictation_stream_partial",
    "dictation.stream.final": "dictation_stream_final",
    "dictation.stream.error": "dictation_stream_error",
    "dictation.stream.finish.accepted": "dictation_stream_finish_accepted",
};

export function eventMessage(method: string, params: unknown): Payload {
    const payload = object(params);
    if (method === "browser.automation.execute.request") {
        return { type: "session", message: { type: method, ...payload } };
    }
    if (method.startsWith("status.")) {
        // Session server-info events are already Paseo status payloads.
        return session("status", {
            ...payload,
            status: method.slice("status.".length),
        });
    }
    return session(EVENTS[method] ?? method, payload);
}

export function responseMessage(
    response: string,
    requestId: string,
    result: unknown,
    request: Payload,
): Payload {
    const payload: Payload = { ...object(result), requestId };
    if (response.startsWith("status:")) {
        const status = response.slice("status:".length);
        if (status === "agent_created" && typeof payload.error === "string") {
            return session("status", {
                ...payload,
                status: "agent_create_failed",
            });
        }
        return session("status", {
            ...payload,
            ...(payload.agentId === undefined &&
            typeof request.agentId === "string"
                ? { agentId: request.agentId }
                : {}),
            status,
        });
    }
    return session(response, payload);
}
