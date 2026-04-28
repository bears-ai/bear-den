import { execFile as execFileCb } from "node:child_process";
import { readFileSync, accessSync, constants as FsConstants } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { promisify } from "node:util";
import express from "express";
import type { ConversationSessionPool } from "./pool.js";
import { sdkMessageToSseDataLine } from "./sse.js";
import { handleOpenAIChatCompletions } from "./openai.js";
import type { ChannelListenerRegistry } from "./channel-listeners.js";
import {
    recordConversationMessagesRequest,
    recordStreamFinishedEmptyFallback,
    recordStreamFinishedError,
    recordStreamFinishedOk,
    recordStreamFinishedUpstreamError,
    renderPrometheusText,
} from "./metrics.js";
import {
    parseBearRuntimePlan,
    type BearRuntimePlan,
} from "./provisioning/types.js";

const packageJson = JSON.parse(
    readFileSync(new URL("../package.json", import.meta.url), "utf8"),
) as { version?: string };

const execFile = promisify(execFileCb);

function memfsRemoteUrlForAgent(agentId: string): {
    url: string;
    source: "LETTA_MEMFS_SERVICE_URL" | "LETTA_BASE_URL";
} {
    const memfsBase = process.env.LETTA_MEMFS_SERVICE_URL?.trim().replace(
        /\/+$/,
        "",
    );
    if (memfsBase) {
        return {
            url: `${memfsBase}/git/${agentId}/state.git`,
            source: "LETTA_MEMFS_SERVICE_URL",
        };
    }
    const lettaBase =
        process.env.LETTA_BASE_URL?.trim().replace(/\/+$/, "") ?? "";
    return {
        url: `${lettaBase}/v1/git/${agentId}/state.git`,
        source: "LETTA_BASE_URL",
    };
}

function gitAuthArgs(): string[] {
    const token = process.env.LETTA_API_KEY?.trim();
    if (!token) return [];
    const basic = Buffer.from(`letta:${token}`).toString("base64");
    return ["-c", `http.extraHeader=Authorization: Basic ${basic}`];
}

function memoryBlockLabelForMarkdownPath(filePath: string): string | null {
    if (!filePath.endsWith(".md")) return null;
    if (filePath.startsWith("skills/")) {
        const parts = filePath.split("/");
        if (parts.length === 3 && parts[2] === "SKILL.md") {
            return `skills/${parts[1]}`;
        }
        return null;
    }
    return filePath.slice(0, -".md".length);
}

async function fetchLettaBlockLabels(agentId: string): Promise<{
    ok: boolean;
    labels: string[];
    error?: string;
    status?: number;
}> {
    const baseUrl =
        process.env.LETTA_BASE_URL?.trim().replace(/\/+$/, "") ?? "";
    if (!baseUrl) {
        return { ok: false, labels: [], error: "LETTA_BASE_URL is not set" };
    }
    const headers: Record<string, string> = { Accept: "application/json" };
    const token = process.env.LETTA_API_KEY?.trim();
    if (token) {
        headers.Authorization = `Bearer ${token}`;
    }
    try {
        const response = await fetch(
            `${baseUrl}/v1/agents/${agentId}/core-memory/blocks`,
            {
                headers,
            },
        );
        if (!response.ok) {
            const text = await response.text().catch(() => "");
            return {
                ok: false,
                labels: [],
                status: response.status,
                error: text || response.statusText,
            };
        }
        const body = (await response.json()) as unknown;
        const items = Array.isArray(body)
            ? body
            : body &&
                typeof body === "object" &&
                Array.isArray((body as { items?: unknown }).items)
              ? (body as { items: unknown[] }).items
              : [];
        const labels = items
            .map((item) =>
                item && typeof item === "object"
                    ? (item as { label?: unknown }).label
                    : null,
            )
            .filter(
                (label): label is string =>
                    typeof label === "string" && label.length > 0,
            )
            .sort();
        return { ok: true, labels };
    } catch (e) {
        return {
            ok: false,
            labels: [],
            error: e instanceof Error ? e.message : String(e),
        };
    }
}

async function runGit(
    args: string[],
    cwd?: string,
): Promise<{
    ok: boolean;
    stdout: string;
    stderr: string;
    error?: string;
}> {
    try {
        const result = await execFile("git", args, {
            cwd,
            timeout: 60_000,
            maxBuffer: 10 * 1024 * 1024,
        });
        return {
            ok: true,
            stdout: result.stdout?.toString() ?? "",
            stderr: result.stderr?.toString() ?? "",
        };
    } catch (e) {
        const err = e as Error & {
            stdout?: Buffer | string;
            stderr?: Buffer | string;
        };
        return {
            ok: false,
            stdout: err.stdout?.toString() ?? "",
            stderr: err.stderr?.toString() ?? "",
            error: err.message,
        };
    }
}

export type ServerContext = {
    pool: ConversationSessionPool;
    channelListeners: ChannelListenerRegistry;
    internalToken: string;
};

