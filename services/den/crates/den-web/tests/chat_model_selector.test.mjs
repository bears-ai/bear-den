import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { code, loadAssets, response, deferred } from "./chat-test-support.mjs";

const template = readFileSync(new URL("../src/templates/bear_chat.html", import.meta.url), "utf8");
const selectorCode = template.slice(
    template.indexOf("    function currentModelSelect()"),
    template.indexOf("    function denLoginRedirectUrl()"),
);
const escapeCode = template.match(/    function escapeHtml\(t\) \{[\s\S]*?\n    \}/)[0];

function fixture() {
    let html = "";
    let selected = "";
    const select = {
        options: [],
        title: "",
        disabled: false,
        get innerHTML() { return html; },
        set innerHTML(value) {
            html = value;
            this.options = Array.from(value.matchAll(/<option\b([^>]*)>([\s\S]*?)<\/option>/g), (match) => ({
                value: match[1].match(/value="([^"]*)"/)[1],
                text: match[2],
                selected: /\bselected\b/.test(match[1]),
                disabled: /\bdisabled\b/.test(match[1]),
            }));
            selected = (this.options.find((option) => option.selected) || this.options[0])?.value || "";
        },
        get value() { return selected; },
        set value(value) { selected = this.options.some((option) => option.value === value) ? value : ""; },
    };
    const errors = [];
    const modelError = { textContent: "", hidden: true };
    const context = vm.createContext({
        document: { getElementById: (id) => id === "den-model-select" ? select : id === "den-model-error" ? modelError : null },
        BEAR_ID: "bear",
        CONVERSATION_ID: "conv-model",
        DEN_MODEL_OPTIONS: [], DEN_MODEL_MUTATION: null, DEN_MODEL_AVAILABLE: false,
        DEN_CONVERSATIONS_LOADED: true, DEN_HATS: [{ id: "hat" }],
        DEN_CONVERSATIONS: [{ id: "conv-model", hat_id: "hat", own_notes_available: true, can_send: true }],
        denResponseRequiresLogin: () => false, denRedirectToLogin: () => {},
        URLSearchParams, URL,
        showErr: (message, token) => context.chatOperations.fail(token, message),
    });
    loadAssets(context);
    context.chatOperations = context.DenChatOperations({
        selection: () => context.CONVERSATION_ID,
        renderError: (message) => { if (message) errors.push(message); },
    });
    vm.runInContext(code("denConversationState") + escapeCode + selectorCode, context);
    return { context, select, errors, modelError };
}

const options = [{ handle: "test/model", label: "Catalog <model>" }];

for (const [source, label] of [
    ["hat_override", "Hat override"],
    ["bear_default", "Bear default"],
    ["deployment_default", "Deployment default"],
]) {
    test(`auto is labelled inheritance from ${source}, not a cached model pin`, () => {
        const { context, select } = fixture();
        context.renderModelSelector({
            selection_mode: "auto", source, selected_model: "stale/cache", requested_model: "stale/cache",
            effective_model: "test/model", configuration_name: source === "deployment_default" ? null : "Deep <config>",
            thinking_effort: source === "deployment_default" ? null : "high", model_options: options,
        });
        assert.equal(select.value, "auto");
        assert.match(select.options[0].text, /^Inherit default/);
        assert.ok(select.options[0].text.includes(label));
        assert.ok(select.title.includes(label));
        assert.ok(!select.title.includes("stale/cache"));
        assert.match(select.options[1].text, /^Pin · /);
        assert.ok(select.options[1].text.includes("&lt;model&gt;"));
        if (source !== "deployment_default") {
            assert.ok(select.options[0].text.includes("Deep &lt;config&gt;"));
            assert.ok(select.title.includes("reasoning high"));
        } else {
            assert.ok(select.title.includes("reasoning model default"));
        }
    });
}

