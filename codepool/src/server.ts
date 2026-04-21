import { readFileSync, accessSync, constants as FsConstants } from "node:fs";
import express from "express";
import type { ConversationSessionPool } from "./pool.js";
import { sdkMessageToSseDataLine } from "./sse.js";
import { handleOpenAIChatCompletions } from "./openai.js";
import type { ChannelListenerRegistry } from "./channel-listeners.js";
import {
  parseBearRuntimePlan,
  type BearRuntimePlan,
} from "./provisioning/types.js";

const packageJson = JSON.parse(
  readFileSync(new URL("../package.json", import.meta.url), "utf8")
) as { version?: string };

export type ServerContext = {
  pool: ConversationSessionPool;
  channelListeners: ChannelListenerRegistry;
  internalToken: string;
};

function authMiddleware(internalToken: string) {
  return (
    req: express.Request,
    res: express.Response,
    next: express.NextFunction
  ) => {
    if (!internalToken) return next();
    const h = req.headers.authorization;
    const ok =
      h === `Bearer ${internalToken}` ||
      h === internalToken;
    if (!ok) {
      res.status(401).json({ error: "unauthorized" });
      return;
    }
    next();
  };
}

/**
 * POST /v1/conversations/:conversationId/messages — Letta-compatible streaming (SSE).
 */
export function attachRoutes(
  app: express.Application,
  ctx: ServerContext
): void {
  const guard = authMiddleware(ctx.internalToken);

  app.get("/health", (_req, res) => {
    const memRoot = process.env.BEAR_MEMORY_ROOT?.trim();
    let memory_root_ok: boolean | undefined;
    if (memRoot) {
      try {
        accessSync(memRoot, FsConstants.R_OK | FsConstants.W_OK);
        memory_root_ok = true;
      } catch {
        memory_root_ok = false;
      }
    }
    res.json({
      ok: true,
      service: "bear-codepool",
      bear_memory_root: memRoot ?? null,
      bear_memory_root_writable: memory_root_ok,
    });
  });

  app.get("/version", (_req, res) => {
    const gitSha = process.env.CODEPOOL_GIT_SHA?.trim() || "unknown";
    res.json({
      service: "bear-codepool",
      version: packageJson.version ?? "0.0.0",
      git_sha: gitSha,
    });
  });

  app.get("/internal/pool", guard, (_req, res) => {
    res.json({
      conversationHandlers: ctx.pool.stats(),
      channelListeners: ctx.channelListeners.stats(),
    });
  });

  app.post(
    "/v1/conversations/:conversationId/messages",
    express.json({ limit: "2mb" }),
    guard,
    async (req, res) => {
      const conversationId = req.params.conversationId ?? "";
      const body = req.body as {
        messages?: Array<{ role?: string; content?: unknown }>;
        streaming?: boolean;
        agent_id?: string;
        bear_id?: string;
        runtime_plan?: unknown;
      };
      const agentId = (body.agent_id as string | undefined)?.trim();
      if (!agentId) {
        res.status(400).json({ error: "agent_id is required" });
        return;
      }
      const bearId = (body.bear_id as string | undefined)?.trim();
      if (!bearId) {
        res.status(400).json({ error: "bear_id is required" });
        return;
      }
      const plan = parseBearRuntimePlan(body.runtime_plan);
      const userMsg = (body.messages ?? [])
        .filter((m) => m.role === "user")
        .map((m) =>
          typeof m.content === "string" ? m.content : JSON.stringify(m.content)
        )
        .pop();
      if (!userMsg?.trim()) {
        res.status(400).json({ error: "user message required" });
        return;
      }

      res.status(200);
      res.setHeader("Content-Type", "text/event-stream; charset=utf-8");
      res.setHeader("Cache-Control", "no-cache");
      res.setHeader("Connection", "keep-alive");

      try {
        for await (const msg of ctx.pool.streamUserMessage(
          agentId,
          conversationId,
          userMsg.trim(),
          { bearId, plan }
        )) {
          const line = sdkMessageToSseDataLine(msg);
          if (line) {
            res.write(`data: ${line}\n\n`);
          }
        }
        res.end();
      } catch (e) {
        const err = e instanceof Error ? e.message : String(e);
        res.write(
          `data: ${JSON.stringify({
            message_type: "error_message",
            message: err,
          })}\n\n`
        );
        res.end();
      }
    }
  );

  app.post(
    "/v1/chat/completions",
    express.json({ limit: "4mb" }),
    guard,
    (req, res) => {
      void handleOpenAIChatCompletions(req, res, ctx.pool);
    }
  );
}
