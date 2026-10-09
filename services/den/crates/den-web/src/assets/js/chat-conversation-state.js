/* Derived UI state only; the conversation list and ownership flags remain server-owned. */
(function (root) {
    "use strict";

    function selection(rows, id, loaded, hats) {
        var row = rows.find(function (candidate) { return candidate.id === id; });
        var kind = !loaded ? "unavailable" : !row ? "unbound" : row.pending ? "pending" :
            !row.hat_id ? "legacy" : row.can_send !== true ? "read-only" : "ready";
        var prompts = {
            unavailable: "Chat is unavailable. Refresh chats to retry.",
            unbound: hats.length ? "Choose a hat under + New chat to start." :
                "Create a hat in Bear settings, then refresh chats and choose it under + New chat.",
            pending: "Create a hat-bound New chat before sending a message.",
            legacy: "This conversation is read-only. Choose a hat under + New chat to continue.",
            "read-only": "This conversation is read-only. Choose an owned chat or start a New chat with a hat.",
            ready: "Message…"
        };
        return {
                    row: row, kind: kind, canSend: kind === "ready", prompt: prompts[kind],
                    canReadHistory: !!(loaded && row && !row.pending),
                    canReadModel: !!(loaded && row && !row.pending && row.hat_id)
                };
    }

    function pick(rows, preferred, selectedId) {
        // Resolve an explicit history alias only through the server's authorized display ID.
        if (preferred) return typeof selectedId === "string" && rows.some(function (row) { return row.id === selectedId; }) ? selectedId : "";
        return rows.length ? rows[0].id : "";
    }

    root.DenChatConversationState = { selection: selection, pick: pick };
})(typeof globalThis !== "undefined" ? globalThis : window);
