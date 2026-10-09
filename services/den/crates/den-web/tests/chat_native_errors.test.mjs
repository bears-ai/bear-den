#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadAssets, response } from "./chat-test-support.mjs";

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
        fetch, BEAR_ID: "bear", CONVERSATION_ID: "conv-errors", DEN_HATS: [{ id: "hat" }],
        DEN_CONVERSATIONS_LOADED: true,
        DEN_CONVERSATIONS: [{ id: "conv-errors", hat_id: "hat", own_notes_available: true, can_send: true }],
        DEN_CHAT_DEBUG_VERBOSE: false, errEl: { hidden: false },
        AbortController, TextDecoder, Uint8Array, URL,
        denResponseRequiresLogin: () => false,
        denReasoningLoadingToggle: () => false,
        stripSystemReminderBlocks: (text) => text,
        stripSystemReminderBlocksLive: (text) => text,
        loadConversations: () => {},
    });
    loadAssets(context);
    context.chatOperations = context.DenChatOperations({ selection: () => context.CONVERSATION_ID, renderError() {} });
    vm.runInContext(["denConversationState", "formatChatError", "denChatSignalError", "denChatParseSendError", "runtimeInnerForStream",
        "runtimeStreamEventErrorSignal", "assistantChunkText", "nativeSseHandler"
    ].map(code).join("\n"), context);
    return { context, messages, signals, opened: () => opened, closed: () => closed };
}
const settle = async () => { await new Promise(setImmediate); await new Promise(setImmediate); };

function streamResponse(events) {
    const chunks = events.map((event) => new TextEncoder().encode("data: " + JSON.stringify(event) + "\n\n"));
    return {
        ok: true, status: 200, headers: { get: (name) => name === "content-type" ? "text/event-stream" : "header-reference" },
        body: { getReader: () => ({ read: async () => chunks.length ? { value: chunks.shift(), done: false } : { done: true } }) },
    };
}

test("native HTTP JSON failures remain visible as error-role text with the support reference", async () => {
    const f = fixture(async () => response({ error: "This conversation is read-only", request_id: "json-reference" }, 403));
    f.context.nativeSseHandler({ messages: [{ role: "user", text: "hello" }] }, f.signals);
    await settle();
    assert.equal(f.messages.length, 1);
    assert.equal(f.messages[0].role, "error");
    assert.match(f.messages[0].text, /Couldn’t send message/);
    assert.match(f.messages[0].text, /read-only/);
    assert.match(f.messages[0].text, /Reference: header-reference/);
    assert.ok(!("error" in f.messages[0]));
});

test("native plain HTTP errors retain the header reference", async () => {
    const f = fixture(async () => response("Server Error: temporarily unavailable", 503, "text/plain"));
    f.context.nativeSseHandler({ messages: [] }, f.signals);
    await settle();
    assert.equal(f.messages[0].role, "error");
    assert.match(f.messages[0].text, /HTTP 503.*temporarily unavailable/s);
    assert.match(f.messages[0].text, /Reference: header-reference/);
    assert.equal(f.closed(), 1);
});

for (const wrapped of [false, true]) {
    test(`native SSE ${wrapped ? "wrapped" : "direct"} errors retain safe summaries, codes and reference, not detail`, async () => {
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
        assert.doesNotMatch(f.messages[0].text, /No task was changed/);
        assert.match(f.messages[0].text, /EACCES/);
        assert.match(f.messages[0].text, /Reference: header-reference/);
        assert.equal(f.closed(), 1);
    });
}

test("native network failures still produce visible error-role text", async () => {
    const f = fixture(async () => { throw new Error("network unavailable"); });
    f.context.nativeSseHandler({ messages: [] }, f.signals);
    await settle();
    assert.equal(f.messages[0].role, "error");
    assert.match(f.messages[0].text, /Connection problem.*Check your connection and retry/s);
    assert.equal(f.closed(), 1);
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
    assert.equal(f.closed(), 1);
});

for (const [status, type] of [[403, "text/html"], [502, "text/plain"], [200, "text/html"]]) {
    test(`send failure HTTP ${status} ${type} never prints a proxy/login body and closes the stream`, async () => {
        const f = fixture(async () => response("<html>https://user:pass@provider.test?api_key=private</html>", status, type));
        await f.context.nativeSseHandler({ messages: [] }, f.signals);
        assert.equal(f.messages[0].role, "error");
        assert.match(f.messages[0].text, new RegExp(`HTTP ${status}`));
        assert.match(f.messages[0].text, /Reference: header-reference/);
        assert.doesNotMatch(f.messages[0].text, /<html>|https:|user:pass|private/);
        assert.equal(f.closed(), 1);
    });
}

