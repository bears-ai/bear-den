import type { AnyAgentTool, AgentToolResult } from "@letta-ai/letta-code-sdk";
import type { BearChannelRequest } from "./bear-channel.js";

export type DenToolDescriptor = {
    name?: unknown;
    provider_name?: unknown;
    label?: unknown;
    description?: unknown;
    input_schema?: unknown;
};

export type DenToolRuntimeContext = {
    bear_id: string;
    bear_slug?: string;
    letta_agent_id: string;
    user_id: number | string;
    username?: string;
    membership_role?: string | null;
    conversation_id: string;
    session_id: string;
    request_id?: string;
    channel?: {
        family?: string;
        client?: string;
        protocol?: string;
    };
};

const EMPTY_SCHEMA = {
    type: "object",
    properties: {},
    additionalProperties: false,
};

export function parseDenServerTools(
    capabilities: BearChannelRequest["capabilities"],
): DenToolDescriptor[] {
    const raw = capabilities?.server_tools;
    if (!Array.isArray(raw)) return [];
    return raw.filter(
        (item): item is DenToolDescriptor =>
            item !== null && typeof item === "object",
    );
}

export function buildDenToolRuntimeContext(
    body: BearChannelRequest,
    sessionId: string,
    conversationId: string,
    requestId: string,
): DenToolRuntimeContext {
    const bearId = body.bear?.id?.trim() ?? "";
    const agentId = body.bear?.letta_agent_id?.trim() ?? "";
    const userId = body.user?.id ?? "";
    if (!bearId) throw new Error("bear.id is required for Den tools");
    if (!agentId)
        throw new Error("bear.letta_agent_id is required for Den tools");
    if (userId === "" || userId === null || userId === undefined) {
        throw new Error("user.id is required for Den tools");
    }
    return {
        bear_id: bearId,
        bear_slug: body.bear?.slug,
        letta_agent_id: agentId,
        user_id: userId,
        username: body.user?.username,
        membership_role: body.user?.membership_role,
        conversation_id: conversationId,
        session_id: sessionId,
        request_id: requestId,
        channel: {
            family: body.channel?.family,
            client: body.channel?.client,
            protocol: body.channel?.protocol,
        },
    };
}

export function makeDenExternalTools(opts: {
    descriptors: DenToolDescriptor[];
    getContext: () => DenToolRuntimeContext;
}): AnyAgentTool[] {
    const usedProviderNames = new Set<string>();
    return opts.descriptors
        .map((descriptor) =>
            toAgentTool(descriptor, opts.getContext, usedProviderNames),
        )
        .filter((tool): tool is AnyAgentTool => tool !== null);
}

const PROVIDER_TOOL_NAME_PATTERN = /^[a-zA-Z0-9_-]+$/;

/**
 * Fallback for older Den payloads that do not yet include `provider_name`.
 * New Den descriptors should provide an explicit provider/API-safe alias.
 */
export function providerSafeToolName(name: string): string {
    const safe = name.replace(/[^a-zA-Z0-9_-]/g, "_");
    if (safe && PROVIDER_TOOL_NAME_PATTERN.test(safe)) return safe;
    return "den_tool";
}

function providerToolName(
    descriptor: DenToolDescriptor,
    denName: string,
): string {
    if (
        typeof descriptor.provider_name === "string" &&
        PROVIDER_TOOL_NAME_PATTERN.test(descriptor.provider_name)
    ) {
        return descriptor.provider_name;
    }
    return providerSafeToolName(denName);
}

function toAgentTool(
    descriptor: DenToolDescriptor,
    getContext: () => DenToolRuntimeContext,
    usedProviderNames: Set<string>,
): AnyAgentTool | null {
    const denName = typeof descriptor.name === "string" ? descriptor.name : "";
    if (!denName.startsWith("den.")) return null;
    const providerName = providerToolName(descriptor, denName);
    if (usedProviderNames.has(providerName)) {
        throw new Error(
            `Den tool provider-name collision: ${denName} maps to duplicate ${providerName}`,
        );
    }
    usedProviderNames.add(providerName);
    const label =
        typeof descriptor.label === "string" && descriptor.label.trim()
            ? descriptor.label
            : denName;
    const rawDescription =
        typeof descriptor.description === "string" &&
        descriptor.description.trim()
            ? descriptor.description.trim()
            : `Invoke Den server tool ${denName}.`;
    const description = `${rawDescription}\n\nDen internal tool: ${denName}.`;
    const parameters = isJsonSchemaObject(descriptor.input_schema)
        ? descriptor.input_schema
        : EMPTY_SCHEMA;

    return {
        name: providerName,
        label,
        description,
        parameters,
        execute: async (_toolCallId: string, args: unknown) => {
            const result = await invokeDenTool(denName, args, getContext());
            return jsonTextResult(result);
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

async function invokeDenTool(
    toolName: string,
    args: unknown,
    context: DenToolRuntimeContext,
): Promise<unknown> {
    const baseUrl = denInternalBaseUrl();
    const response = await fetch(`${baseUrl}/internal/den-tools/invoke`, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
            ...authHeaders(),
        },
        body: JSON.stringify({
            tool_name: toolName,
            arguments: args ?? {},
            context,
        }),
    });
    const text = await response.text();
    let body: unknown = text;
    try {
        body = JSON.parse(text);
    } catch {
        // keep raw text
    }
    if (!response.ok) {
        const message = extractErrorMessage(body) || response.statusText;
        throw new Error(
            `Den tool ${toolName} failed (${response.status}): ${message}`,
        );
    }
    if (
        body &&
        typeof body === "object" &&
        "result" in body &&
        (body as { ok?: unknown }).ok === true
    ) {
        return (body as { result: unknown }).result;
    }
    return body;
}

function denInternalBaseUrl(): string {
    const configured = process.env.DEN_INTERNAL_BASE_URL?.trim().replace(
        /\/+$/,
        "",
    );
    if (configured) return configured;
    return process.env.NODE_ENV === "production"
        ? "http://bears-den:3001"
        : "http://127.0.0.1:3001";
}

function authHeaders(): Record<string, string> {
    const token = process.env.CODEPOOL_INTERNAL_TOKEN?.trim() ?? "";
    if (!token) return {};
    return { Authorization: `Bearer ${token}` };
}

function extractErrorMessage(body: unknown): string | null {
    if (!body || typeof body !== "object") return null;
    const err = (body as { error?: unknown }).error;
    if (err && typeof err === "object") {
        const message = (err as { message?: unknown }).message;
        if (typeof message === "string") return message;
    }
    return null;
}

if (process.env.NODE_ENV === "test") {
    const assertProviderToolNames = () => {
        const samples = [
            ["den.bear.get_self", "den_bear_get_self"],
            ["den.user/get current", "den_user_get_current"],
        ] as const;
        for (const [input, expected] of samples) {
            if (providerSafeToolName(input) !== expected) {
                throw new Error(
                    `providerSafeToolName(${input}) expected ${expected}, got ${providerSafeToolName(input)}`,
                );
            }
        }
    };
    assertProviderToolNames();
}