test("explicit selection is a pin, and does not label its effective model as inherited", () => {
    const { context, select } = fixture();
    context.renderModelSelector({
        selection_mode: "explicit", source: "conversation_explicit", selected_model: "test/model",
        effective_model: "test/model", configuration_name: null, thinking_effort: null, model_options: options,
    });
    assert.equal(select.value, "test/model");
    assert.equal(select.options[0].text, "Inherit default");
    assert.match(select.title, /^Conversation pin: test\/model/);
    assert.ok(select.title.includes("reasoning model default"));
});

test("a validated pin outside shortcut options remains a pin, never silently selects inheritance", () => {
    const { context, select } = fixture();
    context.renderModelSelector({
        selection_mode: "explicit", source: "conversation_explicit", selected_model: "test/other",
        effective_model: "test/other", model_options: options,
    });
    assert.equal(select.value, "test/other");
    assert.equal(select.options.at(-1).text, "Pin · test/other");
});

test("pending conversations show the inherited preview but cannot save a pin yet", () => {
    const { context, select } = fixture();
    context.CONVERSATION_ID = "new-preview";
    context.DEN_CONVERSATIONS = [{ id: "new-preview", hat_id: "hat", own_notes_available: true, can_send: true, pending: true }];
    context.renderModelSelector({ selection_mode: "auto", source: "bear_default", effective_model: "test/model", model_options: options });
    assert.equal(select.value, "auto");
    assert.equal(select.disabled, true);
});

test("revoked pin remains visible with enabled recovery choices and an honest error", () => {
    const { context, select, modelError } = fixture();
    context.renderModelSelector({
        selection_mode: "explicit", source: "conversation_explicit", selected_model: "test/revoked",
        requested_model: "test/revoked", effective_model: null, error: "model is no longer selectable", model_options: options,
    });
    assert.equal(select.disabled, false);
    assert.equal(select.value, "test/revoked");
    assert.equal(select.options[0].value, "auto");
    assert.equal(select.options[1].value, "test/model");
    assert.equal(select.options.at(-1).disabled, true);
    assert.equal(select.options.at(-1).text, "Pin · test/revoked · unavailable");
    assert.equal(modelError.hidden, false);
    assert.equal(modelError.textContent, "model is no longer selectable");
    assert.ok(!select.title.includes("reasoning model default"));
});

test("unavailable inherited config can be replaced by a valid pin and clears its error", () => {
    const { context, select, modelError } = fixture();
    context.renderModelSelector({ selection_mode: "auto", effective_model: null, error: "configuration unavailable", model_options: options });
    assert.equal(select.disabled, false);
    assert.equal(select.value, "auto");
    assert.equal(select.options[0].text, "Inherit default · unavailable");
    assert.equal(modelError.textContent, "configuration unavailable");
    context.renderModelSelector({ selection_mode: "explicit", source: "conversation_explicit", selected_model: "test/model", effective_model: "test/model", error: null, model_options: options });
    assert.equal(select.value, "test/model");
    assert.equal(modelError.hidden, true);
    assert.equal(modelError.textContent, "");
});

test("missing explicit pin has a recovery placeholder, not implicit inheritance", () => {
    const { context, select, modelError } = fixture();
    context.renderModelSelector({ selection_mode: "explicit", selected_model: null, requested_model: null, effective_model: null, error: "explicit selection has no model", model_options: options });
    assert.equal(select.disabled, false);
    assert.equal(select.value, "");
    assert.equal(select.options.at(-1).text, "Pin · missing model · unavailable");
    assert.equal(select.options.at(-1).disabled, true);
    assert.equal(modelError.hidden, false);
});

test("GET of an invalid pin still allows PATCH clear and valid replacement", async () => {
    const { context, select, modelError } = fixture();
    const patches = [];
    let canonical = { selection_mode: "explicit", selected_model: "test/revoked", effective_model: null, error: "revoked pin", model_options: options };
    context.fetch = async (_url, request) => {
        if (!request.method) return response(canonical);
        const body = JSON.parse(request.body);
        patches.push(body);
        canonical = body.selection_mode === "auto"
            ? { selection_mode: "auto", effective_model: null, error: "inherited configuration unavailable", model_options: options }
            : { selection_mode: "explicit", source: "conversation_explicit", selected_model: body.model, effective_model: body.model, error: null, model_options: options };
        return response(canonical);
    };
    await context.loadConversationModel();
    assert.equal(select.disabled, false);
    await context.saveConversationModel("auto");
    assert.equal(patches[0].selection_mode, "auto");
    assert.equal(patches[0].model, null);
    assert.equal(select.disabled, false);
    assert.equal(select.value, "auto");
    assert.equal(modelError.textContent, "inherited configuration unavailable");
    await context.saveConversationModel("test/model");
    assert.equal(patches[1].selection_mode, "explicit");
    assert.equal(select.value, "test/model");
    assert.equal(modelError.hidden, true);
});

