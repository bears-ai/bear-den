import { strict as assert } from "node:assert";
import { test } from "node:test";
import { AcpToolResultRegistry } from "./acp-tool-results.js";
import { makeAcpClientExternalTools } from "./acp-client-tools.js";

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

test("ACP tool result registry resolves cancellation as payload", async () => {
    const registry = new AcpToolResultRegistry();
    const controller = new AbortController();
    const wait = registry.waitForResult({
        sessionId: "session-1",
        requestId: "request-1",
        callId: "call-1",
        timeoutMs: 1_000,
        signal: controller.signal,
    });
    controller.abort();
    const payload = await wait;
    assert.equal(payload.status, "cancelled");
    assert.equal(registry.pendingCount(), 0);
});

test("ACP client tool waiter is registered before request emission", async () => {
    const registry = new AcpToolResultRegistry();
    const tools = makeAcpClientExternalTools({
        descriptors: [
            {
                name: "acp_test_tool",
                input_schema: {
                    type: "object",
                    properties: {},
                },
            },
        ],
        getContext: () => ({
            session_id: "session-1",
            conversation_id: "conversation-1",
            request_id: "request-1",
        }),
        emit: (event) => {
            assert.equal(event.type, "client_tool_request");
            const delivery = registry.deliverResult("session-1", {
                request_id: "request-1",
                call_id: event.call.id,
                status: "ok",
                result: { content: "fast result" },
            });
            assert.deepEqual(delivery, {
                delivered: true,
                reason: "delivered",
            });
        },
        results: registry,
    });

    assert.equal(tools.length, 1);
    const result = await tools[0].execute("call-1", {});
    assert.deepEqual(result.details, { content: "fast result" });
    assert.equal(registry.pendingCount(), 0);
});
