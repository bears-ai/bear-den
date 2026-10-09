#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../src/assets/js/task-selector.js", import.meta.url), "utf8");
const template = readFileSync(new URL("../src/templates/bear_chat.html", import.meta.url), "utf8");
const design = readFileSync(new URL("../src/templates/design/chat.html", import.meta.url), "utf8");
const names = ["current-task", "create-current-task", "clear-current-task", "task-feedback",
    "task-picker", "task-search", "task-status", "task-list", "task-create-form", "task-title",
    "task-create-submit", "task-confirm-form", "task-confirm-title", "task-confirm-submit",
    "task-confirm-cancel", "task-retry", "task-cancel"];

class Element {
    children = [];
    attributes = {};
    listeners = {};
    textContent = "";
    value = "";
    hidden = false;
    disabled = false;
    open = false;
    focused = false;
    addEventListener(name, listener) { this.listeners[name] = listener; }
    setAttribute(name, value) { this.attributes[name] = value; }
    appendChild(child) { this.children.push(child); }
    replaceChildren() { this.children = []; }
    focus() { this.focused = true; }
    fire(name, extra = {}) {
        if (name === "click" && this.disabled) return;
        return this.listeners[name]?.({ preventDefault() {}, ...extra });
    }
}

const response = (data, status = 200) => ({ ok: status < 400, status, json: async () => data });
function deferred() {
    let resolve, reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
}

function fixture() {
    const elements = Object.fromEntries(names.map((name) => [name, new Element()]));
    const document = {
        getElementById: (id) => elements[id.replace(/^den-/, "")],
        createElement: () => new Element(),
    };
    const requests = [];
    const store = {
        tasks: [
            { id: "task-one", title: "Prepare <launch>", status: "in_progress" },
            { id: "task-two", title: "Review launch plan", status: "pending" },
            { id: "task-three", title: "Waiting for access", status: "blocked" },
        ],
        current: "task-one",
    };
    let context = { id: "conv-tasks-a", readOnly: false };
    let override;
    const sandbox = vm.createContext({ URLSearchParams });
    vm.runInContext(source, sandbox);
    const controller = sandbox.DenTaskSelector({
        document, bearId: "bear-one", getContext: () => context,
        fetch: async (url, init) => {
            const path = new URL(url, "http://den.test").pathname;
            const body = init.body ? JSON.parse(init.body) : null;
            const call = { url, path, init, body };
            requests.push(call);
            if (override) {
                const result = override(call);
                if (result) return result;
            }
            if (!body) return response({ tasks: store.tasks.slice(), current_task_id: store.current });
            if (path.endsWith("/selection-request")) {
                const task = store.tasks.find((task) => task.id === body.task_id);
                return response({ task_id: task.id, title: task.title, confirmation_required: true });
            }
            if (path.endsWith("/select")) {
                store.current = body.task_id;
                return response({ ok: true, current_task_id: store.current });
            }
            if (path.endsWith("/clear")) {
                store.current = null;
                return response({ ok: true, current_task_id: null });
            }
            const task = { id: "task-new", title: body.title, status: "pending" };
            store.tasks.push(task);
            return response({ task });
        },
    });
    return {
        controller, elements, requests, store,
        switchTo(id, readOnly = false) { context = { id, readOnly }; controller.sync(); },
        override(fn) { override = fn; },
        labels: () => elements["task-list"].children.map((item) => item.children[0].textContent),
        posts: () => requests.filter((call) => call.body),
        selections: () => requests.filter((call) => call.path.endsWith("/select")),
    };
}

test("Current task opens a searchable authorized list with title, status and current marker", async () => {
    const f = fixture();
    await f.controller.open(false);
    assert.equal(f.elements["task-picker"].hidden, false);
    assert.equal(f.elements["task-picker"].open, true);
    assert.equal(f.elements["current-task"].attributes["aria-expanded"], "true");
    assert.deepEqual(f.labels(), ["Prepare <launch> · In progress · Current", "Review launch plan · Pending", "Waiting for access · Blocked"]);
    assert.equal(f.elements["task-list"].children[0].children[0].attributes["aria-current"], "true");
    assert.equal(f.elements["task-list"].children[2].children[0].disabled, true);
    assert.equal(f.elements["current-task"].textContent, "Task: Prepare <launch>");
    assert.equal(f.posts().length, 0);
    f.elements["task-search"].value = "pending";
    f.elements["task-search"].fire("input");
    assert.deepEqual(f.labels(), ["Review launch plan · Pending"]);
    f.elements["task-search"].value = "nothing matches";
    f.elements["task-search"].fire("input");
    assert.equal(f.elements["task-status"].textContent, "No matching tasks.");
});