test("Den-known but gateway-absent stored primary is not offered as a usable pin", () => {
    const { context, select, modelError } = fixture();
    context.renderModelSelector({
        selection_mode: "auto", source: "bear_default", configuration_name: "Chosen primary",
        effective_model: null, unavailable_model: "openai/gpt-6-sol", error_code: "model_missing",
        error: "The selected model openai/gpt-6-sol is not available to this Bear in Bifrost. Choose an available model in Bear → Models, or correct its Bifrost virtual-key access.",
        model_options: [{ handle: "openai/gpt-4.1", label: "Den label" }],
    });
    assert.equal(select.value, "auto");
    assert.equal(select.disabled, false);
    assert.ok(!select.options.some((option) => option.value === "openai/gpt-6-sol"));
    assert.match(modelError.textContent, /openai\/gpt-6-sol.*Bifrost.*Bear → Models/);
    assert.equal(modelError.hidden, false);
});

for (const code of ["virtual_key_missing", "virtual_key_rejected", "catalog_unavailable"]) {
    test(`${code} leaves unavailable pin inspectable and clearing available without synthesizing models`, () => {
        const { context, select, modelError } = fixture();
        context.renderModelSelector({
            selection_mode: "explicit", selected_model: "openai/gpt-6-sol", effective_model: null,
            error_code: code, error: "Repair this Bear's Bifrost access in Bear → Models.", model_options: [],
        });
        assert.equal(select.value, "openai/gpt-6-sol");
        assert.equal(select.disabled, false);
        assert.equal(select.options.length, 2);
        assert.equal(select.options[0].value, "auto");
        assert.equal(select.options[0].disabled, false);
        assert.equal(select.options[1].disabled, true);
        assert.match(select.options[1].text, /unavailable/);
        assert.match(modelError.textContent, /Bear → Models/);
    });
}

test("an existing pin during catalog outage stays effective but unverified, not offered as a new usable choice", () => {
    const { context, select, modelError } = fixture();
    context.renderModelSelector({
        selection_mode: "explicit", source: "conversation_explicit", requested_model: "openai/gpt-6-sol",
        selected_model: "openai/gpt-6-sol", effective_model: "openai/gpt-6-sol", availability: "unverified",
        error_code: "catalog_unavailable", error: "Availability is unverified. The existing pin may attempt the same model; no substitute will be chosen.", model_options: [],
    });
    assert.equal(select.value, "openai/gpt-6-sol");
    assert.equal(select.disabled, false);
    assert.equal(select.options[1].disabled, true);
    assert.match(select.options[1].text, /availability unverified/);
    assert.match(select.title, /availability unverified/);
    assert.doesNotMatch(select.title, /unavailable|reasoning model default/);
    assert.equal(select.options[0].disabled, false);
    assert.match(modelError.textContent, /same model.*no substitute/);
});

test("empty selectable catalog does not synthesize options or block clearing a pin", () => {
    const { context, select } = fixture();
    context.renderModelSelector({ selection_mode: "explicit", selected_model: "test/revoked", effective_model: null, error: "model unavailable", model_options: [] });
    assert.equal(select.disabled, false);
    assert.equal(select.options.length, 2);
    assert.equal(select.options[0].value, "auto");
    assert.equal(select.options[1].disabled, true);
});

