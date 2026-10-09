import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

export const template = readFileSync(new URL("../src/templates/bear_chat.html", import.meta.url), "utf8");
export const inlineScript = template.match(/<script>([\s\S]*?)<\/script>/)[1];
export function code(name) {
    const result = template.match(new RegExp(`    function ${name}\\([^]*?\\n    \\}`));
    assert.ok(result, `missing Chat function ${name}`);
    return result[0];
}
export function loadAssets(context) {
    for (const name of ["chat-errors", "chat-conversation-state", "chat-operations"]) {
        vm.runInContext(readFileSync(new URL(`../src/assets/js/${name}.js`, import.meta.url), "utf8"), context);
    }
}
export function response(body, status = 200, type = "application/json", reference = "header-reference") {
    return new Response(typeof body === "string" ? body : JSON.stringify(body), {
        status, headers: { "content-type": type, "x-request-id": reference },
    });
}
export function listResponse(hats, conversations, requestedId = null, selectedId = requestedId) {
    return response({ hats, conversations, selected_conversation_id:
        conversations.some((row) => row.id === selectedId) ? selectedId : null });
}
export function deferred() {
    let resolve, reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
}
export const settle = async () => { await new Promise(setImmediate); await new Promise(setImmediate); };
