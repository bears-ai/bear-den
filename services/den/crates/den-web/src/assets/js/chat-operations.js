/* Selection-scoped request fences and error ownership; no independent conversation state. */
(function (root) {
    "use strict";

    root.DenChatOperations = function (options) {
        var selectionRevision = 0;
        var draftRevision = 0;
        var revisions = Object.create(null);
        var errors = Object.create(null);

        function renderErrors() {
            options.renderError(Object.keys(errors).map(function (op) { return errors[op].message; }).join("\n\n"));
        }

        function retarget(token) {
            return Object.assign({}, token, { selection: options.selection(), selectionRevision: selectionRevision });
        }

        function begin(op) {
            revisions[op] = (revisions[op] || 0) + 1;
            return retarget({ op: op, revision: revisions[op], draftRevision: draftRevision });
        }

        function current(token, protectDraft) {
            return !!token && token.revision === revisions[token.op] &&
                token.selectionRevision === selectionRevision && token.selection === options.selection() &&
                (!protectDraft || token.draftRevision === draftRevision);
        }

        function fail(token, message) {
            if (!current(token)) return;
            errors[token.op] = { revision: token.revision, message: message };
            renderErrors();
        }

        function recover(token) {
            if (!current(token)) return;
            if (errors[token.op] && errors[token.op].revision <= token.revision) {
                delete errors[token.op];
                renderErrors();
            }
        }

        return {
            begin: begin, current: current, retarget: retarget, fail: fail, recover: recover,
            selectionChanged: function () {
                selectionRevision++;
                errors = Object.create(null);
                renderErrors();
            },
            draftChanged: function () { draftRevision++; }
        };
    };
})(typeof globalThis !== "undefined" ? globalThis : window);
