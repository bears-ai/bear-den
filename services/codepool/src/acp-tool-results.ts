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

type Waiter = {
    resolve: (payload: AcpToolResultPayload) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
};

export class AcpToolResultRegistry {
    private waiters = new Map<string, Waiter>();

    waitForResult(opts: {
        sessionId: string;
        requestId: string;
        callId: string;
        timeoutMs: number;
        signal?: AbortSignal;
    }): Promise<AcpToolResultPayload> {
        const key = this.key(opts.sessionId, opts.requestId, opts.callId);
        if (this.waiters.has(key)) {
            return Promise.reject(
                new Error(`duplicate ACP tool waiter for ${opts.callId}`),
            );
        }
        return new Promise((resolve, reject) => {
            const cleanup = () => {
                clearTimeout(waiter.timer);
                this.waiters.delete(key);
                opts.signal?.removeEventListener("abort", onAbort);
            };
            const onAbort = () => {
                cleanup();
                reject(new Error(`ACP tool call ${opts.callId} was cancelled`));
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

    deliverResult(sessionId: string, payload: AcpToolResultPayload): boolean {
        const key = this.key(sessionId, payload.request_id, payload.call_id);
        const waiter = this.waiters.get(key);
        if (!waiter) return false;
        waiter.resolve(payload);
        return true;
    }

    pendingCount(): number {
        return this.waiters.size;
    }

    private key(sessionId: string, requestId: string, callId: string): string {
        return `${sessionId}\n${requestId}\n${callId}`;
    }
}

export function normalizeAcpToolResultStatus(value: unknown): AcpToolResultStatus | null {
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
