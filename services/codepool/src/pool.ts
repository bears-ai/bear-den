import {
    createSession,
    resumeSession,
    type Session,
} from "@letta-ai/letta-code-sdk";
import type { SDKMessage } from "@letta-ai/letta-code-sdk";
import {
    isLikelyLettaCodeMemfsCorruption,
    removeLettaCodeAgentMemoryWorktree,
} from "./memfsLocalRepair.js";
import type {
    BearRuntimePlan,
    BearRuntimeProvisioner,
    EnsureResult,
} from "./provisioning/types.js";
import type { AnyAgentTool } from "@letta-ai/letta-code-sdk";
import { logger } from "./logger.js";

export type PoolKey = string;

export function makePoolKey(agentId: string, conversationId: string): PoolKey {
    return `${agentId}\n${conversationId}`;
}

function resumeTargetFor(agentId: string, conversationId: string): string {
    return conversationId === "default" ? agentId : conversationId;
}

function sessionMethodFor(
    conversationId: string,
): "createSession" | "resumeSession" {
    return isPendingNewConversationId(conversationId)
        ? "createSession"
        : "resumeSession";
}

function isPendingNewConversationId(conversationId: string): boolean {
    return conversationId.startsWith("new-");
}

function isMissingConversationError(err: unknown): boolean {
    const s = err instanceof Error ? err.message : String(err);
    return /Conversation\s+conv-[A-Za-z0-9_-]+\s+not found/i.test(s);
}

function toolsSignature(tools?: AnyAgentTool[]): string {
    return (tools ?? [])
        .map((tool) => tool.name)
        .sort()
        .join(",");
}

type Entry = {
    session: Session;
    lastUsed: number;
    toolSignature: string;
};

export type StreamUserOpts = {
    bearId: string;
    plan: BearRuntimePlan;
    tools?: AnyAgentTool[];
};

export type ConversationPoolStats = {
    kind: "conversation";
    warm: number;
    maxEntries: number;
    ttlSecs: number;
    keys: string[];
};

export class ConversationSessionPool {
    private readonly map = new Map<PoolKey, Entry>();
    private readonly ttlMs: number;
    private readonly maxEntries: number;
    private readonly includePartialMessages: boolean;
    private readonly provisioner: BearRuntimeProvisioner;
    /** One ensure result per bear (shared across conversations). */
    private readonly ensureByBear = new Map<string, EnsureResult>();
    private sweepTimer: ReturnType<typeof setInterval> | undefined;
    /** Exclusive lock per conversation (one active run at a time). */
    private tail = new Map<PoolKey, Promise<void>>();

    constructor(opts: {
        ttlSecs: number;
        maxEntries: number;
        includePartialMessages?: boolean;
        provisioner: BearRuntimeProvisioner;
    }) {
        this.ttlMs = opts.ttlSecs * 1000;
        this.maxEntries = opts.maxEntries;
        this.includePartialMessages = opts.includePartialMessages ?? true;
        this.provisioner = opts.provisioner;
        this.sweepTimer = setInterval(
            () => this.evictIdle(),
            Math.min(60_000, this.ttlMs / 2),
        );
        this.sweepTimer.unref?.();
    }

    stats(): ConversationPoolStats {
        return {
            kind: "conversation",
            warm: this.map.size,
            maxEntries: this.maxEntries,
            ttlSecs: Math.round(this.ttlMs / 1000),
            keys: [...this.map.keys()].map((k) => k.replace("\n", " / ")),
        };
    }

    shutdown(): void {
        if (this.sweepTimer) clearInterval(this.sweepTimer);
        for (const e of this.map.values()) {
            try {
                e.session.close();
            } catch {
                /* ignore */
            }
        }
        this.map.clear();
        this.ensureByBear.clear();
    }

    private async acquireLock(key: PoolKey): Promise<() => void> {
        const prev = this.tail.get(key) ?? Promise.resolve();
        let release!: () => void;
        const done = new Promise<void>((r) => {
            release = r;
        });
        this.tail.set(
            key,
            prev.then(() => done),
        );
        await prev;
        return () => {
            release();
        };
    }

    private evictIdle(): void {
        const now = Date.now();
        for (const [key, entry] of this.map) {
            if (now - entry.lastUsed > this.ttlMs) {
                try {
                    entry.session.close();
                } catch {
                    /* ignore */
                }
                this.map.delete(key);
            }
        }
        while (this.map.size > this.maxEntries) {
            let oldestKey: PoolKey | undefined;
            let oldest = Infinity;
            for (const [key, entry] of this.map) {
                if (entry.lastUsed < oldest) {
                    oldest = entry.lastUsed;
                    oldestKey = key;
                }
            }
            if (!oldestKey) break;
            const e = this.map.get(oldestKey);
            if (e) {
                try {
                    e.session.close();
                } catch {
                    /* ignore */
                }
            }
            this.map.delete(oldestKey);
        }
    }