test("zero hats, missing ownership, pending or unlisted conversation never permits a send", () => {
    for (const rows of [[], [{ id: "conv-errors" }], [{ id: "conv-errors", hat_id: "hat" }],
        [{ id: "conv-errors", hat_id: "hat", own_notes_available: true, pending: true }]]) {
        let calls = 0;
        const f = fixture(async () => calls++);
        f.context.DEN_HATS = [];
        f.context.DEN_CONVERSATIONS = rows;
        f.context.nativeSseHandler({ messages: [] }, f.signals);
        assert.equal(calls, 0);
        assert.equal(f.messages[0].role, "error");
        assert.equal(f.closed(), 1);
    }
});

test("send login redirect and redirected POST 405 retain login handling without rendering its body", async () => {
    for (const res of [response("<html>private login</html>", 401, "text/html"),
        response("<html>private login</html>", 405, "text/html"),
        { ok: true, status: 200, redirected: true, url: "https://den.test/login", headers: { get: () => "" } }]) {
        const f = fixture(async () => res);
        let redirects = 0;
        f.context.denResponseRequiresLogin = (value) => f.context.DenChatErrors.requiresLogin(value, "https://den.test");
        f.context.denRedirectToLogin = () => redirects++;
        await f.context.nativeSseHandler({ messages: [] }, f.signals);
        assert.equal(redirects, 1);
        assert.equal(f.messages.length, 0);
        assert.equal(f.closed(), 1);
    }
});

test("stream read failures and stops retain the safe header reference without exposing an exception", async () => {
    for (const name of ["Error", "AbortError"]) {
        const res = streamResponse([]);
        res.body.getReader = () => ({ read: async () => {
            const err = new Error("https://user:pass@provider.test?key=private"); err.name = name; throw err;
        } });
        const f = fixture(async () => res);
        await f.context.nativeSseHandler({ messages: [] }, f.signals);
        assert.match(f.messages[0].text, /Reference: header-reference/);
        assert.doesNotMatch(f.messages[0].text, /https:|user:pass|private/);
        assert.equal(f.closed(), 1);
    }
});

test("runtime errors do not dump upstream credentials, HTML or URLs into Chat", async () => {
    const f = fixture(async () => streamResponse([{
        message_type: "error_message", error_type: "provider_failure", message: "Provider unavailable",
        detail: "Retry connection https://user:pass@api.test/?api_key=private Bearer private-token", support_ref: "wrong",
        context: { upstream_error: { code: "EACCES", body: "<html>private upstream</html>", api_key: "secret" } },
    }]));
    await f.context.nativeSseHandler({ messages: [] }, f.signals);
    assert.ok(f.messages[0].text.includes(f.context.DenChatErrors.literalText("[provider_failure]")));
    assert.match(f.messages[0].text, /Provider unavailable.*EACCES.*Reference: header-reference/s);
    assert.doesNotMatch(f.messages[0].text, /https:|user:pass|private-token|<html>|secret|wrong/);
    assert.equal(f.closed(), 1);
});

for (const wrapped of [false, true]) {
    test(`serialized provider detail is never printed in ${wrapped ? "wrapped" : "direct"} runtime stream errors`, async () => {
        const error = { message_type: "error_message", error_type: "llm_error", message: "Provider unavailable",
            detail: JSON.stringify({ api_key: "quoted-api-secret", password: "quoted-password-secret", provider_metadata: "other-secret" }),
            context: { upstream_error: { code: "EACCES" } } };
        const f = fixture(async () => streamResponse([wrapped ? { contents: error } : error]));
        await f.context.nativeSseHandler({ messages: [] }, f.signals);
        assert.match(f.messages[0].text, /Provider unavailable.*EACCES.*Retry or ask for help.*Reference: header-reference/s);
        assert.doesNotMatch(f.messages[0].text, /quoted-api-secret|quoted-password-secret|other-secret|provider_metadata/);
        assert.equal(f.closed(), 1);
    });
}

test("JSON error Markdown and header punctuation are literal text, not executable links or formatting", async () => {
    const business = "Choose [another model](javascript:alert(1)), not **this pin**.";
    const f = fixture(async () => response({ error: business }, 400, "application/json", "REF__exact__"));
    await f.context.nativeSseHandler({ messages: [] }, f.signals);
    assert.equal(f.messages[0].text, f.context.DenChatErrors.literalText(
        f.context.DenChatErrors.parse(JSON.stringify({ error: business }), 400, "REF__exact__", "Couldn’t send message", "application/json")));
    assert.ok(f.messages[0].text.includes("\\[another model\\]\\(javascript:alert\\(1\\)\\)"));
    assert.ok(f.messages[0].text.includes("REF\\_\\_exact\\_\\_"));
    assert.equal(f.closed(), 1);
});
