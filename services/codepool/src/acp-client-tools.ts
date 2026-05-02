import type { AnyAgentTool, AgentToolResult } from "@letta-ai/letta-code-sdk";
import { randomUUID } from "node:crypto";
import type { BearChannelEvent } from "./bear-channel.js";
import type { AcpToolResultRegistry } from "./acp-tool-results.js";

export type AcpClientToolDescriptor = {
    name?: unknown;
    title?: unknown;
    description?: unknown;
    input_schema?: unknown;
    approval_policy?: unknown;
};

export type AcpClientToolRuntimeContext = {
    session_id: string;
    conversation_id: string;
    request_id: string;
};

const DEFAULT_TIMEOUT_MS = 30_000;

export function parseAcpClientTools(
    capabilities:
        | {
              client_tools?: unknown[];
          }
        | undefined,
): AcpClientToolDescriptor[] {
    const raw = capabilities?.client_tools;
    if (!Array.isArray(raw)) return [];
    return raw.filter(
        (item): item is AcpClientToolDescriptor =>
            item !== null && typeof item === "object",
    );
}

export function makeAcpClientExternalTools(opts: {
    descriptors: AcpClientToolDescriptor[];
    getContext: () => AcpClientToolRuntimeContext;
    emit: (event: BearChannelEvent) => void;
    results: AcpToolResultRegistry;
}): AnyAgentTool[] {
    const used = new Set<string>();
    return opts.descriptors
        .map((descriptor) =>
            toAgentTool(
                descriptor,
                opts.getContext,
                opts.emit,
                opts.results,
                used,
            ),
        )
        .filter((tool): tool is AnyAgentTool => tool !== null);
}

function toAgentTool(
    descriptor: AcpClientToolDescriptor,
    getContext: () => AcpClientToolRuntimeContext,
    emit: (event: BearChannelEvent) => void,
    results: AcpToolResultRegistry,
    used: Set<string>,
): AnyAgentTool | null {
    const name = typeof descriptor.name === "string" ? descriptor.name : "";
    if (!name.startsWith("acp_")) return null;
    if (used.has(name)) {
        throw new Error(`ACP client tool collision: ${name}`);
    }
    used.add(name);

    const label =
        typeof descriptor.title === "string" && descriptor.title.trim()
            ? descriptor.title.trim()
            : name;
    const description =
        typeof descriptor.description === "string" &&
        descriptor.description.trim()
            ? descriptor.description.trim()
            : `Invoke ACP client tool ${name}.`;
    const parameters = isJsonSchemaObject(descriptor.input_schema)
        ? descriptor.input_schema
        : {
              type: "object",
              properties: {},
              additionalProperties: true,
          };

    return {
        name,
        label,
        description,
        parameters,
        execute: async (
            toolCallId: string,
            args: unknown,
            signal?: AbortSignal,
        ) => {
            const context = getContext();
            const callId = toolCallId?.trim() || randomUUID();
            emit({
                type: "client_tool_request",
                request_id: context.request_id,
                session_id: context.session_id,
                conversation_id: context.conversation_id,
                call: {
                    id: callId,
                    name,
                    arguments: args ?? {},
                    descriptor,
                    approval_policy:
                        typeof descriptor.approval_policy === "string"
                            ? descriptor.approval_policy
                            : undefined,
                    timeout_ms: DEFAULT_TIMEOUT_MS,
                },
            });
            const payload = await results.waitForResult({
                sessionId: context.session_id,
                requestId: context.request_id,
                callId,
                timeoutMs: DEFAULT_TIMEOUT_MS,
                signal,
                toolName: name,
                conversationId: context.conversation_id,
            });
            if (payload.status === "ok") {
                return jsonTextResult(payload.result ?? {});
            }
            return nonOkToolResult(name, callId, payload);
        },
    };
}

function isJsonSchemaObject(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value);
}

function nonOkToolResult(
    toolName: string,
    callId: string,
    payload: { status: string; error?: unknown },
): AgentToolResult<unknown> {
    const message = nonOkToolMessage(toolName, payload.status, payload.error);
    const details = {
        status: payload.status,
        call_id: callId,
        tool_name: toolName,
        error: payload.error ?? { message },
    };
    return {
        content: [
            {
                type: "text",
                text: message,
            },
        ],
        details,
    };
}

function nonOkToolMessage(
    toolName: string,
    status: string,
    error: unknown,
): string {
    const errorMessage = extractErrorText(error);
    if (status === "timeout") {
        return `The local editor tool ${toolName} timed out before BEARS received a result. This is a tool delivery failure, not evidence that the requested file does not exist. You may retry once, preferably with a smaller or more specific file path.`;
    }
    if (status === "cancelled") {
        return `The local editor tool ${toolName} was cancelled by the ACP client or user before it returned a result.`;
    }
    return `The local editor tool ${toolName} failed before returning a usable result.${
        errorMessage ? ` Error: ${errorMessage}` : ""
    }`;
}

function extractErrorText(error: unknown): string | null {
    if (typeof error === "string") return error;
    if (!error || typeof error !== "object") return null;
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return message;
    try {
        return JSON.stringify(error);
    } catch {
        return null;
    }
}

function jsonTextResult(value: unknown): AgentToolResult<unknown> {
    return {
        content: [
            {
                type: "text",
                text: JSON.stringify(value, null, 2),
            },
        ],
        details: value,
    };
}
