import type { AnyAgentTool, AgentToolResult } from "@letta-ai/letta-code-sdk";
import { randomUUID } from "node:crypto";
import type { BearChannelEvent } from "./bear-channel.js";

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

export function parseAcpClientTools(capabilities: {
    client_tools?: unknown[];
} | undefined): AcpClientToolDescriptor[] {
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
}): AnyAgentTool[] {
    const used = new Set<string>();
    return opts.descriptors
        .map((descriptor) => toAgentTool(descriptor, opts.getContext, opts.emit, used))
        .filter((tool): tool is AnyAgentTool => tool !== null);
}

function toAgentTool(
    descriptor: AcpClientToolDescriptor,
    getContext: () => AcpClientToolRuntimeContext,
    emit: (event: BearChannelEvent) => void,
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
        typeof descriptor.description === "string" && descriptor.description.trim()
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
        execute: async (toolCallId: string, args: unknown, signal?: AbortSignal) => {
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
            // MVP bridge: expose the request in the stream, then return a structured
            // diagnostic result so the model knows the user/client must provide the
            // observed content through the ACP transcript. The durable continuation
            // endpoint can replace this placeholder with a true wait in the next slice.
            if (signal?.aborted) {
                return jsonTextResult({
                    status: "cancelled",
                    call_id: callId,
                    tool_name: name,
                });
            }
            return jsonTextResult({
                status: "requested",
                call_id: callId,
                tool_name: name,
                message:
                    "ACP client tool request was emitted to the active editor session. Await user/client response in the ACP transcript.",
            });
        },
    };
}

function isJsonSchemaObject(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value);
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
