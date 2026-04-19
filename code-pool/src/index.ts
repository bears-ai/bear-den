import express from "express";
import { ConversationSessionPool } from "./pool.js";
import { attachRoutes } from "./server.js";
import { createChannelListenerRegistry } from "./channel-listeners.js";
import { verifyLettaReachableAtStartup } from "./letta-upstream.js";

async function main(): Promise<void> {
  const port = Number(process.env.PORT || "3030");
  const ttlSecs = Number(process.env.POOL_TTL_SECS || "600");
  const maxEntries = Number(process.env.POOL_MAX_ENTRIES || "256");

  process.env.LETTA_BASE_URL =
    process.env.LETTA_BASE_URL?.trim() ||
    process.env.LETTA_API_BASE_URL?.trim() ||
    "";
  const lettaBaseUrl = process.env.LETTA_BASE_URL;
  if (!lettaBaseUrl) {
    console.error(
      "bears-code-pool: LETTA_BASE_URL is not set — cannot start without Letta API"
    );
    process.exit(1);
  }

  const lettaApiKey = process.env.LETTA_API_KEY?.trim() ?? "";
  console.log("bears-code-pool: verifying Letta connectivity (GET /v1/health)…");
  await verifyLettaReachableAtStartup({
    baseUrl: lettaBaseUrl,
    apiKey: lettaApiKey,
  });
  console.log("bears-code-pool: Letta health check passed");

  const internalToken = process.env.CODE_POOL_INTERNAL_TOKEN?.trim() ?? "";

  const pool = new ConversationSessionPool({
    ttlSecs,
    maxEntries,
    includePartialMessages: true,
  });

  const channelListeners = createChannelListenerRegistry();

  const app = express();
  attachRoutes(app, { pool, channelListeners, internalToken });

  const server = app.listen(port, () => {
    console.log(`bears-code-pool listening on ${port}`);
  });

  function shutdown() {
    pool.shutdown();
    server.close(() => process.exit(0));
    setTimeout(() => process.exit(0), 5_000).unref();
  }

  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

void main().catch((err) => {
  console.error("bears-code-pool:", err);
  process.exit(1);
});
