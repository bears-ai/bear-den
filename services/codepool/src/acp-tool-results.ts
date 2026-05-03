export type AcpToolResultStatus = "ok" | "error" | "cancelled" | "timeout";

export type AcpToolResultPayload = {
    conversation_id?: string;
    request_id: string;
    call_id: string;
    tool_name?: string;
    status: AcpToolResultStatus;
    result?: unknown;
    error?: unknown;
};

export type AcpToolWaiterInfo = {
    session_id: string;
    request_id: string;
    call_id: string;
    tool_name?: string;
    conversation_id?: string;
    created_at_ms: number;
    timeout_ms: number;
    age_ms: number;
};

export type AcpToolDeliveryResult = {
    delivered: boolean;
    reason: "delivered" | "no_waiter";
};

export type AcpToolWaiterRemovalResult = {
    removed: boolean;
    reason: "removed" | "no_waiter";
};

type Waiter = {
    resolve: (payload: AcpToolResultPayload) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
    info: Omit<AcpToolWaiterInfo, "age_ms">;
};

export class AcpToolResultRegistry {
    private waiters = new Map<string, Waiter>();

    waitForResult(opts: {
        sessionId: string;
        requestId: string;
        callId: string;
        timeoutMs: number;
        signal?: AbortSignal;
        toolName?: string;
        conversationId?: string;
    }): Promise<AcpToolResultPayload> {
        const key = this.key(opts.sessionId, opts.requestId, opts.callId);
        if (this.waiters.has(key)) {
            throw new Error(`duplicate ACP tool waiter for ${opts.callId}`);
        }
        return new Promise((resolve, reject) => {
            const cleanup = () => {
                clearTimeout(waiter.timer);
                this.waiters.delete(key);
                opts.signal?.removeEventListener("abort", onAbort);
            };
            const cancelledPayload = (
                message: string,
            ): AcpToolResultPayload => ({
                request_id: opts.requestId,
                call_id: opts.callId,
                status: "cancelled",
                error: { message },
            });
            const onAbort = () => {
                cleanup();
                resolve(
                    cancelledPayload(
                        `ACP tool call ${opts.callId} was cancelled`,
                    ),
                );
            };
            const waiter: Waiter = {
                resolve: (payload) => {
                    cleanup();
                    resolve(payload);
                },
                reject: (error) => {
                    cleanup();
                    reject(error);
                },
                info: {
                    session_id: opts.sessionId,
                    request_id: opts.requestId,
                    call_id: opts.callId,
                    tool_name: opts.toolName,
                    conversation_id: opts.conversationId,
                    created_at_ms: Date.now(),
                    timeout_ms: opts.timeoutMs,
                },
                timer: setTimeout(() => {
                    cleanup();
                    resolve({
                        request_id: opts.requestId,
                        call_id: opts.callId,
                        status: "timeout",
                        error: { message: "ACP client tool call timed out" },
                    });
                }, opts.timeoutMs),
            };
            waiter.timer.unref?.();
            this.waiters.set(key, waiter);
            if (opts.signal?.aborted) {
                onAbort();
            } else {
                opts.signal?.addEventListener("abort", onAbort, { once: true });
            }
        });
    }

    deliverResult(
        sessionId: string,
        payload: AcpToolResultPayload,
    ): AcpToolDeliveryResult {
        const key = this.key(sessionId, payload.request_id, payload.call_id);
        const waiter = this.waiters.get(key);
        if (!waiter) return { delivered: false, reason: "no_waiter" };
        waiter.resolve(payload);
        return { delivered: true, reason: "delivered" };
    }

    cancelWaiter(opts: {
        sessionId: string;
        requestId: string;
        callId: string;
        message: string;
    }): AcpToolWaiterRemovalResult {
        const key = this.key(opts.sessionId, opts.requestId, opts.callId);
        const waiter = this.waiters.get(key);
        if (!waiter) return { removed: false, reason: "no_waiter" };
        waiter.resolve({
            request_id: opts.requestId,
            call_id: opts.callId,
            status: "cancelled",
            error: { message: opts.message },
        });
        return { removed: true, reason: "removed" };
    }

    rejectWaiter(opts: {
        sessionId: string;
        requestId: string;
        callId: string;
        error: Error;
    }): AcpToolWaiterRemovalResult {
        const key = this.key(opts.sessionId, opts.requestId, opts.callId);
        const waiter = this.waiters.get(key);
        if (!waiter) return { removed: false, reason: "no_waiter" };
        waiter.reject(opts.error);
        return { removed: true, reason: "removed" };
    }

    listWaiters(): AcpToolWaiterInfo[] {
        const now = Date.now();
        return [...this.waiters.values()].map((waiter) => ({
            ...waiter.info,
            age_ms: now - waiter.info.created_at_ms,
        }));
    }

    pendingCount(): number {
        return this.waiters.size;
    }

    private key(sessionId: string, requestId: string, callId: string): string {
        return `${sessionId}\n${requestId}\n${callId}`;
    }
}

export function normalizeAcpToolResultStatus(
    value: unknown,
): AcpToolResultStatus | null {
    if (
        value === "ok" ||
        value === "error" ||
        value === "cancelled" ||
        value === "timeout"
    ) {
        return value;
    }
    return null;
}
