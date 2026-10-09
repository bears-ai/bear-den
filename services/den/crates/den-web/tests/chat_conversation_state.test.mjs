import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { inlineScript, template, loadAssets, response, listResponse, deferred, settle } from "./chat-test-support.mjs";

class Element {
    children = [];
    listeners = {};
    attributes = {};
    style = {};
    textContent = "";
    value = "";
    disabled = false;
    hidden = false;
    open = false;
    addEventListener(name, listener) { this.listeners[name] = listener; }
    setAttribute(name, value) { this.attributes[name] = value; }
    appendChild(child) { this.children.push(child); }
    replaceChildren() { this.children = []; }
    set innerHTML(value) { this.html = value; this.children = []; }
    get innerHTML() { return this.html || ""; }
}

const hat = { id: "hat-one", name: "Writer", purpose: "Write" };
const owned = { id: "conv-owned", title: "Owned chat", hat_id: hat.id, own_notes_available: true, can_send: true };
function fixture(search = "") {
    const elements = new Map();
    const get = (id) => {
        if (!elements.has(id)) elements.set(id, new Element());
        return elements.get(id);
    };
    get("chat").textInput = { disabled: true };
    let remounts = 0;
    Object.defineProperty(get("chat-wrap"), "innerHTML", {
        set(value) {
            remounts++;
            elements.set("chat", Object.assign(new Element(), { textInput: { disabled: true }, draft: "" }));
        },
    });
    const requests = [];
    const location = { href: "https://den.test/bear/chat" + search, pathname: "/bear/chat", search, origin: "https://den.test" };
    const context = vm.createContext({
        URL, URLSearchParams, AbortController, TextDecoder, Uint8Array,
        document: { getElementById: get, createElement: () => new Element(), documentElement: new Element() },
        window: { location, prompt: () => null, history: { replaceState(_state, _title, url) {
            const next = new URL(url); location.href = url; location.search = next.search;
        } } },
        customElements: { whenDefined: async () => {} },
        DenTaskSelector: () => ({ sync() {}, load: async () => {} }),
        fetch: async (url, init) => {
            requests.push({ url, init });
            return context.reply(url, init);
        },
    });
    loadAssets(context);
    vm.runInContext(inlineScript.replace(/\n    boot\(\);/, ""), context);
    context.BEAR_ID = "bear";
    context.resolveDeepChatLayoutTokens = () => Object.fromEntries(["bubblePad", "lh", "inputFont", "inputPadT", "inputPadB", "inputPadL", "inputPadR", "submitMb", "submitMr"].map((key) => [key, "1px"]));
    context.cv = () => "1px";
    context.configureDenChat();
    return {
        context, requests, get, location, remounts: () => remounts,
        draft(text) { get("chat").draft = text; get("chat").onInput(); },
        state: () => context.denConversationState(),
        newControls: () => {
            const item = get("den-conv-menu").children.at(-1);
            return { select: item.children[0], button: item.children[1] };
        },
        onlyList(hats = [], conversations = []) {
            context.reply = async (url) => {
                if (url.startsWith("/v1/chat/model?")) return response({ effective_model: "test/model" });
                assert.ok(url.startsWith("/v1/chat/conversations?"), `unexpected request ${url}`);
                return listResponse(hats, conversations, new URL(url, location.origin).searchParams.get("conversation_id"));
            };
        },
    };
}

