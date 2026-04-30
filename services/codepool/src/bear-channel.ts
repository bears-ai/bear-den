import type { SDKMessage } from "@letta-ai/letta-code-sdk";
import { extractStreamTextDelta } from "@letta-ai/letta-code-sdk";
import {
    parseBearRuntimePlan,
    type BearRuntimePlan,
} from "./provisioning/types.js";

export type BearChannelFamily =
    | "browser_chat"
    | "coding_workspace"
    | "team_chat"
    | "automation"
    | string;

export type BearChannelClient =
    | "den_web"
    | "zed"
    | "opencode"
    | "slack"
    | "routine"
    | string;

export type BearChannelProtocol =
    | "den_chat"
    | "agent_client_protocol"
    | "openai_compatible"
    | "slack_events"
    | string;

export type BearChannelRequest = {
    session_id?: string;
    conversation_id?: string;
    bear?: {
        id?: string;
        slug?: string;
        name?: string;
        letta_agent_id?: string;
    };
    user?: {
        id?: number | string;
        username?: string;
        membership_role?: string | null;
    };
    channel?: {
        family?: BearChannelFamily;
        client?: BearChannelClient;
        protocol?: BearChannelProtocol;
    };
    message?: {
        type?: "text" | string;
        content?: unknown;
    };
    capabilities?: {
        server_tools?: unknown[];
        client_tools?: unknown[];
        supports_cancellation?: boolean;
        supports_rich_events?: boolean;
    };
    runtime_plan?: unknown;
    request_id?: string;
};

export type BearChannelEvent =
    | { type: "assistant_delta"; text: string; id?: string }
    | { type: "reasoning_delta"; text: string; id?: string }
    | { type: "server_tool_started"; tool: string; label?: string }
    | {
          type: "server_tool_finished";
          tool: string;
          label?: string;
          summary?: string;
      }
    | {
          type: "client_tool_request";
          call: BearChannelToolCall;
          request_id?: string;
          session_id?: string;
          conversation_id?: string;
      }
    | { type: "subagent_started"; name: string; label?: string }
    | {
          type: "subagent_finished";
          name: string;
          label?: string;
          summary?: string;
      }
    | { type: "memory_update_recorded"; target?: string; summary?: string }
    | {
          type: "error";
          message: string;
          detail?: string;
          error_type?: string;
          request_id?: string;
          context?: BearChannelErrorContext;
      }
    | { type: "conversation_resolved"; conversation_id: string }
    | {
          type: "done";
          outcome?: "ok" | "upstream_error" | "empty_fallback" | "cancelled";
      };

export type BearChannelToolCall = {
    id: string;
    name: string;
    arguments: unknown;
    descriptor?: unknown;
    approval_policy?: string;
    timeout_ms?: number;
};

export type BearChannelErrorContext = {
    session_id: string;
    conversation_id: string;
    agent_id: string;
    bear_id: string;
    resume_target: string;
    session_method: "createSession" | "resumeSession";
    sdk_message_count?: number;
    sdk_message_types?: Record<string, number>;
    sse_data_lines?: number;
    unmapped_stream_event_samples?: unknown[];
    upstream_error?: unknown;
};

export type ParsedBearChannelRequest = {
    conversationId: string;
    agentId: string;
    bearId: string;
    userText: string;
    plan: BearRuntimePlan;
};

export function runtimeErrorContext(
    sessionId: string,
    conversationId: string,
    agentId: string,
    bearId: string,
): BearChannelErrorContext {
    const pendingNew = conversationId.startsWith("new-");
    return {
        session_id: sessionId,
        conversation_id: conversationId,
        agent_id: agentId,
        bear_id: bearId,
        resume_target: conversationId === "default" ? agentId : conversationId,
        session_method: pendingNew ? "createSession" : "resumeSession",
    };
}

export function parseBearChannelRequest(
    body: BearChannelRequest,
): ParsedBearChannelRequest {
    const conversationId = body.conversation_id?.trim() || "default";
    const agentId = body.bear?.letta_agent_id?.trim();
    if (!agentId) {
        throw new Error("bear.letta_agent_id is required");
    }

    const bearId = body.bear?.id?.trim();
    if (!bearId) {
        throw new Error("bear.id is required");
    }

    if (body.message?.type && body.message.type !== "text") {
        throw new Error(
            "only text bear_channel messages are currently supported",
        );
    }

    const content = body.message?.content;
    const userText = typeof content === "string" ? content.trim() : "";
    if (!userText) {
        throw new Error("message.content is required");
    }

    return {
        conversationId,
        agentId,
        bearId,
        userText,
        plan: parseBearRuntimePlan(body.runtime_plan),
    };
}

export function summarizeSdkMessageForDiagnostics(msg: SDKMessage): unknown {
    if (msg.type !== "stream_event") {
        return { type: msg.type };
    }
    const ev = msg.event as Record<string, unknown>;
    return {
        type: msg.type,
        event_keys: Object.keys(ev).sort(),
        message_type:
            typeof ev.message_type === "string" ? ev.message_type : undefined,
        event_type: typeof ev.type === "string" ? ev.type : undefined,
        name: typeof ev.name === "string" ? ev.name : undefined,
        role: typeof ev.role === "string" ? ev.role : undefined,
        has_content: typeof ev.content === "string" && ev.content.length > 0,
        has_delta: typeof ev.delta === "object" && ev.delta !== null,
        preview:
            typeof ev.content === "string"
                ? ev.content.slice(0, 160)
                : JSON.stringify(ev).slice(0, 240),
    };
}

