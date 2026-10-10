import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { loadAssets, response } from "./chat-test-support.mjs";

const context = vm.createContext({ URL });
loadAssets(context);
const errors = context.DenChatErrors;

for (const [status, hint] of [[401, /Sign in/], [403, /Access was denied/], [404, /unavailable/],
    [409, /Refresh chats/], [429, /Wait/], [502, /Retry/], [503, /Retry/]]) {
    test(`HTTP ${status} does not echo HTML, proxy plaintext, malformed JSON or diagnostics`, () => {
        for (const body of ["<!doctype html><h1>403</h1><pre>database secret</pre>",
            "proxy failure at https://user:password@provider.test/?api_key=private", "{broken", "",
            JSON.stringify({ detail: "server diagnostics", error: { message: "provider credential" } })]) {
            const text = errors.parse(body, status, "request-EXACT:01", "Could not load chats");
            assert.match(text, new RegExp(`HTTP ${status}`));
            assert.match(text, hint);
            assert.match(text, /Reference: request-EXACT:01$/);
            assert.doesNotMatch(text, /doctype|<h1|database|https:|password|private|diagnostics|provider credential/);
        }
    });
}

test("expected JSON business errors retain their exact actionable text and do not dump extra fields", () => {
    const business = "This model is no longer selectable. Choose another model or inherit default.";
    const text = errors.parse(JSON.stringify({ error: business, detail: "SQL secret", request_id: "body-ref",
        upstream_error: { api_key: "private" } }), 403, "header-ref", "Model unavailable", "application/json; charset=utf-8");
    assert.match(text, new RegExp(business.replaceAll(".", "\\.")));
    assert.match(text, /Reference: header-ref$/);
    assert.doesNotMatch(text, /SQL|secret|body-ref|private/);
    assert.equal(errors.apiMessage("Model 'a/b' is unavailable; choose 'c/d'. Budget < 5."), "Model 'a/b' is unavailable; choose 'c/d'. Budget < 5.");
});

test("legacy JSON is preferred without a media type, but HTML media types and structured errors are never trusted", () => {
    const json = JSON.stringify({ error: "Choose a configured hat", request_id: "body-only" });
    assert.match(errors.parse(json, 400, ""), /Choose a configured hat/);
    assert.doesNotMatch(errors.parse(json, 400, ""), /body-only/);
    assert.doesNotMatch(errors.parse(json, 403, "", "Failure", "text/html"), /Choose a configured hat/);
    assert.match(errors.parse(json, 400, "", "Failure", "application/problem+json"), /Choose a configured hat/);
});

test("credential evidence and markup in JSON error strings cannot reach the UI", () => {
    const text = errors.parse(JSON.stringify({ error: "Provider unavailable at https://user:pass@api.test/v1?key=secret; Bearer private-token api_key=raw-key sk-abcdefghijklmnop" }), 503, "safe-ref");
    assert.match(text, /Provider unavailable/);
    assert.doesNotMatch(text, /https:|user:pass|secret|private-token|raw-key|sk-abcdefgh/);
    for (const value of ["<img src=x onerror=alert(1)>", "<!doctype html>private", "&lt;h1&gt;private"]) {
        assert.match(errors.parse(JSON.stringify({ error: value }), 403, ""), /HTTP 403/);
    }
});

test("request references come only from a safe header and remain exact, never trimmed or repaired", async () => {
    assert.equal(errors.requestRef("Req-Exact_9.0:Ab"), "Req-Exact_9.0:Ab");
    for (const ref of [" ref ", "<img>", "https://user:secret@host", "line\nbreak", "x".repeat(129), {}, null]) {
        assert.equal(errors.requestRef(ref), "");
        assert.doesNotMatch(errors.parse("bad", 403, ref), /Reference:/);
    }
    await assert.rejects(errors.rejectResponse(response({ error: "Choose a hat", request_id: "wrong" }, 400, "application/json", "Ref-EXACT")),
        (error) => error.denChatControlled && error.message.includes("Reference: Ref-EXACT") && !error.message.includes("wrong"));
});

test("successful HTML/proxy pages and unreadable error bodies still give safe status, recovery and header reference", async () => {
    await assert.rejects(errors.readJson(response("<html>private login proxy</html>", 200, "text/html", "REF-exact"), "Could not load chats"), (error) => {
        assert.equal(error.denChatControlled, true);
        assert.match(error.message, /HTTP 200.*unexpected response.*Refresh and retry.*Reference: REF-exact/s);
        assert.doesNotMatch(error.message, /<html>|private login/);
        return true;
    });
    const unreadable = { status: 502, headers: { get: (name) => name === "x-request-id" ? "REF-exact" : "text/html" },
        text: async () => { throw new Error("private https://user:pass@provider"); } };
    await assert.rejects(errors.rejectResponse(unreadable, "Could not load chats"), (error) => {
        assert.match(error.message, /HTTP 502.*Reference: REF-exact/s);
        assert.doesNotMatch(error.message, /private|https:|user:pass/);
        return true;
    });
});