test("fresh zero-hat UI has no fabricated Main chat, no executable New chat, no model/history/send request", async () => {
    const f = fixture();
    assert.equal(f.get("chat").textInput.disabled, true);
    f.onlyList();
    await f.context.loadConversations();
    assert.equal(f.context.CONVERSATION_ID, "");
    assert.equal(f.context.DEN_CONVERSATIONS.length, 0);
    assert.match(f.get("den-chat-selection-status").textContent, /Create a hat/);
    assert.match(f.get("den-conv-current").textContent, /Create a hat/);
    const controls = f.newControls();
    assert.equal(controls.select.disabled, true);
    assert.equal(controls.button.disabled, true);
    controls.button.onclick(); // Even programmatic invocation cannot create a legacy chat.
    await f.context.loadConversationModel();
    await f.context.saveConversationModel("auto");
    await f.context.loadDenHistory(0);
    const messages = [];
    let closed = 0;
    f.context.nativeSseHandler({ messages: [] }, { onResponse: (m) => messages.push(m), onClose: () => closed++ });
    assert.match(messages[0].text, /Create a hat/);
    assert.equal(closed, 1);
    assert.equal(f.get("chat").validateInput("hello"), false);
    assert.equal(f.requests.length, 1);
    assert.doesNotMatch(template, /title: "Main chat"|makeNewConversationId|DEN_PENDING_CONVERSATION/);
});

test("configured hat with no conversation requires explicit choose + New chat, then server-confirmed ownership", async () => {
    const f = fixture();
    f.onlyList([hat]);
    await f.context.loadConversations();
    assert.equal(f.requests.length, 1, "no unbound model inspection, including default");
    assert.equal(f.get("chat").textInput.disabled, true);
    assert.match(f.get("den-chat-selection-status").textContent, /Choose a hat/);
    const controls = f.newControls();
    assert.equal(controls.select.value, "");
    assert.equal(controls.button.disabled, true);
    controls.select.value = hat.id;
    controls.select.onchange();
    assert.equal(controls.button.disabled, false);
    const listed = deferred();
    f.context.reply = async (url, init) => {
        if (init.method === "POST") {
            assert.deepEqual(JSON.parse(init.body), { bear_id: "bear", hat_id: hat.id });
            return response({ id: owned.id, hat_id: hat.id, title: owned.title });
        }
        if (url.startsWith("/v1/chat/conversations?")) return listed.promise;
        assert.ok(url.startsWith("/v1/chat/model?"));
        assert.equal(new URL(url, f.location.origin).searchParams.get("conversation_id"), owned.id);
        return response({ selection_mode: "auto", effective_model: "test/model", model_options: [] });
    };
    const creation = controls.button.onclick();
    await settle();
    assert.equal(f.state().canSend, false, "POST response alone does not invent ownership/binding");
    assert.equal(f.context.DEN_CONVERSATIONS.length, 0);
    listed.resolve(listResponse([hat], [owned], owned.id));
    await creation;
    await settle();
    assert.equal(f.state().canSend, true);
    assert.equal(f.get("chat").textInput.disabled, false);
    assert.equal(f.get("chat").validateInput("hello"), true);
    assert.match(f.location.search, /conversation_id=conv-owned/);
    assert.equal(f.requests.filter(({ init }) => init.method === "POST").length, 1);
    assert.ok(f.requests.every(({ url }) => !url.includes("conversation_id=default")));
});

for (const row of [{ id: "legacy", title: "Legacy" }, { ...owned, own_notes_available: false, can_send: false },
    { ...owned, can_send: undefined }, { ...owned, can_send: false, hat_status: "inactive" }, { ...owned, pending: true }]) {
    test(`non-owned/bound state stays read-only: ${JSON.stringify(row)}`, async () => {
        const f = fixture();
        f.onlyList([hat], [row]);
        await f.context.loadConversations();
        assert.equal(f.state().canSend, false);
        assert.equal(f.get("chat").textInput.disabled, true);
        assert.equal(f.get("den-model-select").disabled, true);
        assert.equal(f.requests.length, row.hat_id && !row.pending ? 2 : 1);
        if (row.hat_id && !row.pending) assert.match(f.get("den-model-select").title, /test\/model/);
    });
}

test("real zero-hat legacy history remains readable but can never be converted to an executable chat", async () => {
    const f = fixture("?conversation_id=legacy");
    f.onlyList([], [{ id: "legacy", title: "Old main chat" }]);
    await f.context.loadConversations();
    assert.match(f.get("den-conv-current").textContent, /Legacy \(read-only\)/);
    assert.equal(f.get("den-legacy-conversation-notice").hidden, false);
    f.context.reply = async (url) => {
        assert.ok(url.startsWith("/v1/chat/history?"));
        return response({ messages: [{ role: "user", text: "Earlier message" }], has_more: false });
    };
    const history = await f.context.loadDenHistory(0);
    assert.equal(history[0].text, "Earlier message");
    assert.equal(f.state().canSend, false);
    assert.equal(f.newControls().button.disabled, true);
});

