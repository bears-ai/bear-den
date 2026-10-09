/* Chat API boundaries: never display an unstructured response body or upstream diagnostics. */
(function (root) {
    "use strict";

    function requestRef(value) {
        // Keep a valid header byte-for-byte; do not repair, truncate or prefer a body reference.
        return typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/.test(value) ? value : "";
    }

    function apiMessage(value) {
        if (typeof value !== "string" || !value.trim()) return "";
        if (/<\/?[a-z][^>]*>|<!|&#?\w+;/i.test(value)) return "";
        // Expected JSON business messages remain intact, apart from credential-bearing evidence.
        return value
            .replace(/\b[a-z][a-z0-9+.-]*:\/\/[^\s<>"']+/gi, "[connection address withheld]")
            .replace(/\b(?:Bearer|Basic)\s+[A-Za-z0-9+/_=.:-]+/gi, "[credential withheld]")
            .replace(/(["'](?:api[_ -]?key|access[_ -]?token|authorization|password|client[_ -]?secret)["']\s*:\s*)(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')/gi, '$1"[credential withheld]"')
            .replace(/\b(?:api[_ -]?key|access[_ -]?token|authorization|password|client[_ -]?secret)\s*[:=]\s*[^\s,;]+/gi, "[credential withheld]")
            .replace(/\b(?:sk|rk|pk)-[A-Za-z0-9_-]{8,}\b/g, "[credential withheld]");
    }

    function literalText(value) {
        // Deep Chat parses text as Markdown. Escape its syntax, not the business message,
        // so links/images stay literal and header references display byte-for-byte.
        return String(value)
            .replace(/[\\`*_{}\[\]()!]/g, "\\$&")
            .replace(/^([ \t]*)([#>+-])/gm, "$1\\$2")
            .replace(/^([ \t]*\d+)\./gm, "$1\\.");
    }

    function format(title, detail, reference) {
        var parts = [title];
        if (detail) parts.push("", detail);
        var ref = requestRef(reference);
        if (ref) parts.push("", "Reference: " + ref);
        return parts.join("\n");
    }

    function recovery(status) {
        switch (status) {
            case 200: return "The service returned an unexpected response. Refresh and retry.";
            case 401: case 405: return "Sign in again, then retry.";
            case 403: return "Access was denied. Choose an accessible chat, or choose a hat under + New chat.";
            case 404: return "This chat is unavailable. Refresh chats or start a New chat with a hat.";
            case 409: return "The chat changed. Refresh chats and retry.";
            case 429: return "Too many requests. Wait a moment and retry.";
            default: return status >= 500 ? "The service is temporarily unavailable. Retry in a moment." :
                "The request could not be completed. Refresh chats and retry.";
        }
    }

    function parse(text, status, reference, title, contentType) {
        var detail = "";
        // Accept legacy JSON without a media type, but never JSON embedded in an HTML/proxy page.
        if (!contentType || /\bapplication\/(?:[\w.-]+\+)?json\b/i.test(contentType)) {
            try {
                var body = JSON.parse(text);
                if (body && !Array.isArray(body)) detail = apiMessage(body.error);
            } catch (_) { /* Non-JSON response bodies are deliberately not echoed. */ }
        }
        var statusText = Number.isInteger(status) && status > 0 ? "HTTP " + status + ". " : "";
        return format(title || "Request failed", detail || statusText + recovery(status), reference);
    }

    function responseError(response, title) {
        var reference = response.headers ? response.headers.get("x-request-id") : "";
        var contentType = response.headers ? response.headers.get("content-type") : "";
        return response.text().catch(function () { return ""; }).then(function (text) {
            return new Error(parse(text, response.status, reference, title, contentType));
        });
    }

    function failure(error, title) {
        // Only errors constructed at the HTTP boundary may carry server-provided text.
        return error && error.denChatControlled ? error.message :
            format(title || "Connection problem", "Could not connect to the service. Check your connection and retry.");
    }

    function rejectResponse(response, title) {
        return responseError(response, title).then(function (error) {
            error.denChatControlled = true;
            throw error;
        });
    }

    function readJson(response, title) {
        if (!response.ok) return rejectResponse(response, title);
        return response.json().catch(function () {
            var reference = response.headers ? response.headers.get("x-request-id") : "";
            var error = new Error(parse("", response.status, reference, title));
            error.denChatControlled = true;
            throw error;
        });
    }

    function requiresLogin(response, origin) {
        if (!response) return false;
        if (response.status === 401 || response.status === 405) return true;
        if (!response.redirected || !response.url) return false;
        try { return new URL(response.url, origin).pathname === "/login"; }
        catch (_) { return false; }
    }

    function runtimeError(inner, reference) {
        var lines = [];
        var kind = typeof inner.error_type === "string" && /^[a-z][a-z0-9_]{0,63}$/.test(inner.error_type) ? inner.error_type : "";
        if (kind) lines.push("[" + kind + "]");
        // `detail` can contain serialized provider responses. Never forward it, even after
        // redaction: credential patterns are defense-in-depth, not a diagnostic disclosure policy.
        var message = typeof inner.message === "string" && !/[\[{]/.test(inner.message) ? apiMessage(inner.message) : "";
        lines.push(message || "The assistant could not complete this reply.");
        // Retain actionable error codes, not provider bodies, headers or credential URLs.
        var upstream = inner.context && inner.context.upstream_error;
        if (upstream && typeof upstream.code === "string" && /^[A-Z][A-Z0-9_]{0,47}$/.test(upstream.code)) {
            lines.push("Upstream code: " + upstream.code);
        }
        return format(lines.join("\n\n"), "Retry or ask for help.", reference);
    }

    root.DenChatErrors = {
        requestRef: requestRef, apiMessage: apiMessage, literalText: literalText, format: format, parse: parse,
        rejectResponse: rejectResponse, readJson: readJson, failure: failure, requiresLogin: requiresLogin,
        runtimeError: runtimeError
    };
})(typeof globalThis !== "undefined" ? globalThis : window);
