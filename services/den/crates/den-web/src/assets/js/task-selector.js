/* Hosted Chat enhancement; authorization and current-task state remain server-owned. */
(function (root) {
    "use strict";

    root.DenTaskSelector = function (options) {
        var document = options.document;
        var elements = {};
        ["current-task", "create-current-task", "clear-current-task", "task-feedback",
            "task-picker", "task-search", "task-status", "task-list", "task-create-form",
            "task-title", "task-create-submit", "task-confirm-form", "task-confirm-title",
            "task-confirm-submit", "task-confirm-cancel", "task-retry", "task-cancel"
        ].forEach(function (name) { elements[name] = document.getElementById("den-" + name); });
        var context = null;
        var revision = 0;
        var tasks = [];
        var current = null;
        var loaded = false;
        var busy = false;
        var preview = null;
        var error = "";
        var statusLabels = {
            pending: "Pending", in_progress: "In progress", blocked: "Blocked",
            completed: "Completed", cancelled: "Cancelled"
        };

        function feedback(message) {
            error = message || "";
            elements["task-feedback"].textContent = error;
            elements["task-feedback"].hidden = !error;
        }

        function selectable(task) {
            return task.status === "pending" || task.status === "in_progress";
        }

        function render() {
            var readOnly = !context || context.readOnly;
            var selected = tasks.find(function (task) { return task.id === current; });
            elements["current-task"].textContent = selected ? "Task: " + selected.title :
                current ? "Current task unavailable" : "Current task";
            elements["current-task"].disabled = readOnly;
            elements["current-task"].setAttribute("aria-expanded", String(elements["task-picker"].open));
            elements["create-current-task"].disabled = readOnly || !loaded || busy;
            elements["clear-current-task"].hidden = !current;
            elements["clear-current-task"].disabled = readOnly || !loaded || busy;
            elements["task-retry"].disabled = readOnly || busy;
            elements["task-retry"].textContent = error ? "Retry / refresh tasks" : "Refresh tasks";
            elements["task-search"].disabled = readOnly || busy;
            elements["task-create-submit"].disabled = readOnly || !loaded || busy;
            elements["task-confirm-submit"].disabled = readOnly || !preview || busy;
            elements["task-confirm-form"].hidden = !preview;
            elements["task-confirm-title"].textContent = preview ?
                (preview.created ? "Task created. " : "") +
                "Make “" + preview.title + "” the current task for this chat?" +
                (preview.created ? " Cancelling selection keeps the task." : "") : "";
            var query = elements["task-search"].value.trim().toLowerCase();
            var visible = tasks.filter(function (task) {
                return (task.title + " " + (statusLabels[task.status] || "Unavailable"))
                    .toLowerCase().includes(query);
            });
            elements["task-list"].replaceChildren();
            visible.forEach(function (task) {
                var item = document.createElement("li");
                var button = document.createElement("button");
                button.type = "button";
                button.className = "button-link";
                button.textContent = task.title + " · " + (statusLabels[task.status] || "Unavailable") +
                    (task.id === current ? " · Current" : "");
                button.disabled = readOnly || busy || !!preview || !selectable(task) || task.id === current;
                if (task.id === current) button.setAttribute("aria-current", "true");
                button.addEventListener("click", function () { requestSelection(task.id); });
                item.appendChild(button);
                elements["task-list"].appendChild(item);
            });
            elements["task-status"].textContent = busy ? "Loading…" :
                !loaded ? "" :
                current && !selected ? "The current task is no longer available. Clear it or choose another task." :
                !tasks.length ? "No tasks for this chat. Choose New task to create one." :
                !visible.length ? "No matching tasks." : "";
        }

        function cancel(close) {
            revision++;
            busy = false;
            preview = null;
            if (close) {
                elements["task-picker"].open = false;
                elements["task-picker"].hidden = true;
                elements["current-task"].focus();
            }
            render();
        }

        function sync() {
            var next = options.getContext();
            if (!context || next.id !== context.id || next.readOnly !== context.readOnly) {
                revision++;
                context = { id: next.id, readOnly: next.readOnly };
                tasks = [];
                current = null;
                loaded = false;
                busy = false;
                preview = null;
                elements["task-search"].value = "";
                elements["task-title"].value = "";
                elements["task-create-form"].hidden = true;
                elements["task-picker"].open = false;
                elements["task-picker"].hidden = true;
                feedback(next.readOnly ? (next.reason === undefined ?
                    "Read-only chat. Start a New chat to use tasks." : next.reason) : "");
                render();
            }
            return !context.readOnly;
        }

        function begin() {
            if (!sync() || busy) return null;
            var token = { id: context.id, revision: ++revision };
            busy = true;
            preview = null;
            feedback("");
            render();
            return token;
        }

        function active(token) {
            sync();
            return token && !context.readOnly && token.id === context.id && token.revision === revision;
        }

        function bodyFor(token, extra) {
            return Object.assign({ bear_id: options.bearId, conversation_id: token.id }, extra || {});
        }

        async function request(token, path, extra, fallback) {
            var url = "/v1/chat/current-task" + path;
            var init = { credentials: "same-origin", headers: { Accept: "application/json" } };
            if (extra === null) {
                url += "?" + new URLSearchParams(bodyFor(token)).toString();
            } else {
                init.method = "POST";
                init.headers["Content-Type"] = "application/json";
                init.body = JSON.stringify(bodyFor(token, extra));
            }
            var response = await options.fetch(url, init);
            var data = null;
            try { data = await response.json(); } catch (_) { /* Never display an HTML error page as text. */ }
            if (!response.ok) {
                var message = data && (data.message || data.error) || fallback;
                if (path === "/selection-request" || path === "/select") {
                    if (response.status === 400 || response.status === 404)
                        message = "Task is no longer available. Refresh tasks or clear the current task.";
                    if (response.status === 403)
                        message = "Task access changed. Refresh tasks or start a New chat.";
                }
                if (response.status === 401 || response.status === 405 || response.redirected)
                    message = "Sign in again, then retry task selection.";
                throw new Error(String(message));
            }
            if (!data) throw new Error(fallback + ". Retry / refresh tasks.");
            return data;
        }

        function failed(token, exception) {
            if (!active(token)) return;
            busy = false;
            preview = null;
            feedback(String(exception.message || exception) + " Retry / refresh tasks, or close the picker.");
            render();
        }

        async function load() {
            var token = begin();
            if (!token) return;
            loaded = false;
            tasks = [];
            render();
            try {
                var data = await request(token, "", null, "Could not load tasks");
                if (!active(token)) return;
                if (!Array.isArray(data.tasks)) throw new Error("Could not read task choices");
                tasks = data.tasks.filter(function (task) {
                    return task && typeof task.id === "string" && typeof task.title === "string";
                });
                current = data.current_task_id || null;
                loaded = true;
                busy = false;
                render();
            } catch (exception) { failed(token, exception); }
        }

        async function open(create) {
            if (!sync()) return;
            cancel(false);
            elements["task-picker"].hidden = false;
            elements["task-picker"].open = true;
            elements["task-create-form"].hidden = !create;
            render();
            (create ? elements["task-title"] : elements["task-search"]).focus();
            await load();
        }

        async function prepare(token, taskId, created) {
            var data = await request(token, "/selection-request", { task_id: taskId }, "Could not preview task");
            if (!active(token)) return;
            if (data.task_id !== taskId || data.confirmation_required !== true || typeof data.title !== "string")
                throw new Error("Could not confirm this task. Refresh tasks before selecting again.");
            preview = { task_id: taskId, title: data.title, token: token, created: !!created };
            busy = false;
            elements["task-create-form"].hidden = true;
            render();
            elements["task-confirm-submit"].focus();
        }

        async function requestSelection(taskId) {
            // Only a displayed, server-listed choice can enter the confirmation flow.
            var task = tasks.find(function (task) { return task.id === taskId; });
            if (!task || !selectable(task) || taskId === current || preview) return;
            var token = begin();
            if (!token) return;
            try { await prepare(token, taskId); } catch (exception) { failed(token, exception); }
        }

        async function confirm() {
            var selection = preview;
            if (!selection || busy || !active(selection.token)) return;
            busy = true;
            feedback("");
            render();
            try {
                await request(selection.token, "/select", { task_id: selection.task_id }, "Could not select task");
                if (!active(selection.token)) return;
                current = selection.task_id;
                busy = false;
                preview = null;
                await load();
            } catch (exception) { failed(selection.token, exception); }
        }

        async function clear() {
            if (!loaded || !current) return;
            var token = begin();
            if (!token) return;
            try {
                await request(token, "/clear", {}, "Could not clear current task");
                if (!active(token)) return;
                current = null;
                busy = false;
                await load();
            } catch (exception) { failed(token, exception); }
        }

        async function create() {
            var title = elements["task-title"].value.trim();
            if (!title || !loaded) return;
            var token = begin();
            if (!token) return;
            try {
                var data = await request(token, "", { title: title }, "Could not create task");
                if (!active(token)) return;
                if (!data.task || typeof data.task.id !== "string")
                    throw new Error("Could not read the created task. Refresh tasks before creating another");
                await prepare(token, data.task.id, true);
            } catch (exception) { failed(token, exception); }
        }

        elements["current-task"].addEventListener("click", function () { open(false); });
        elements["create-current-task"].addEventListener("click", function () { open(true); });
        elements["clear-current-task"].addEventListener("click", clear);
        elements["task-search"].addEventListener("input", render);
        elements["task-retry"].addEventListener("click", load);
        elements["task-create-form"].addEventListener("submit", function (event) { event.preventDefault(); create(); });
        elements["task-confirm-form"].addEventListener("submit", function (event) { event.preventDefault(); confirm(); });
        elements["task-confirm-cancel"].addEventListener("click", function () { cancel(false); elements["task-search"].focus(); });
        elements["task-cancel"].addEventListener("click", function () { cancel(true); });
        elements["task-picker"].addEventListener("keydown", function (event) {
            if (event.key === "Escape") { event.preventDefault(); cancel(true); }
        });
        elements["task-picker"].addEventListener("toggle", function () {
            if (!elements["task-picker"].open && !elements["task-picker"].hidden) cancel(true);
        });
        sync();
        return { sync: sync, load: load, open: open, requestSelection: requestSelection,
            confirm: confirm, cancel: cancel, clear: clear, create: create };
    };
})(globalThis);