test("inaccessible explicit history does not silently select, inspect or convert another conversation", async () => {
    const f = fixture("?conversation_id=inaccessible");
    f.onlyList([hat], [owned]);
    await f.context.loadConversations();
    assert.equal(f.context.CONVERSATION_ID, "");
    assert.equal(f.location.search, "?conversation_id=inaccessible");
    assert.match(f.get("err").textContent, /conversation is unavailable/);
    await f.context.loadDenHistory(0);
    assert.equal(f.requests.length, 1);
    assert.equal(f.state().canSend, false);
});

for (const [title, status, body, type] of [
    ["legacy HTML403", 403, "<!doctype html><pre>https://user:pass@api.test?key=private</pre>", "text/html"],
    ["proxy failure", 502, "upstream password=private at https://api.test", "text/plain"],
    ["JSON business failure", 403, { error: "Hat access was revoked. Choose another hat." }, "application/json"],
]) {
    test(`list and history failures are visible, safe and retryable: ${title}`, async () => {
        const f = fixture("?conversation_id=" + owned.id);
        f.context.reply = async () => response(body, status, type, "REF-exact");
        await f.context.loadConversations();
        assert.equal(f.context.DEN_CONVERSATIONS.length, 0);
        assert.equal(f.state().canSend, false);
        assert.equal(f.location.search, "?conversation_id=" + owned.id);
        assert.match(f.get("err").textContent, /Reference: REF-exact/);
        assert.doesNotMatch(f.get("err").textContent, /<!|<pre|https:|private|password/);
        if (typeof body === "object") assert.match(f.get("err").textContent, /Hat access was revoked/);
        assert.equal(f.requests.length, 1);
        f.context.reply = async (url) => url.startsWith("/v1/chat/conversations?") ? listResponse([hat], [owned], owned.id) :
            url.startsWith("/v1/chat/model?") ? response({ effective_model: "test/model" }) : response(body, status, type, "REF-exact");
        await f.context.loadConversations();
        assert.equal(f.state().canSend, true);
        await f.context.loadDenHistory(0);
        assert.match(f.get("err").textContent, /Reference: REF-exact/);
        assert.equal(f.get("err").hidden, false);
        assert.doesNotMatch(f.get("err").textContent, /<!|<pre|https:|private|password/);
    });
}

test("New chat failure restores the explicit choice for retry and never fabricates a row", async () => {
    const f = fixture();
    f.onlyList([hat]);
    await f.context.loadConversations();
    const { select, button } = f.newControls();
    select.value = hat.id; select.onchange();
    f.context.reply = async () => response("<html>password=private</html>", 403, "text/html");
    await button.onclick();
    assert.equal(button.disabled, false);
    assert.equal(select.value, hat.id);
    assert.equal(f.context.DEN_CONVERSATIONS.length, 0);
    assert.match(f.get("err").textContent, /HTTP 403.*Reference: header-reference/s);
    assert.doesNotMatch(f.get("err").textContent, /<html>|private/);
    f.context.reply = async () => { throw new Error("https://user:password@api.test/?token=private"); };
    await button.onclick();
    assert.equal(button.disabled, false);
    assert.match(f.get("err").textContent, /Check your connection and retry/);
    assert.doesNotMatch(f.get("err").textContent, /https:|password|private/);
});

