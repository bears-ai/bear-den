/**
 * Versioned snapshot from Den (`bears.runtime_plan`) for runtime provisioning.
 * Keep in sync with Den `effective_runtime_plan` / `default_runtime_plan`.
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
  /** Working directory for Letta Code SDK (`resumeSession`). */
  cwd: string;
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