test("only a listed actionable task can be previewed, and selection requires explicit confirmation", async () => {
    const f = fixture();
    await f.controller.open(false);
    await f.controller.requestSelection("arbitrary-uuid");
    await f.controller.requestSelection("task-three");
    await f.controller.requestSelection("task-one");
    assert.equal(f.posts().length, 0);
    await f.controller.requestSelection("task-two");
    assert.equal(f.posts()[0].path, "/v1/chat/current-task/selection-request");
    assert.deepEqual(f.posts()[0].body, { bear_id: "bear-one", conversation_id: "conv-tasks-a", task_id: "task-two" });
    assert.equal(f.selections().length, 0);
    assert.equal(f.elements["task-confirm-form"].hidden, false);
    assert.match(f.elements["task-confirm-title"].textContent, /Review launch plan/);
    await f.controller.confirm();
    assert.equal(f.selections().length, 1);
    assert.deepEqual(f.selections()[0].body, f.posts()[0].body);
    assert.equal(f.elements["current-task"].textContent, "Task: Review launch plan");
    assert.equal(f.elements["task-confirm-form"].hidden, true);
});

test("cancel confirmation and Escape/close never select a task", async () => {
    const f = fixture();
    await f.controller.open(false);
    await f.controller.requestSelection("task-two");
    f.elements["task-confirm-cancel"].fire("click");
    await f.controller.confirm();
    assert.equal(f.selections().length, 0);
    assert.equal(f.elements["task-confirm-form"].hidden, true);
    assert.equal(f.elements["task-picker"].open, true);
    await f.controller.requestSelection("task-two");
    f.elements["task-picker"].fire("keydown", { key: "Escape" });
    await f.controller.confirm();
    assert.equal(f.selections().length, 0);
    assert.equal(f.elements["task-picker"].hidden, true);
    assert.equal(f.elements["current-task"].focused, true);
});

test("cancelling an in-flight preview invalidates its eventual result", async () => {
    const f = fixture(), delayed = deferred();
    await f.controller.open(false);
    f.override((call) => call.path.endsWith("/selection-request") && delayed.promise);
    const pending = f.controller.requestSelection("task-two");
    f.controller.cancel(true);
    delayed.resolve(response({ task_id: "task-two", title: "Late title", confirmation_required: true }));
    await pending;
    await f.controller.confirm();
    assert.equal(f.selections().length, 0);
    assert.equal(f.elements["task-confirm-form"].hidden, true);
});

test("stale list results cannot overwrite a different conversation or an A → B → A visit", async () => {
    const f = fixture(), delayed = deferred();
    f.override(() => delayed.promise);
    const old = f.controller.open(false);
    f.switchTo("conv-tasks-b");
    f.switchTo("conv-tasks-a");
    f.override(null);
    f.store.tasks = [{ id: "fresh", title: "Fresh choice", status: "pending" }];
    f.store.current = null;
    await f.controller.load();
    delayed.resolve(response({ tasks: [{ id: "stale", title: "Old private task", status: "pending" }], current_task_id: "stale" }));
    await old;
    assert.deepEqual(f.labels(), ["Fresh choice · Pending"]);
    assert.equal(f.elements["current-task"].textContent, "Current task");
});

test("navigation rejects an in-flight preview and an already-visible confirmation", async () => {
    for (const inFlight of [true, false]) {
        const f = fixture(), delayed = deferred();
        await f.controller.open(false);
        if (inFlight) f.override((call) => call.path.endsWith("/selection-request") && delayed.promise);
        const pending = f.controller.requestSelection("task-two");
        if (!inFlight) await pending;
        f.switchTo("conv-tasks-b");
        delayed.resolve(response({ task_id: "task-two", title: "Old chat task", confirmation_required: true }));
        await pending;
        await f.controller.confirm();
        assert.equal(f.selections().length, 0);
        assert.equal(f.elements["task-confirm-form"].hidden, true);
        assert.equal(f.elements["current-task"].textContent, "Current task");
    }
});

