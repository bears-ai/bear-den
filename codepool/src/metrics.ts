/**
 * Hand-rolled Prometheus text metrics (no prom-client dependency).
 */

let conversationMessagesRequests = 0;
let streamFinishedOk = 0;
let streamFinishedEmptyFallback = 0;
let streamFinishedError = 0;

export function recordConversationMessagesRequest(): void {
  conversationMessagesRequests += 1;
}

export function recordStreamFinishedOk(): void {
  streamFinishedOk += 1;
}

export function recordStreamFinishedEmptyFallback(): void {
  streamFinishedEmptyFallback += 1;
}

export function recordStreamFinishedError(): void {
  streamFinishedError += 1;
}

export function renderPrometheusText(): string {
  const lines: string[] = [
    "# HELP codepool_conversation_messages_requests_total POST /v1/conversations/:id/messages accepted (before streaming).",
    "# TYPE codepool_conversation_messages_requests_total counter",
    `codepool_conversation_messages_requests_total ${conversationMessagesRequests}`,
    "",
    "# HELP codepool_stream_finished_ok_total Streams that emitted at least one user-visible SSE line before end.",
    "# TYPE codepool_stream_finished_ok_total counter",
    `codepool_stream_finished_ok_total ${streamFinishedOk}`,
    "",
    "# HELP codepool_stream_finished_empty_fallback_total Streams with no visible output; synthetic error_message was appended.",
    "# TYPE codepool_stream_finished_empty_fallback_total counter",
    `codepool_stream_finished_empty_fallback_total ${streamFinishedEmptyFallback}`,
    "",
    "# HELP codepool_stream_finished_error_total Streams that ended in catch (exception before res.end).",
    "# TYPE codepool_stream_finished_error_total counter",
    `codepool_stream_finished_error_total ${streamFinishedError}`,
    "",
  ];
  return lines.join("\n");
}