function authMiddleware(internalToken: string) {
    return (
        req: express.Request,
        res: express.Response,
        next: express.NextFunction,
    ) => {
        if (!internalToken) return next();
        const h = req.headers.authorization;
        const ok = h === `Bearer ${internalToken}` || h === internalToken;
        if (!ok) {
            res.status(401).json({ error: "unauthorized" });
            return;
        }
        next();
    };
}

/**
 * POST /v1/conversations/:conversationId/messages — Letta-compatible streaming (SSE).
 */
export function attachRoutes(
    app: express.Application,
    ctx: ServerContext,
): void {
    const guard = authMiddleware(ctx.internalToken);

    app.get("/health", (_req, res) => {
        const lettaCliHome = join(homedir(), ".letta");
        let letta_cli_home_writable: boolean | undefined;
        try {
            accessSync(lettaCliHome, FsConstants.R_OK | FsConstants.W_OK);
            letta_cli_home_writable = true;
        } catch {
            letta_cli_home_writable = false;
        }
        res.json({
            ok: true,
            service: "bears-codepool",
            letta_memfs_local: process.env.LETTA_MEMFS_LOCAL ?? null,
            letta_memfs_service_url:
                process.env.LETTA_MEMFS_SERVICE_URL ?? null,
            session_memfs: true,
            letta_cli_home: lettaCliHome,
            letta_cli_home_writable,
        });
    });

    app.get("/version", (_req, res) => {
        const gitSha = process.env.CODEPOOL_GIT_SHA?.trim() || "unknown";
        res.json({
            service: "bears-codepool",
            version: packageJson.version ?? "0.0.0",
            git_sha: gitSha,
        });
    });

    app.get("/internal/pool", guard, (_req, res) => {
        res.json({
            conversationHandlers: ctx.pool.stats(),
            channelListeners: ctx.channelListeners.stats(),
        });
    });

    app.get("/internal/memfs/:agentId/check", guard, async (req, res) => {
        const agentId = (req.params.agentId ?? "").trim();
        if (!/^agent-[A-Za-z0-9_-]+$/.test(agentId)) {
            res.status(400).json({ error: "invalid agent id" });
            return;
        }

        const mode = req.query.mode === "clone" ? "clone" : "ls-remote";
        const remote = memfsRemoteUrlForAgent(agentId);
        const authArgs = gitAuthArgs();
        const lsRemote = await runGit([...authArgs, "ls-remote", remote.url]);
        const refs = lsRemote.stdout
            .split("\n")
            .map((line) => line.trim())
            .filter(Boolean)
            .map((line) => {
                const [sha, ref] = line.split(/\s+/, 2);
                return { sha: sha ?? "", ref: ref ?? "" };
            });

        const body: Record<string, unknown> = {
            ok: lsRemote.ok,
            mode,
            agent_id: agentId,
            remote_url: remote.url,
            remote_url_source: remote.source,
            ls_remote: {
                ok: lsRemote.ok,
                refs,
                stderr: lsRemote.stderr,
                error: lsRemote.error ?? null,
            },
        };

        if (mode === "clone" && lsRemote.ok) {
            const tempRoot = await mkdtemp(
                join(homedir(), ".letta", "memfs-check-"),
            );
            const checkout = join(tempRoot, "checkout");
            try {
                const clone = await runGit([
                    ...authArgs,
                    "clone",
                    remote.url,
                    checkout,
                ]);
                const files = clone.ok
                    ? await runGit([
                          "-C",
                          checkout,
                          "ls-tree",
                          "-r",
                          "--name-only",
                          "HEAD",
                      ])
                    : {
                          ok: false,
                          stdout: "",
                          stderr: "",
                          error: "clone failed",
                      };
                const filePaths = files.ok
                    ? files.stdout.split("\n").filter((line) => line.trim())
                    : [];
                const expectedLabels = filePaths
                    .map(memoryBlockLabelForMarkdownPath)
                    .filter((label): label is string => label !== null)
                    .sort();
                const lettaBlocks = await fetchLettaBlockLabels(agentId);
                const blockLabels = new Set(lettaBlocks.labels);
                const expectedLabelSet = new Set(expectedLabels);
                const missingBlocks = expectedLabels.filter(
                    (label) => !blockLabels.has(label),
                );
                const extraBlocks = lettaBlocks.labels.filter(
                    (label) => !expectedLabelSet.has(label),
                );
                body.ok =
                    clone.ok &&
                    files.ok &&
                    (!lettaBlocks.ok || missingBlocks.length === 0);
                body.clone = {
                    ok: clone.ok,
                    stderr: clone.stderr,
                    error: clone.error ?? null,
                    file_count: filePaths.length,
                    files: filePaths.slice(0, 50),
                };
                body.letta_block_cache = {
                    checked: true,
                    ok: lettaBlocks.ok,
                    status: lettaBlocks.status ?? null,
                    error: lettaBlocks.error ?? null,
                    expected_from_markdown_count: expectedLabels.length,
                    block_count: lettaBlocks.labels.length,
                    missing_blocks: missingBlocks.slice(0, 50),
                    extra_blocks: extraBlocks.slice(0, 50),
                    warning:
                        missingBlocks.length > 0
                            ? "Git repo contains markdown memory files that are not present in Letta's block cache. Route pushes through Letta /v1/git or run a server-side git-to-block sync."
                            : null,
                };
            } finally {
                await rm(tempRoot, { recursive: true, force: true });
            }
        }

        res.status(body.ok ? 200 : 502).json(body);
    });

    app.get("/metrics", (_req, res) => {
        res.type("text/plain; version=0.0.4; charset=utf-8");
        res.send(renderPrometheusText());
    });

    app.post(
        "/v1/conversations/:conversationId/messages",
        express.json({ limit: "2mb" }),
        guard,
        async (req, res) => {
            const conversationId = req.params.conversationId ?? "";
            const body = req.body as {
                messages?: Array<{ role?: string; content?: unknown }>;
                streaming?: boolean;
                agent_id?: string;
                bear_id?: string;
                runtime_plan?: unknown;
            };
            const agentId = (body.agent_id as string | undefined)?.trim();
            if (!agentId) {
                res.status(400).json({ error: "agent_id is required" });
                return;
            }
            const bearId = (body.bear_id as string | undefined)?.trim();
            if (!bearId) {
                res.status(400).json({ error: "bear_id is required" });
                return;
            }
            const plan = parseBearRuntimePlan(body.runtime_plan);
            const userMsg = (body.messages ?? [])
                .filter((m) => m.role === "user")
                .map((m) =>
                    typeof m.content === "string"
                        ? m.content
                        : JSON.stringify(m.content),
                )
                .pop();
            if (!userMsg?.trim()) {
                res.status(400).json({ error: "user message required" });
                return;
            }

            res.status(200);
            res.setHeader("Content-Type", "text/event-stream; charset=utf-8");
            res.setHeader("Cache-Control", "no-cache");
            res.setHeader("Connection", "keep-alive");

            const rawReqId = req.headers["x-request-id"];
            const requestId =
                typeof rawReqId === "string" && rawReqId.trim()
                    ? rawReqId.trim()
                    : randomUUID();
            res.setHeader("X-Request-Id", requestId);

            recordConversationMessagesRequest();
            const t0 = Date.now();
            console.log(
                JSON.stringify({
                    event: "conversation_messages_start",
                    service: "bears-codepool",
                    request_id: requestId,
                    conversation_id: conversationId,
                    agent_id: agentId,
                    bear_id: bearId,
                }),
            );

            let hadAssistantOrReasoning = false;
            let sawUpstreamErrorMessage = false;
            let sseDataLines = 0;

            try {
                for await (const msg of ctx.pool.streamUserMessage(
                    agentId,
                    conversationId,
                    userMsg.trim(),
                    { bearId, plan },
                )) {
                    const line = sdkMessageToSseDataLine(msg);
                    if (line) {
                        sseDataLines += 1;
                        res.write(`data: ${line}\n\n`);
                        try {
                            const parsed = JSON.parse(line) as {
                                message_type?: string;
                            };
                            const mt = parsed.message_type;
                            if (
                                mt === "assistant_message" ||
                                mt === "reasoning_message"
                            ) {
                                hadAssistantOrReasoning = true;
                            } else if (mt === "error_message") {
                                sawUpstreamErrorMessage = true;
                            }
                        } catch {
                            /* ignore */
                        }
                    }
                }
                let outcome: "ok" | "upstream_error" | "empty_fallback";
                if (hadAssistantOrReasoning) {
                    recordStreamFinishedOk();
                    outcome = "ok";
                } else if (sawUpstreamErrorMessage) {
                    recordStreamFinishedUpstreamError();
                    outcome = "upstream_error";
                } else {
                    recordStreamFinishedEmptyFallback();
                    outcome = "empty_fallback";
                    res.write(
                        `data: ${JSON.stringify({
                            message_type: "error_message",
                            message: "No response from the assistant.",
                            detail: "The stream ended without any assistant output. Check Codepool and Letta logs.",
                            support_ref: requestId,
                        })}\n\n`,
                    );
                }
                res.end();
                const ms = Date.now() - t0;
                console.log(
                    JSON.stringify({
                        event: "conversation_messages_end",
                        service: "bears-codepool",
                        request_id: requestId,
                        outcome,
                        duration_ms: ms,
                        sse_data_lines: sseDataLines,
                    }),
                );
            } catch (e) {
                recordStreamFinishedError();
                const err = e instanceof Error ? e.message : String(e);
                console.log(
                    JSON.stringify({
                        event: "conversation_messages_error",
                        service: "bears-codepool",
                        request_id: requestId,
                        error: err,
                    }),
                );
                res.write(
                    `data: ${JSON.stringify({
                        message_type: "error_message",
                        message: err,
                        support_ref: requestId,
                    })}\n\n`,
                );
                res.end();
            }
        },
    );

    app.post(
        "/v1/chat/completions",
        express.json({ limit: "4mb" }),
        guard,
        (req, res) => {
            void handleOpenAIChatCompletions(req, res, ctx.pool);
        },
    );
}
