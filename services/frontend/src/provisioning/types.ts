/**
 * Versioned snapshot from Den (`bears.runtime_plan`). Optional overrides; default
 * upstream path uses Letta server memfs — `memory.git_remote` is only for
 * exceptional cases, not the primary BEARS + local memfs flow.
 */
export const RUNTIME_PLAN_VERSION = 1;

export type BearRuntimePlan = {
  version: number;
  memory: {
    git_remote: string | null;
    git_ref: string;
    seed_template: string;
  };
};

export type RuntimeProvisioningContext = {
  bearId: string;
  agentId: string;
  conversationId: string;
};

export type EnsureResult = {
  memoryDir: string;
  /** If set, passed to `resumeSession`; omit so Letta Code uses default memfs layout under `$HOME/.letta`. */
  cwd?: string;
  /** Extra env vars to merge for the SDK subprocess (if supported). */
  env: Record<string, string>;
  metadata: Record<string, string>;
};

export interface BearRuntimeProvisioner {
  ensure(
    ctx: RuntimeProvisioningContext,
    plan: BearRuntimePlan
  ): Promise<EnsureResult>;
}

export function parseBearRuntimePlan(raw: unknown): BearRuntimePlan {
  const o = raw as Record<string, unknown> | null | undefined;
  const mem = (o?.memory as Record<string, unknown>) ?? {};
  return {
    version: typeof o?.version === "number" ? o.version : RUNTIME_PLAN_VERSION,
    memory: {
      git_remote: typeof mem.git_remote === "string" ? mem.git_remote : null,
      git_ref: typeof mem.git_ref === "string" ? mem.git_ref : "main",
      seed_template:
        typeof mem.seed_template === "string" ? mem.seed_template : "default",
    },
  };
}
