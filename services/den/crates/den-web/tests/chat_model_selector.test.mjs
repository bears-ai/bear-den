import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

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
        DEN_MODEL_OPTIONS: [],
        URLSearchParams,
        showErr: (message) => errors.push(message),
    });
    vm.runInContext(escapeCode + selectorCode, context);
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
    context.fetch = async (_url, request) => {
        if (!request.method) return { ok: true, json: async () => ({ selection_mode: "explicit", selected_model: "test/revoked", effective_model: null, error: "revoked pin", model_options: options }) };
        const body = JSON.parse(request.body);
        patches.push(body);
        return { ok: true, json: async () => body.selection_mode === "auto"
            ? { selection_mode: "auto", effective_model: null, error: "inherited configuration unavailable", model_options: options }
            : { selection_mode: "explicit", source: "conversation_explicit", selected_model: body.model, effective_model: body.model, error: null, model_options: options } };
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
    context.fetch = async () => ({ ok: false, status: 403, text: async () => "not authorized" });
    await context.loadConversationModel();
    assert.equal(select.disabled, true);
    assert.equal(select.options[0].text, "Model unavailable");
    assert.equal(select.value, "");
    assert.deepEqual(errors, ["not authorized"]);
});