test("a failed refresh freezes a stale owned row; recovery re-enables it without a default GET", async () => {
    const f = fixture();
    f.context.reply = async (url) => url.startsWith("/v1/chat/conversations?") ? listResponse([hat], [owned], owned.id) : response({ effective_model: "test/model" });
    await f.context.loadConversations();
    assert.equal(f.state().canSend, true);
    f.context.reply = async () => { throw new Error("fetch https://user:pass@host"); };
    await f.context.loadConversations();
    assert.equal(f.context.CONVERSATION_ID, owned.id);
    assert.equal(f.context.DEN_CONVERSATIONS[0].id, owned.id);
    assert.equal(f.state().canSend, false);
    assert.equal(f.get("chat").textInput.disabled, true);
    const count = f.requests.length;
    await f.context.loadDenHistory(0);
    await f.context.loadConversationModel();
    assert.equal(f.requests.length, count);
});

test("stale list results cannot overwrite a newer explicit refresh", async () => {
    const f = fixture();
    const old = deferred();
    let calls = 0;
    f.context.reply = async () => ++calls === 1 ? old.promise : listResponse([hat], []);
    const first = f.context.loadConversations();
    await f.context.loadConversations();
    old.resolve(listResponse([], [{ id: "default", title: "Legacy", can_send: false }]));
    await first;
    assert.equal(f.context.DEN_HATS[0].id, hat.id);
    assert.equal(f.context.DEN_CONVERSATIONS.length, 0);
    assert.equal(f.state().canSend, false);
});

test("conversation PATCH uses the same controlled JSON/HTML boundary", async () => {
    const f = fixture();
    for (const [body, type] of [["<html>private server trace</html>", "text/html"],
        [{ error: "This conversation cannot be renamed" }, "application/json"]]) {
        f.context.reply = async () => response(body, 403, type, "PATCH-ref");
        await assert.rejects(f.context.patchConversation(owned.id, { title: "Renamed" }), (error) => {
            assert.equal(error.denChatControlled, true);
            assert.match(error.message, /Reference: PATCH-ref/);
            assert.doesNotMatch(error.message, /<html>|private|server trace/);
            if (typeof body === "object") assert.match(error.message, /cannot be renamed/);
            return true;
        });
    }
});

test("list, history, conversation PATCH and New chat preserve login-required handling", async () => {
    for (const operation of ["list", "history", "patch", "new"]) {
        const f = fixture("?conversation_id=" + owned.id);
        f.context.reply = async (url) => url.startsWith("/v1/chat/conversations?") ?
            listResponse([hat], [owned], owned.id) : response({ effective_model: "test/model" });
        await f.context.loadConversations();
        f.context.reply = async () => response("<html>private login</html>", 401, "text/html");
        if (operation === "list") await f.context.loadConversations();
        if (operation === "history") await f.context.loadDenHistory(0);
        if (operation === "patch") await f.context.patchConversation(owned.id, { title: "Renamed" });
        if (operation === "new") {
            const { select, button } = f.newControls();
            select.value = hat.id; select.onchange(); await button.onclick();
        }
        assert.match(f.location.href, /^\/login\?next=/);
        assert.doesNotMatch(f.get("err").textContent, /<html>|private login/);
        assert.equal(f.context.DEN_CONVERSATIONS.length, 1);
    }
});

async function readyFixture() {
    const f = fixture();
    f.onlyList([hat], [owned, { ...owned, id: "conv-other", title: "Other chat" }]);
    await f.context.loadConversations();
    await settle();
    return f;
}

for (const afterPost of [false, true]) {
    test(`late create ${afterPost ? "list" : "POST"} completion cannot activate/remount over a later selected chat or draft`, async () => {
        const f = await readyFixture();
        const old = deferred();
        f.context.reply = async (url, init) => {
            if (init.method === "POST") return afterPost ? response({ id: "conv-created" }) : old.promise;
            if (url.startsWith("/v1/chat/conversations?")) return old.promise;
            return response({ effective_model: "test/model" });
        };
        const { select, button } = f.newControls();
        select.value = hat.id; select.onchange();
        const creation = button.onclick();
        await settle();
        f.context.setConversation("conv-other", true);
        await settle();
        f.draft("Keep the newer draft");
        const remounts = f.remounts();
        old.resolve(afterPost ? listResponse([hat], [{ ...owned, id: "conv-created" }], "conv-created") : response({ id: "conv-created" }));
        await creation;
        await settle();
        assert.equal(f.context.CONVERSATION_ID, "conv-other");
        assert.equal(f.context.PREFERRED_CONVERSATION_ID, "conv-other");
        assert.match(f.location.search, /conversation_id=conv-other/);
        assert.equal(f.get("chat").draft, "Keep the newer draft");
        assert.equal(f.remounts(), remounts);
        assert.equal(f.context.DEN_CONVERSATIONS.some((row) => row.id === "conv-created"), false);
    });
}