test("a late select response stays scoped to the initiating chat and cannot reload/change the new chat", async () => {
    const f = fixture(), delayed = deferred();
    await f.controller.open(false);
    await f.controller.requestSelection("task-two");
    f.override((call) => call.path.endsWith("/select") && delayed.promise);
    const pending = f.controller.confirm();
    f.switchTo("conv-tasks-b");
    const before = f.requests.length;
    delayed.resolve(response({ ok: true, current_task_id: "task-two" }));
    await pending;
    assert.equal(f.requests.length, before);
    assert.equal(f.selections()[0].body.conversation_id, "conv-tasks-a");
    assert.equal(f.elements["current-task"].textContent, "Current task");
});

test("pending and history-only conversations disable every task mutation and issue no task requests", async () => {
    for (const id of ["new-pending", "conv-admin-history", "conv-legacy"]) {
        const f = fixture();
        await f.controller.open(false);
        await f.controller.requestSelection("task-two");
        f.elements["task-title"].value = "Do not create";
        f.switchTo(id, true);
        const before = f.requests.length;
        await f.controller.open(true);
        await f.controller.load();
        await f.controller.requestSelection("task-two");
        await f.controller.confirm();
        await f.controller.clear();
        await f.controller.create();
        assert.equal(f.requests.length, before);
        for (const name of ["current-task", "create-current-task", "clear-current-task", "task-confirm-submit"])
            assert.equal(f.elements[name].disabled, true);
        assert.match(f.elements["task-feedback"].textContent, /Read-only/);
    }
});

test("revoked task preview/select produces visible recovery feedback without changing current task", async () => {
    for (const path of ["/selection-request", "/select"]) {
        const f = fixture();
        await f.controller.open(false);
        f.override((call) => call.path.endsWith(path) && response({ message: "private internal task-id details" }, 404));
        await f.controller.requestSelection("task-two");
        await f.controller.confirm();
        assert.equal(f.store.current, "task-one");
        assert.equal(f.elements["task-confirm-form"].hidden, true);
        assert.equal(f.elements["task-feedback"].hidden, false);
        assert.match(f.elements["task-feedback"].textContent, /no longer available.*Refresh/);
        assert.ok(!f.elements["task-feedback"].textContent.includes("internal task-id"));
        assert.equal(f.elements["clear-current-task"].disabled, false);
        f.override(null);
        await f.controller.load();
        assert.equal(f.elements["task-feedback"].hidden, true);
    }
});

