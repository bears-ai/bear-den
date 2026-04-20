/**
 * Map Letta Code SDK stream messages to BEARS Deep Chat–compatible SSE payloads
 * (`data: {json}\\n\\n`) matching Letta server `assistant_message` / `reasoning_message` / `error_message`.
 */
import {
  extractStreamTextDelta,
  type SDKMessage,
} from "@letta-ai/letta-code-sdk";

export function sdkMessageToSseDataLine(msg: SDKMessage): string | null {
  switch (msg.type) {
    case "init":
    case "tool_call":
    case "tool_result":
    case "result":
    case "retry":
      return null;
    case "assistant":
      return JSON.stringify({
        message_type: "assistant_message",
        content: msg.content,
        id: msg.uuid,
      });
    case "reasoning":
      return JSON.stringify({
        message_type: "reasoning_message",
        reasoning: msg.content,
        content: msg.content,
        id: msg.uuid,
      });
    case "stream_event": {
      const ev = msg.event as Record<string, unknown>;
      // Letta-shaped chunks (message_type + content) — pass through for the web UI parser.
      if (typeof ev.message_type === "string") {
        return JSON.stringify(ev);
      }
      // Token / delta chunks (`delta.text` / `delta.reasoning`) have no `message_type`;
      // previously we dropped them entirely, so the chat showed nothing until history reload.
      const delta = extractStreamTextDelta(msg.event);
      if (!delta) {
        return null;
      }
      if (delta.kind === "assistant") {
        return JSON.stringify({
          message_type: "assistant_message",
          content: delta.text,
          id: msg.uuid,
        });
      }
      return JSON.stringify({
        message_type: "reasoning_message",
        reasoning: delta.text,
        content: delta.text,
        id: msg.uuid,
      });
    }
    case "error":
      return JSON.stringify({
        message_type: "error_message",
        message: msg.message,
        detail: msg.errorDetail,
        error_type: msg.errorCode,
      });
    default:
      return null;
  }
}