test("typing a draft during create intent prevents activation, even without a selection switch", async () => {
    const f = await readyFixture();
    const post = deferred();
    f.context.reply = async () => post.promise;
    const { select, button } = f.newControls();
    select.value = hat.id; select.onchange();
    const creation = button.onclick();
    f.draft("Keep this draft");
    const remounts = f.remounts();
    post.resolve(response({ id: "conv-created" }));
    await creation;
    assert.equal(f.context.CONVERSATION_ID, owned.id);
    assert.equal(f.get("chat").draft, "Keep this draft");
    assert.equal(f.remounts(), remounts);
    assert.equal(button.disabled, false);
});

test("the actual refresh caller cannot remount after a newer selection or a same-selection draft edit", async () => {
    for (const switchSelection of [false, true]) {
        const f = await readyFixture();
        await f.context.boot();
        const old = deferred();
        f.context.reply = async (url) => url.startsWith("/v1/chat/conversations?") ? old.promise : response({ effective_model: "test/model" });
        const refresh = f.get("den-chats-refresh").listeners.click();
        if (switchSelection) f.context.setConversation("conv-other", true);
        await settle();
        f.draft("Draft during refresh");
        const remounts = f.remounts();
        old.resolve(listResponse([hat], [owned, { ...owned, id: "conv-other" }], owned.id));
        await refresh;
        await settle();
        assert.equal(f.context.CONVERSATION_ID, switchSelection ? "conv-other" : owned.id);
        assert.equal(f.get("chat").draft, "Draft during refresh");
        assert.equal(f.remounts(), remounts);
    }
});

test("a newer refresh supersedes every older caller effect, not just its list mutation", async () => {
    const f = await readyFixture();
    const older = deferred(), newer = deferred();
    let lists = 0;
    f.context.reply = async (url) => url.startsWith("/v1/chat/conversations?") ? (++lists === 1 ? older.promise : newer.promise) : response({ effective_model: "test/model" });
    const first = f.context.loadConversations({ remount: true });
    const second = f.context.loadConversations({ remount: true });
    newer.resolve(listResponse([hat], [owned], owned.id));
    await second;
    await settle();
    f.draft("Draft after latest refresh");
    const remounts = f.remounts();
    older.resolve(listResponse([hat], [owned], owned.id));
    assert.equal(await first, null);
    assert.equal(f.remounts(), remounts);
    assert.equal(f.get("chat").draft, "Draft after latest refresh");
});

test("list includes explicit history ID and adopts only the server's canonical display ID, including old requested rows", async () => {
    const f = fixture("?conversation_id=default");
    const canonical = { ...owned, id: "conv-canonical-old", can_send: false, hat_status: "inactive" };
    const recent = Array.from({ length: 100 }, (_, i) => ({ ...owned, id: "recent-" + i }));
    f.context.reply = async (url) => {
        const parsed = new URL(url, f.location.origin);
        if (parsed.pathname === "/v1/chat/conversations") {
            assert.equal(parsed.searchParams.get("bear_id"), "bear");
            assert.equal(parsed.searchParams.get("conversation_id"), "default");
            return listResponse([hat], [...recent, canonical], "default", canonical.id);
        }
        assert.equal(parsed.searchParams.get("conversation_id"), canonical.id);
        return response(parsed.pathname === "/v1/chat/model" ? { effective_model: "test/readonly" } : { messages: [{ role: "user", text: "Older history" }] });
    };
    await f.context.loadConversations();
    assert.equal(f.context.CONVERSATION_ID, canonical.id);
    assert.equal(f.context.PREFERRED_CONVERSATION_ID, canonical.id);
    assert.match(f.location.search, /conversation_id=conv-canonical-old/);
    assert.equal(f.state().canSend, false);
    assert.match(f.get("den-model-select").title, /test\/readonly/);
    assert.equal(f.get("den-model-select").disabled, true);
    assert.equal((await f.context.loadDenHistory(0))[0].text, "Older history");
});

