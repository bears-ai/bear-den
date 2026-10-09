import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { loadAssets } from "./chat-test-support.mjs";

function fixture() {
    const context = vm.createContext({});
    loadAssets(context);
    let selection = "a", error = "";
    const operations = context.DenChatOperations({ selection: () => selection, renderError: (message) => { error = message; } });
    return { operations, error: () => error, select(id) { selection = id; operations.selectionChanged(); } };
}

test("operation revisions fence earlier successes and failures, including A -> B -> A selections", () => {
    const f = fixture();
    const first = f.operations.begin("list");
    const latest = f.operations.begin("list");
    assert.equal(f.operations.current(first), false);
    assert.equal(f.operations.current(latest), true);
    f.operations.fail(first, "stale list error");
    assert.equal(f.error(), "");
    f.select("b"); f.select("a");
    assert.equal(f.operations.current(latest), false);
});

test("a draft edit fences remounts without suppressing same-selection request errors", () => {
    const f = fixture();
    const token = f.operations.begin("create");
    f.operations.draftChanged();
    assert.equal(f.operations.current(token), true);
    assert.equal(f.operations.current(token, true), false);
    f.operations.fail(token, "Could not create chat");
    assert.equal(f.error(), "Could not create chat");
});

test("recovery clears only that operation's error and never hides an unrelated failed mutation", () => {
    const f = fixture();
    f.operations.fail(f.operations.begin("model-write"), "Failed model mutation");
    const read = f.operations.begin("model-read");
    f.operations.fail(read, "Failed model read");
    f.operations.recover(f.operations.begin("model-read"));
    assert.equal(f.error(), "Failed model mutation");
    f.operations.recover(f.operations.begin("list"));
    assert.equal(f.error(), "Failed model mutation");
    f.operations.fail(read, "Late read error");
    assert.equal(f.error(), "Failed model mutation");
    f.operations.recover(f.operations.begin("model-write"));
    assert.equal(f.error(), "");
});

test("switching selection clears old errors, and retargeting retains the captured draft fence", () => {
    const f = fixture();
    const token = f.operations.begin("list");
    f.operations.fail(token, "Old selection error");
    f.select("b");
    assert.equal(f.error(), "");
    const adopted = f.operations.retarget(token);
    assert.equal(f.operations.current(adopted, true), true);
    f.operations.draftChanged();
    assert.equal(f.operations.current(adopted, true), false);
});