function getStringField(
    obj: Record<string, unknown>,
    keys: string[],
): string | undefined {
    for (const key of keys) {
        const value = obj[key];
        if (typeof value === "string" && value.trim()) return value;
    }
    return undefined;
}

function findNestedError(value: unknown, depth = 0): unknown {
    if (!value || typeof value !== "object" || depth > 4) return undefined;
    const obj = value as Record<string, unknown>;
    if (obj.error && typeof obj.error === "object") return obj.error;
    for (const key of ["data", "response", "body", "detail", "cause"]) {
        const found = findNestedError(obj[key], depth + 1);
        if (found) return found;
    }
    return undefined;
}

export function extractUpstreamErrorSummary(value: unknown): {
    message?: string;
    detail?: string;
    error_type?: string;
    param?: string;
    code?: string;
} | null {
    const err = findNestedError(value) ?? value;
    if (!err || typeof err !== "object") return null;
    const obj = err as Record<string, unknown>;
    const message = getStringField(obj, ["message", "error", "detail"]);
    const detail = getStringField(obj, ["detail", "body", "response"]);
    const error_type = getStringField(obj, ["type", "error_type"]);
    const param = getStringField(obj, ["param"]);
    const code = getStringField(obj, ["code"]);
    if (!message && !detail && !error_type && !param && !code) return null;
    return { message, detail, error_type, param, code };
}

function llmApiErrorDetail(ev: Record<string, unknown>): string {
    const summary = extractUpstreamErrorSummary(ev);
    const parts = [
        "Letta Code emitted stop_reason=llm_api_error before any assistant output.",
    ];
    if (summary?.message) parts.push(summary.message);
    if (summary?.error_type) parts.push(`type=${summary.error_type}`);
    if (summary?.param) parts.push(`param=${summary.param}`);
    if (summary?.code) parts.push(`code=${summary.code}`);
    if (summary?.detail && summary.detail !== summary.message) {
        parts.push(summary.detail);
    }
    if (!summary) {
        parts.push(
            "Check Letta, Bifrost, and model provider logs/configuration for the underlying model API failure.",
        );
    }
    return parts.join("\n");
}

export function sdkMessageToBearChannelEvents(
    msg: SDKMessage,
): BearChannelEvent[] {
    switch (msg.type) {
        case "init":
            return [
                {
                    type: "conversation_resolved",
                    conversation_id: msg.conversationId,
                },
            ];
        case "result":
        case "retry":
            return [];
        case "tool_call":
            return [
                {
                    type: "server_tool_started",
                    tool: String(
                        (msg as unknown as { name?: unknown }).name ?? "tool",
                    ),
                },
            ];
        case "tool_result":
            return [
                {
                    type: "server_tool_finished",
                    tool: String(
                        (msg as unknown as { name?: unknown }).name ?? "tool",
                    ),
                },
            ];
        case "assistant":
            return [
                { type: "assistant_delta", text: msg.content, id: msg.uuid },
            ];
        case "reasoning":
            return [
                { type: "reasoning_delta", text: msg.content, id: msg.uuid },
            ];
        case "stream_event": {
            const ev = msg.event as Record<string, unknown>;
            if (typeof ev.message_type === "string") {
                if (ev.message_type === "assistant_message") {
                    return [
                        {
                            type: "assistant_delta",
                            text:
                                typeof ev.content === "string"
                                    ? ev.content
                                    : "",
                            id: msg.uuid,
                        },
                    ];
                }
                if (ev.message_type === "reasoning_message") {
                    const text =
                        typeof ev.reasoning === "string"
                            ? ev.reasoning
                            : typeof ev.content === "string"
                              ? ev.content
                              : "";
                    return [{ type: "reasoning_delta", text, id: msg.uuid }];
                }
                if (ev.message_type === "error_message") {
                    return [
                        {
                            type: "error",
                            message:
                                typeof ev.message === "string"
                                    ? ev.message
                                    : "Upstream error",
                            detail:
                                typeof ev.detail === "string"
                                    ? ev.detail
                                    : undefined,
                            error_type:
                                typeof ev.error_type === "string"
                                    ? ev.error_type
                                    : undefined,
                        },
                    ];
                }
                if (ev.message_type === "stop_reason") {
                    const stopReason =
                        typeof ev.stop_reason === "string"
                            ? ev.stop_reason
                            : "unknown";
                    if (stopReason === "llm_api_error") {
                        return [
                            {
                                type: "error",
                                message:
                                    "Letta stopped because the LLM API returned an error.",
                                detail: llmApiErrorDetail(ev),
                                error_type: stopReason,
                            },
                        ];
                    }
                    if (stopReason && stopReason !== "end_turn") {
                        return [
                            {
                                type: "error",
                                message: `Letta stopped before producing assistant output: ${stopReason}`,
                                error_type: stopReason,
                            },
                        ];
                    }
                    return [];
                }
            }

            const delta = extractStreamTextDelta(msg.event);
            if (!delta) return [];
            return [
                delta.kind === "assistant"
                    ? {
                          type: "assistant_delta",
                          text: delta.text,
                          id: msg.uuid,
                      }
                    : {
                          type: "reasoning_delta",
                          text: delta.text,
                          id: msg.uuid,
                      },
            ];
        }
        case "error":
            return [
                {
                    type: "error",
                    message: msg.message,
                    detail: msg.errorDetail,
                    error_type: msg.errorCode,
                },
            ];
        default:
            return [];
    }
}

export function bearChannelEventToSseDataLine(event: BearChannelEvent): string {
    return JSON.stringify(event);
}