test("request failures expose Retry/close, clear stale choices, and never fall back to UUID entry", async () => {
    const f = fixture();
    await f.controller.open(false);
    f.override(() => ({ ok: false, status: 500, json: async () => { throw new Error("HTML response"); } }));
    await f.controller.load();
    assert.deepEqual(f.labels(), []);
    assert.equal(f.elements["task-feedback"].hidden, false);
    assert.match(f.elements["task-feedback"].textContent, /Could not load tasks.*Retry/);
    assert.equal(f.elements["create-current-task"].disabled, true);
    await f.controller.requestSelection("arbitrary-uuid");
    assert.equal(f.posts().length, 0);
    assert.doesNotMatch(source, /\bprompt\s*\(|Task UUID/);
    f.override(null);
    await f.controller.load();
    assert.equal(f.elements["task-feedback"].hidden, true);
});

test("stale request errors do not appear in the new chat", async () => {
    const f = fixture(), delayed = deferred();
    f.override(() => delayed.promise);
    const old = f.controller.open(false);
    f.switchTo("conv-tasks-b");
    delayed.reject(new Error("old private failure"));
    await old;
    assert.equal(f.elements["task-feedback"].hidden, true);
});

test("preview mismatches cannot confirm a different task", async () => {
    const f = fixture();
    await f.controller.open(false);
    f.override((call) => call.path.endsWith("/selection-request") && response({ task_id: "task-wrong", title: "Wrong", confirmation_required: true }));
    await f.controller.requestSelection("task-two");
    await f.controller.confirm();
    assert.equal(f.selections().length, 0);
    assert.match(f.elements["task-feedback"].textContent, /Could not confirm/);
});

test("inline creation preserves title on failure and uses the same explicit selection confirmation", async () => {
    const f = fixture();
    await f.controller.open(true);
    f.elements["task-title"].value = "  New readable task  ";
    await f.controller.create();
    assert.equal(f.posts()[0].body.title, "New readable task");
    assert.equal(f.posts()[1].path, "/v1/chat/current-task/selection-request");
    assert.equal(f.selections().length, 0);
    assert.match(f.elements["task-confirm-title"].textContent, /Task created.*Cancelling selection keeps the task/);
    f.controller.cancel(false);
    await f.controller.confirm();
    assert.equal(f.selections().length, 0);
    assert.equal(f.store.tasks.at(-1).title, "New readable task");
    await f.controller.open(true);
    f.override((call) => call.path === "/v1/chat/current-task" && call.body && response({ message: "Creation unavailable" }, 503));
    await f.controller.create();
    assert.equal(f.elements["task-title"].value, "  New readable task  ");
    assert.match(f.elements["task-feedback"].textContent, /Creation unavailable/);
});

test("missing created task ID and stale creation do not issue selection requests", async () => {
    const f = fixture();
    await f.controller.open(true);
    f.elements["task-title"].value = "Draft";
    f.override((call) => call.body && response({ task: {} }));
    await f.controller.create();
    assert.match(f.elements["task-feedback"].textContent, /Could not read the created task.*Refresh tasks/);
    assert.equal(f.posts().length, 1);
    const delayed = deferred();
    f.override((call) => call.body && delayed.promise);
    const pending = f.controller.create();
    f.switchTo("conv-tasks-b");
    delayed.resolve(response({ task: { id: "new-old-chat-task" } }));
    await pending;
    assert.equal(f.posts().length, 2);
    assert.equal(f.elements["task-confirm-form"].hidden, true);
});

test("unavailable current task can be cleared through the canonical endpoint", async () => {
    const f = fixture();
    f.store.tasks = [];
    await f.controller.open(false);
    assert.equal(f.elements["current-task"].textContent, "Current task unavailable");
    assert.equal(f.elements["clear-current-task"].hidden, false);
    await f.controller.clear();
    assert.equal(f.posts()[0].path, "/v1/chat/current-task/clear");
    assert.deepEqual(f.posts()[0].body, { bear_id: "bear-one", conversation_id: "conv-tasks-a" });
    assert.equal(f.elements["clear-current-task"].hidden, true);
});

test("hosted Chat and fixture use balanced semantic HTML outside scripts/templates", () => {
    const tracked = new Set(["button", "code", "details", "form", "label", "section", "select",
        "small", "strong", "summary", "table", "tbody", "td", "textarea", "th", "thead", "tr", "ul"]);
    for (const page of [template, design]) {
        const html = page.replace(/\{[\{%#][\s\S]*?[\}%#]\}/g, "")
            .replace(/<script\b[^>]*>[\s\S]*?<\/script>/g, "");
        const stack = [];
        for (const [, closing, tag] of html.matchAll(/<(\/?)([a-z][a-z0-9-]*)\b[^>]*>/gi)) {
            if (!tracked.has(tag)) continue;
            if (closing) assert.equal(stack.pop(), tag, `unexpected closing ${tag}`);
            else stack.push(tag);
        }
        assert.deepEqual(stack, []);
    }
});

test("hosted Chat and design fixture keep accessible task controls synced and no duplicate Overview", () => {
    for (const page of [template, design]) {
        for (const name of names) assert.ok(page.includes(`id="den-${name}"`), name);
        assert.match(page, /aria-controls="den-task-picker"/);
        assert.match(page, /<details id="den-task-picker" hidden>/);
        assert.match(page, /<label for="den-task-title">Task title<\/label>/);
        assert.match(page, /role="status" hidden/);
        assert.doesNotMatch(page, />Overview<\/a/);
        for (const script of page.matchAll(/<script>([\s\S]*?)<\/script>/g)) new vm.Script(script[1]);
    }
    const picker = (page) => page.match(/    <details id="den-task-picker" hidden>[\s\S]*?    <\/details>/)[0];
    assert.equal(picker(template), picker(design));
    assert.ok(template.includes("taskSelector.sync();"));
    assert.match(template, /var state = denConversationState\(\);\s*return \{\s*id: CONVERSATION_ID,\s*readOnly: !state.canSend/);
    assert.doesNotMatch(template, /Task UUID|window.prompt\("Task title/);
    assert.match(template, /Private conversation notes/);
    assert.match(template, /Only this conversation can use these unreviewed notes/);
    assert.match(template, /aria-expanded="false">Expand/);
    assert.match(template, /Read-only legacy chat.*New chat/);
});
