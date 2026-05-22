import test from "node:test";
import assert from "node:assert/strict";
import {
    isCancelledStopReason,
    sdkMessageToBearChannelEvents,
    type BearChannelEvent,
} from "./bear-channel.js";

test("requires_approval stop reason is treated as recoverable", () => {
    const events = sdkMessageToBearChannelEvents({
        type: "stream_event",
        event: {
            message_type: "stop_reason",
            stop_reason: "requires_approval",
        },
        uuid: "msg-1",
    } as never);

    assert.deepEqual(events, []);
});

test("non-terminal stop reasons still surface as upstream errors", () => {
    const events = sdkMessageToBearChannelEvents({
        type: "stream_event",
        event: {
            message_type: "stop_reason",
            stop_reason: "max_steps",
        },
        uuid: "msg-2",
    } as never);

    assert.equal(events.length, 1);
    assert.equal(events[0]?.type, "error");
});

test("cancelled stop reason is not surfaced as upstream error", () => {
    const events = sdkMessageToBearChannelEvents({
        type: "stream_event",
        event: {
            message_type: "stop_reason",
            stop_reason: "cancelled",
        },
        uuid: "msg-3",
    } as never);

    assert.deepEqual(events, []);
});

test("canceled stop reason alias is recognized", () => {
    assert.equal(isCancelledStopReason("canceled"), true);
    assert.equal(isCancelledStopReason("cancelled"), true);
    assert.equal(isCancelledStopReason("end_turn"), false);
});

test("error_message passes through typed terminal metadata", () => {
    const events = sdkMessageToBearChannelEvents({
        type: "stream_event",
        event: {
            message_type: "error_message",
            message: "No response from the assistant.",
            detail: "empty stream",
            terminal: {
                outcome: "empty_fallback",
                recovery_hint: "check_upstream_logs",
                user_message: "Check Codepool/Letta logs and retry if appropriate.",
            },
        },
        uuid: "msg-4",
    } as never);

    assert.equal(events.length, 1);
    const event = events[0] as BearChannelEvent & { type: "error" };
    assert.equal(event.type, "error");
    assert.equal(event.terminal?.outcome, "empty_fallback");
    assert.equal(event.terminal?.recovery_hint, "check_upstream_logs");
    assert.equal(
        event.terminal?.user_message,
        "Check Codepool/Letta logs and retry if appropriate.",
    );
});