test("authorization failure clears stale display and disables the selector", async () => {
    const { context, select, errors } = fixture();
    context.renderModelSelector({ selection_mode: "auto", source: "bear_default", effective_model: "test/model", model_options: options });
    context.fetch = async () => response({ error: "not authorized" }, 403);
    await context.loadConversationModel();
    assert.equal(select.disabled, true);
    assert.equal(select.options[0].text, "Model unavailable");
    assert.equal(select.value, "");
    assert.match(errors[0], /not authorized.*Reference: header-reference/s);
});

for (const body of ["<html>private provider diagnostics https://user:pass@host</html>", "proxy failure password=private"]) {
    test(`model GET hides an unstructured failed body: ${body}`, async () => {
        const { context, select, errors } = fixture();
        context.fetch = async () => response(body, 403, "text/html", "REF-exact");
        await context.loadConversationModel();
        assert.equal(select.disabled, true);
        assert.match(select.title, /HTTP 403.*Reference: REF-exact/s);
        assert.match(errors[0], /Access was denied/);
        assert.doesNotMatch(select.title + errors[0], /<html>|private|diagnostics|https:|user:pass/);
    });
}

test("failed model PATCH remains visible while GET restores recovery choices", async () => {
    const { context, select, errors } = fixture();
    let gets = 0;
    context.fetch = async (_url, init) => init.method === "PATCH" ?
        response({ error: "Model access was revoked. Choose another model.", request_id: "wrong" }, 403) :
        (gets++, response({ selection_mode: "auto", effective_model: "test/model", model_options: options }));
    await context.saveConversationModel("test/revoked");
    assert.equal(gets, 1);
    assert.equal(select.disabled, false);
    assert.match(errors[0], /Model access was revoked.*Reference: header-reference/s);
    assert.doesNotMatch(errors[0], /wrong/);
});

test("no model GET/PATCH is issued for a fresh unbound or pending preview", async () => {
    const { context, select } = fixture();
    context.fetch = async () => { assert.fail("unbound preview must not inspect or persist default"); };
    for (const id of ["", "default", "new-preview"]) {
        context.CONVERSATION_ID = id;
        await context.loadConversationModel();
        await context.saveConversationModel("auto");
        assert.equal(select.disabled, true);
    }
});

test("stale model errors do not disable a different selected chat or hide its recovery", async () => {
    const { context, select, errors } = fixture();
    const old = deferred();
    context.fetch = async () => old.promise;
    const first = context.loadConversationModel();
    context.CONVERSATION_ID = "conv-next";
    context.DEN_CONVERSATIONS.push({ id: "conv-next", hat_id: "hat", own_notes_available: true, can_send: true });
    context.fetch = async () => response({ selection_mode: "auto", effective_model: "test/next", model_options: [] });
    await context.loadConversationModel();
    old.resolve(response("<html>old error private</html>", 403, "text/html"));
    await first;
    assert.equal(errors.length, 0);
    assert.equal(select.disabled, false);
    assert.match(select.title, /test\/next/);
});

test("model login-required failures redirect rather than showing a login HTML body", async () => {
    const { context, errors } = fixture();
    let redirects = 0;
    context.denResponseRequiresLogin = (res) => context.DenChatErrors.requiresLogin(res, "https://den.test");
    context.denRedirectToLogin = () => redirects++;
    context.fetch = async () => response("<html>private login page</html>", 401, "text/html");
    await context.loadConversationModel();
    await context.saveConversationModel("auto");
    assert.equal(redirects, 3);
    assert.equal(errors.length, 0);
});

for (const canSend of [false, undefined, "true"]) {
    test(`server-listed read-only bound models remain inspectable with can_send=${canSend}`, async () => {
        const { context, select } = fixture();
        context.DEN_CONVERSATIONS[0].can_send = canSend;
        context.DEN_CONVERSATIONS[0].hat_status = "inactive";
        let reads = 0;
        context.fetch = async (_url, init) => {
            assert.equal(init.method, undefined, "read permission must not imply PATCH permission");
            reads++;
            return response({ selection_mode: "explicit", selected_model: "test/readonly", effective_model: "test/readonly", model_options: options });
        };
        await context.loadConversationModel();
        assert.equal(reads, 1);
        assert.equal(select.value, "test/readonly");
        assert.match(select.title, /test\/readonly/);
        assert.equal(select.disabled, true);
        await context.saveConversationModel("auto");
        assert.equal(reads, 1);
    });
}

