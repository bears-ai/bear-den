import { homedir } from "node:os";
import type {
  BearRuntimePlan,
  BearRuntimeProvisioner,
  EnsureResult,
  RuntimeProvisioningContext,
} from "./types.js";

/**
 * Upstream local memfs: canonical git state lives on the Letta server
 * (`LETTA_MEMFS_SERVICE_URL=local`); Letta Code mirrors under `$HOME/.letta` with
 * `LETTA_MEMFS_LOCAL=1`. No per-bear directory provisioning in codepool.
 */
export function createNoopMemoryProvisioner(): BearRuntimeProvisioner {
  return {
    async ensure(
      _ctx: RuntimeProvisioningContext,
      _plan: BearRuntimePlan
    ): Promise<EnsureResult> {
      const memoryDir = homedir();
      return {
        memoryDir,
        cwd: undefined,
        env: {},
        metadata: {},
      };
    },
  };
}