test("an old/mismatched list contract fails closed instead of inferring selection or send permission", async () => {
    const f = fixture("?conversation_id=default");
    f.context.reply = async () => response({ hats: [hat], conversations: [owned] });
    await f.context.loadConversations();
    assert.equal(f.context.DEN_CONVERSATIONS_LOADED, false);
    assert.equal(f.state().canSend, false);
    assert.equal(f.context.CONVERSATION_ID, "");
    assert.match(f.get("err").textContent, /Chat list response is incompatible/);
    assert.equal(f.requests.length, 1);
});

test("error recovery is operation-owned: successful reads/list refresh do not erase a failed model mutation", async () => {
    const f = await readyFixture();
    f.context.reply = async (url, init) => init.method === "PATCH" ? response({ error: "Model mutation failed" }, 403) :
        url.startsWith("/v1/chat/history?") ? response({ error: "History read failed" }, 403) : response({ effective_model: "test/model" });
    await f.context.saveConversationModel("test/revoked");
    await f.context.loadDenHistory(0);
    assert.match(f.get("err").textContent, /Model mutation failed.*History read failed/s);
    f.context.reply = async (url) => url.startsWith("/v1/chat/conversations?") ? listResponse([hat], [owned], owned.id) :
        url.startsWith("/v1/chat/history?") ? response({ messages: [] }) : response({ effective_model: "test/model" });
    await f.context.loadDenHistory(0);
    assert.match(f.get("err").textContent, /Model mutation failed/);
    assert.doesNotMatch(f.get("err").textContent, /History read failed/);
    await f.context.loadConversations();
    assert.match(f.get("err").textContent, /Model mutation failed/);
    assert.equal(f.get("err").hidden, false);
    await f.context.saveConversationModel("auto");
    assert.equal(f.get("err").textContent, "");
    assert.equal(f.get("err").hidden, true);
});

test("model/list recovery and selection changes clear stale errors; same-ID ABA history failures stay fenced", async () => {
    const f = await readyFixture();
    f.context.reply = async () => response({ error: "Model read failed" }, 403);
    await f.context.loadConversationModel();
    assert.match(f.get("err").textContent, /Model read failed/);
    f.context.reply = async () => response({ effective_model: "test/model" });
    await f.context.loadConversationModel();
    assert.equal(f.get("err").hidden, true);
    const old = deferred();
    f.context.reply = async (url) => url.startsWith("/v1/chat/history?") ? old.promise : response({ effective_model: "test/model" });
    const history = f.context.loadDenHistory(0);
    f.context.showErr("Old selection error", f.context.chatOperations.begin("rename"));
    f.context.setConversation("conv-other", true);
    assert.equal(f.get("err").hidden, true);
    f.context.setConversation(owned.id, true);
    old.resolve(response({ error: "Old history failure" }, 403));
    await history;
    assert.equal(f.get("err").hidden, true);
    assert.equal(f.get("err").textContent, "");
    f.context.reply = async () => response({ error: "List failed" }, 403);
    await f.context.loadConversations();
    assert.match(f.get("err").textContent, /List failed/);
    f.onlyList([hat], [owned]);
    await f.context.loadConversations();
    assert.equal(f.get("err").hidden, true);
});

test("boot and inline Chat script are syntax checked by the existing Node VM harness", () => {
    assert.doesNotThrow(() => new vm.Script(inlineScript));
    assert.match(template, /textInput='\{"disabled":true\}'/);
    assert.match(template, /id="den-chats-refresh"/);
});
