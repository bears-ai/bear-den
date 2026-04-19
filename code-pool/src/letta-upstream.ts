/**
 * Fail fast at startup if the Letta HTTP API is unreachable (same check Den uses: GET /v1/health).
 */
export async function verifyLettaReachableAtStartup(options: {
  baseUrl: string;
  apiKey: string;
  timeoutMs?: number;
}): Promise<void> {
  const base = options.baseUrl.trim().replace(/\/$/, "");
  const url = `${base}/v1/health`;
  const headers: Record<string, string> = {};
  const key = options.apiKey.trim();
  if (key) {
    headers["Authorization"] = `Bearer ${key}`;
  }
  const timeoutMs = options.timeoutMs ?? 15_000;
  const controller = new AbortController();
  const t = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res = await fetch(url, { headers, signal: controller.signal });
    clearTimeout(t);
    if (!res.ok) {
      const body = await res.text();
      throw new Error(`HTTP ${res.status}: ${body}`);
    }
  } catch (e) {
    clearTimeout(t);
    const msg = e instanceof Error ? e.message : String(e);
    throw new Error(`cannot reach Letta at ${url} (${msg})`);
  }
}
