#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const template = readFileSync(new URL("../src/templates/bear_chat.html", import.meta.url), "utf8");
function code(name) {
    const result = template.match(new RegExp(`    function ${name}\\([^]*?\\n    \\}`));
    assert.ok(result, `missing Chat function ${name}`);
    return result[0];
}
function fixture(fetch) {
    const messages = [];
    let opened = 0, closed = 0;
    const signals = {
        onResponse: (value) => messages.push(value),
        onOpen: () => opened++, onClose: () => closed++, stopClicked: {},
    };
    const context = vm.createContext({
        fetch, BEAR_ID: "bear", CONVERSATION_ID: "conv-errors", DEN_HATS: [], DEN_CONVERSATIONS: [],
        DEN_CHAT_DEBUG_VERBOSE: false, errEl: { hidden: false },
        AbortController, TextDecoder, Uint8Array,
        denResponseRequiresLogin: () => false,
        denReasoningLoadingToggle: () => false,
        stripSystemReminderBlocks: (text) => text,
        stripSystemReminderBlocksLive: (text) => text,
        loadConversations: () => {},
    });
    vm.runInContext(["formatChatError", "denChatSignalError", "denChatParseSendError", "runtimeInnerForStream",
        "compactJsonForError", "appendErrorContextLines", "runtimeStreamEventErrorSignal", "assistantChunkText", "nativeSseHandler"
    ].map(code).join("\n"), context);
    return { context, messages, signals, opened: () => opened, closed: () => closed };
}
const settle = async () => { await new Promise(setImmediate); await new Promise(setImmediate); };

function streamResponse(events) {
    const chunks = events.map((event) => new TextEncoder().encode("data: " + JSON.stringify(event) + "\n\n"));
    return {
        ok: true, status: 200, headers: { get: () => "header-reference" },
        body: { getReader: () => ({ read: async () => chunks.length ? { value: chunks.shift(), done: false } : { done: true } }) },
    };
}

test("native HTTP JSON failures remain visible as error-role text with the support reference", async () => {
    const f = fixture(async () => ({
        ok: false, status: 403, headers: { get: () => "header-reference" },
        text: async () => JSON.stringify({ error: "This conversation is read-only", request_id: "json-reference" }),
    }));
    f.context.nativeSseHandler({ messages: [{ role: "user", text: "hello" }] }, f.signals);
    await settle();
    assert.equal(f.messages.length, 1);
    assert.equal(f.messages[0].role, "error");
    assert.match(f.messages[0].text, /Couldn’t send message/);
    assert.match(f.messages[0].text, /read-only/);
    assert.match(f.messages[0].text, /Reference: json-reference/);
    assert.ok(!("error" in f.messages[0]));
});

test("native plain HTTP errors retain the header reference", async () => {
    const f = fixture(async () => ({
        ok: false, status: 503, headers: { get: () => "header-reference" },
        text: async () => "Server Error: temporarily unavailable",
    }));
    f.context.nativeSseHandler({ messages: [] }, f.signals);
    await settle();
    assert.equal(f.messages[0].role, "error");
    assert.match(f.messages[0].text, /temporarily unavailable/);
    assert.match(f.messages[0].text, /Reference: header-reference/);
});

for (const wrapped of [false, true]) {
    test(`native SSE ${wrapped ? "wrapped" : "direct"} errors retain details, upstream evidence and reference`, async () => {
        const error = {
            message_type: "error_message", error_type: "tool_failure", message: "Permission revoked",
            detail: "No task was changed", support_ref: "event-reference",
            context: { upstream_error: { code: "EACCES" } },
        };
        const f = fixture(async () => streamResponse([wrapped ? { contents: error } : error]));
        f.context.nativeSseHandler({ messages: [] }, f.signals);
        await settle();
        assert.equal(f.messages.length, 1);
        assert.equal(f.messages[0].role, "error");
        assert.match(f.messages[0].text, /Permission revoked/);
        assert.match(f.messages[0].text, /No task was changed/);
        assert.match(f.messages[0].text, /EACCES/);
        assert.match(f.messages[0].text, /Reference: event-reference/);
        assert.equal(f.closed(), 1);
    });
}

test("native network failures still produce visible error-role text", async () => {
    const f = fixture(async () => { throw new Error("network unavailable"); });
    f.context.nativeSseHandler({ messages: [] }, f.signals);
    await settle();
    assert.equal(f.messages[0].role, "error");
    assert.match(f.messages[0].text, /Connection problem.*network unavailable/s);
});

test("read-only legacy Chat rejects sends locally and points to New chat", () => {
    let called = false;
    const f = fixture(async () => { called = true; });
    f.context.DEN_HATS = [{ id: "hat" }];
    f.context.DEN_CONVERSATIONS = [{ id: "conv-errors" }];
    f.context.nativeSseHandler({ messages: [] }, f.signals);
    assert.equal(called, false);
    assert.match(f.messages[0].text, /read-only.*New chat/);
    assert.equal(f.messages[0].role, "error");
    assert.match(template, /errorMessages='\{"displayServiceErrorMessages":true\}'/);
    assert.match(template, /chat.errorMessages = \{\s*displayServiceErrorMessages: true/);
});