test("network exceptions never expose a URL, credentials or browser/proxy exception details", () => {
    assert.doesNotMatch(errors.failure(new Error("fetch https://user:pass@host?api_key=secret")), /https:|user:pass|secret/);
    assert.match(errors.failure(new Error("offline")), /Check your connection and retry/);
});

for (const [code, status, message] of [
    ["model_missing", 400, "The selected model openai/gpt-6-sol is missing from this Bear's Bifrost catalog. Choose an available model in Bear → Models."],
    ["virtual_key_rejected", 409, "This Bear's Bifrost virtual key could not authorize model access. Repair its gateway setup in Bear → Models."],
    ["catalog_unavailable", 503, "The Bifrost model catalog could not be checked. Try again shortly."],
]) {
    test(`${code} displays model-specific recovery and the header reference without treating gateway auth as Den login`, async () => {
        const res = response({ code, error: message, model: "openai/gpt-6-sol", request_id: "wrong-body-ref",
            detail: "provider-password-secret-CANARY", virtual_key: "vk-web-fixture-secret-CANARY" }, status, "application/json", "REF-availability-exact");
        assert.equal(errors.requiresLogin(res, "https://den.test"), false);
        await assert.rejects(errors.readJson(res, "Could not send message"), (error) => {
            assert.ok(error.message.includes(message));
            assert.match(error.message, /Reference: REF-availability-exact$/);
            assert.doesNotMatch(error.message, /wrong-body-ref|CANARY|vk-web|provider-password/);
            return true;
        });
    });
}

test("login-required checks retain 401, redirected POST 405 and login-page redirect handling", () => {
    for (const item of [{ status: 401 }, { status: 405 }, { status: 200, redirected: true, url: "https://den.test/login?next=%2Fchat" }]) {
        assert.equal(errors.requiresLogin(item, "https://den.test"), true);
    }
    assert.equal(errors.requiresLogin({ status: 403 }, "https://den.test"), false);
    assert.equal(errors.requiresLogin({ status: 502, redirected: true, url: "https://proxy.test/failure" }, "https://den.test"), false);
});

test("runtime-specific failures preserve summaries and codes, never arbitrary detail", () => {
    const text = errors.runtimeError({ error_type: "tool_failure", message: "Permission revoked",
        detail: "No task was changed. Retry after access is restored.", support_ref: "body-ref",
        context: { upstream_error: { code: "EACCES", body: "secret", headers: { Authorization: "Bearer private" },
            url: "https://user:pass@host" } } }, "header-ref");
    assert.match(text, /tool_failure.*Permission revoked.*EACCES.*Retry or ask for help.*Reference: header-ref/s);
    assert.doesNotMatch(text, /No task was changed|secret|private|https:|body-ref/);
});

for (const detail of [
    JSON.stringify({ error: { api_key: "quoted-api-secret", password: "quoted-password-secret", metadata: "unrecognized-secret", code: "EACCES" } }),
    '{"api_key": "escaped-\\"secret", "password": "nested-password"}',
    "Provider diagnostic with unlabelled unrecognized-secret",
]) {
    test(`runtime never forwards provider detail: ${detail}`, () => {
        const text = errors.runtimeError({ error_type: "llm_error", message: "Provider unavailable", detail,
            context: { upstream_error: { code: "EACCES" } } }, "EXACT-ref");
        assert.match(text, /Provider unavailable.*EACCES.*Retry or ask for help.*Reference: EXACT-ref/s);
        assert.doesNotMatch(text, /quoted-api-secret|quoted-password-secret|unrecognized-secret|escaped-|nested-password|metadata|Provider diagnostic/);
    });
}

test("serialized diagnostics in runtime message are also omitted; quoted credential redaction is defense-in-depth", () => {
    for (const message of ['{"password":"message-secret"}', 'Provider diagnostic: {"api_key":"message-secret","unknown":"other-secret"}']) {
        const text = errors.runtimeError({ message, detail: "detail-secret" }, "EXACT-ref");
        assert.match(text, /could not complete.*Retry or ask for help.*Reference: EXACT-ref/s);
        assert.doesNotMatch(text, /message-secret|detail-secret|other-secret/);
    }
    const text = errors.apiMessage('{"api_key":"quoted-api-secret","password":"quoted-password-secret","authorization":"Bearer raw-secret"}');
    assert.doesNotMatch(text, /quoted-api-secret|quoted-password-secret|raw-secret/);
});
