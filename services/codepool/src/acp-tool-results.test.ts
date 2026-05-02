import { strict as assert } from "node:assert";
import { test } from "node:test";
import { AcpToolResultRegistry } from "./acp-tool-results.js";

test("ACP tool result registry delivers waiter payloads", async () => {
    const registry = new AcpToolResultRegistry();
    const wait = registry.waitForResult({
        sessionId: "session-1",
        requestId: "request-1",
        callId: "call-1",
        timeoutMs: 1_000,
    });
    assert.equal(registry.pendingCount(), 1);
    assert.deepEqual(
        registry.deliverResult("session-1", {
            request_id: "request-1",
            call_id: "call-1",
            status: "ok",
            result: { content: "hello" },
        }),
        { delivered: true, reason: "delivered" },
    );
    const payload = await wait;
    assert.equal(payload.status, "ok");
    assert.deepEqual(payload.result, { content: "hello" });
    assert.equal(registry.pendingCount(), 0);
});

test("ACP tool result registry reports undelivered unknown calls", () => {
    const registry = new AcpToolResultRegistry();
    assert.deepEqual(
        registry.deliverResult("session-1", {
            request_id: "request-1",
            call_id: "missing-call",
            status: "ok",
            result: {},
        }),
        { delivered: false, reason: "no_waiter" },
    );
});