    private async ensureRuntime(
        bearId: string,
        agentId: string,
        conversationId: string,
        plan: BearRuntimePlan,
    ): Promise<EnsureResult> {
        let e = this.ensureByBear.get(bearId);
        if (!e) {
            e = await this.provisioner.ensure(
                { bearId, agentId, conversationId },
                plan,
            );
            this.ensureByBear.set(bearId, e);
        }
        return e;
    }

    private getOrCreateSession(
        agentId: string,
        conversationId: string,
        ensure: EnsureResult,
        tools?: AnyAgentTool[],
    ): Session {
        const key = makePoolKey(agentId, conversationId);
        const rt = resumeTargetFor(agentId, conversationId);
        let entry = this.map.get(key);
        const now = Date.now();
        const toolSignature = toolsSignature(tools);
        if (entry) {
            if (entry.toolSignature === toolSignature) {
                entry.lastUsed = now;
                return entry.session;
            }
            logger.info(
                "Letta Code session tool set changed; reopening warm session",
                {
                    event: "letta_code_session_tools_changed",
                    agent_id: agentId,
                    conversation_id: conversationId,
                    previous_tool_signature: entry.toolSignature,
                    next_tool_signature: toolSignature,
                },
            );
            try {
                entry.session.close();
            } catch {
                /* ignore */
            }
            this.map.delete(key);
            entry = undefined;
        }
        this.evictIdle();
        const sessionOpts: Parameters<typeof resumeSession>[1] = {
            includePartialMessages: this.includePartialMessages,
            systemInfoReminder: false,
            memfs: true,
        };
        if (tools && tools.length > 0) {
            sessionOpts.tools = tools;
        }
        const cwd = ensure.cwd?.trim();
        if (cwd) {
            sessionOpts.cwd = cwd;
        }
        const method = sessionMethodFor(conversationId);
        logger.info("Letta Code session opened", {
            event: "letta_code_session_open",
            agent_id: agentId,
            conversation_id: conversationId,
            resume_target: rt,
            session_method: method,
            cwd: cwd || null,
        });
        const session =
            method === "createSession"
                ? createSession(agentId, sessionOpts)
                : resumeSession(rt, sessionOpts);
        this.map.set(key, { session, lastUsed: now, toolSignature });
        return session;
    }

    /**
     * Send user text and stream SDK messages (map to SSE in server).
     */
    async *streamUserMessage(
        agentId: string,
        conversationId: string,
        userText: string,
        opts: StreamUserOpts,
    ): AsyncGenerator<SDKMessage, void, unknown> {
        const key = makePoolKey(agentId, conversationId);
        const unlock = await this.acquireLock(key);
        try {
            const ensure = await this.ensureRuntime(
                opts.bearId,
                agentId,
                conversationId,
                opts.plan,
            );
            for (let attempt = 0; attempt < 2; attempt++) {
                try {
                    const session = this.getOrCreateSession(
                        agentId,
                        conversationId,
                        ensure,
                        opts.tools,
                    );
                    await session.send(userText);
                    for await (const msg of session.stream()) {
                        yield msg as SDKMessage;
                    }
                    return;
                } catch (e) {
                    if (attempt === 0 && isLikelyLettaCodeMemfsCorruption(e)) {
                        const ent = this.map.get(key);
                        if (ent) {
                            try {
                                ent.session.close();
                            } catch {
                                /* ignore */
                            }
                            this.map.delete(key);
                        }
                        removeLettaCodeAgentMemoryWorktree(agentId);
                        continue;
                    }
                    if (
                        attempt === 0 &&
                        conversationId !== "default" &&
                        !isPendingNewConversationId(conversationId) &&
                        isMissingConversationError(e)
                    ) {
                        const ent = this.map.get(key);
                        if (ent) {
                            try {
                                ent.session.close();
                            } catch {
                                /* ignore */
                            }
                            this.map.delete(key);
                        }
                        const sessionOpts: Parameters<typeof createSession>[1] =
                            {
                                includePartialMessages:
                                    this.includePartialMessages,
                                systemInfoReminder: false,
                                memfs: true,
                            };
                        if (opts.tools && opts.tools.length > 0) {
                            sessionOpts.tools = opts.tools;
                        }
                        const cwd = ensure.cwd?.trim();
                        if (cwd) {
                            sessionOpts.cwd = cwd;
                        }
                        const session = createSession(agentId, sessionOpts);
                        this.map.set(key, {
                            session,
                            lastUsed: Date.now(),
                            toolSignature: toolsSignature(opts.tools),
                        });
                        await session.send(userText);
                        for await (const msg of session.stream()) {
                            yield msg as SDKMessage;
                        }
                        return;
                    }
                    throw e;
                }
            }
        } finally {
            unlock();
        }
    }
}