test("model mutations cannot overlap, remain disabled through reconciliation, and ignore pre-PATCH cached GETs", async () => {
    const { context, select } = fixture();
    const oldRead = deferred(), patch = deferred(), reread = deferred();
    const patches = [];
    let gets = 0, inFlight = 0, maximum = 0;
    context.fetch = async (_url, init) => {
        if (!init.method) return ++gets === 1 ? oldRead.promise : reread.promise;
        patches.push(JSON.parse(init.body).model);
        maximum = Math.max(maximum, ++inFlight);
        const result = await patch.promise;
        inFlight--;
        return result;
    };
    const old = context.loadConversationModel();
    const a = context.saveConversationModel("test/A");
    assert.equal(select.disabled, true);
    const blockedB = context.saveConversationModel("test/B");
    assert.equal(blockedB, a);
    assert.deepEqual(patches, ["test/A"]);
    oldRead.resolve(response({ selection_mode: "explicit", selected_model: "cached/old", effective_model: "cached/old" }));
    await old;
    assert.doesNotMatch(select.title, /cached\/old/);
    patch.resolve(response({ selection_mode: "explicit", selected_model: "cached/patch-response", effective_model: "cached/patch-response" }));
    await new Promise(setImmediate);
    assert.equal(gets, 2);
    assert.equal(select.disabled, true, "PATCH success alone must not unlock stale UI");
    await context.loadConversationModel();
    assert.equal(gets, 2, "background reads cannot race reconciliation");
    reread.resolve(response({ selection_mode: "explicit", selected_model: "test/A", effective_model: "test/A", model_options: options }));
    await a;
    assert.equal(select.value, "test/A");
    assert.equal(select.disabled, false);
    assert.doesNotMatch(select.title, /cached/);
    assert.equal(maximum, 1);
    let canonical = "test/A";
    context.fetch = async (_url, init) => {
        if (init.method) { canonical = JSON.parse(init.body).model; patches.push(canonical); }
        return response({ selection_mode: "explicit", selected_model: canonical, effective_model: canonical, model_options: options });
    };
    await context.saveConversationModel("test/B");
    assert.deepEqual(patches, ["test/A", "test/B"]);
    assert.equal(select.value, "test/B");
});

test("selection changes during the final model GET require a fresh reread, including A -> B -> A", async () => {
    const { context, select } = fixture();
    const old = deferred();
    let gets = 0;
    context.fetch = async (_url, init) => init.method ? response({ ok: true }) : ++gets === 1 ? old.promise :
        response({ selection_mode: "explicit", selected_model: "test/A", effective_model: "test/A", model_options: options });
    const mutation = context.saveConversationModel("test/A");
    await new Promise(setImmediate);
    context.CONVERSATION_ID = "conv-next"; context.chatOperations.selectionChanged();
    context.DEN_CONVERSATIONS.push({ id: "conv-next", hat_id: "hat", can_send: true });
    await context.loadConversationModel();
    context.CONVERSATION_ID = "conv-model"; context.chatOperations.selectionChanged();
    await context.loadConversationModel();
    old.resolve(response({ selection_mode: "explicit", selected_model: "cached/old", effective_model: "cached/old" }));
    await mutation;
    assert.equal(gets, 2);
    assert.equal(select.value, "test/A");
    assert.equal(select.disabled, false);
    assert.doesNotMatch(select.title, /cached\/old/);
});

test("canonical model reread failures stay disabled rather than unlocking unavailable metadata", async () => {
    const { context, select, errors } = fixture();
    context.fetch = async (_url, init) => init.method ? response({ ok: true }) : response({ error: "Model inspection denied" }, 403);
    await context.saveConversationModel("test/A");
    assert.equal(context.DEN_MODEL_MUTATION, null);
    assert.equal(select.disabled, true);
    assert.match(errors.at(-1), /Model inspection denied/);
});
