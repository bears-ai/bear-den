import type { Request, Response } from "express";
import type { ConversationSessionPool } from "./pool.js";
import { sdkMessageToSseDataLine } from "./sse.js";
import { parseBearRuntimePlan } from "./provisioning/types.js";

/**
 * Minimal OpenAI-compatible chat completions for OpenWebUI.
 * Uses `user` + optional `conversation_id` in `metadata` to map to the pool (role agent id from metadata.role_agent_id).
 */
export async function handleOpenAIChatCompletions(
    req: Request,
    res: Response,
    pool: ConversationSessionPool,
): Promise<void> {
    const body = req.body as {
        model?: string;
        messages?: Array<{ role: string; content: string | null }>;
        stream?: boolean;
        metadata?: {
            role_agent_id?: string;
            conversation_id?: string;
            bear_id?: string;
            runtime_plan?: unknown;
        };
        user?: string;
    };

    const agentId = body.metadata?.role_agent_id?.trim();
    if (!agentId) {
        res.status(400).json({
            error: {
                message:
                    "metadata.role_agent_id is required (Letta agent id for the selected bear role)",
                type: "invalid_request_error",
            },
        });
        return;
    }

    const conversationId = body.metadata?.conversation_id?.trim() || "default";

    const bearId = body.metadata?.bear_id?.trim() || `role-agent-${agentId}`;
    const plan = parseBearRuntimePlan(body.metadata?.runtime_plan);

    const userText = [...(body.messages ?? [])]
        .reverse()
        .find((m) => m.role === "user");
    const text =
        typeof userText?.content === "string" ? userText.content.trim() : "";
    if (!text) {
        res.status(400).json({
            error: {
                message: "No user message",
                type: "invalid_request_error",
            },
        });
        return;
    }

    if (body.stream) {
        res.status(200);
        res.setHeader("Content-Type", "text/event-stream; charset=utf-8");
        res.setHeader("Cache-Control", "no-cache");
        res.setHeader("Connection", "keep-alive");
        const id = `chatcmpl-${Date.now()}`;
        try {
            for await (const msg of pool.streamUserMessage(
                agentId,
                conversationId,
                text,
                { bearId, plan },
            )) {
                const line = sdkMessageToSseDataLine(msg);
                if (!line) continue;
                const inner = JSON.parse(line) as { content?: string };
                const chunk = {
                    id,
                    object: "chat.completion.chunk",
                    choices: [
                        {
                            index: 0,
                            delta: { content: inner.content ?? "" },
                            finish_reason: null,
                        },
                    ],
                };
                res.write(`data: ${JSON.stringify(chunk)}\n\n`);
            }
            res.write(`data: [DONE]\n\n`);
            res.end();
        } catch (e) {
            const err = e instanceof Error ? e.message : String(e);
            res.write(
                `data: ${JSON.stringify({ error: { message: err } })}\n\n`,
            );
            res.end();
        }
        return;
    }

    const parts: string[] = [];
    try {
        for await (const msg of pool.streamUserMessage(
            agentId,
            conversationId,
            text,
            { bearId, plan },
        )) {
            const line = sdkMessageToSseDataLine(msg);
            if (!line) continue;
            const o = JSON.parse(line) as {
                message_type?: string;
                content?: string;
            };
            if (o.message_type === "assistant_message" && o.content)
                parts.push(o.content);
        }
        res.json({
            id: `chatcmpl-${Date.now()}`,
            object: "chat.completion",
            choices: [
                {
                    index: 0,
                    message: { role: "assistant", content: parts.join("") },
                    finish_reason: "stop",
                },
            ],
        });
    } catch (e) {
        const err = e instanceof Error ? e.message : String(e);
        res.status(500).json({
            error: { message: err, type: "server_error" },
        });
    }
}
