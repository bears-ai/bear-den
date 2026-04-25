import type { BearRuntimeProvisioner } from "./types.js";
import { createNoopMemoryProvisioner } from "./noop.js";

export * from "./types.js";
export { createNoopMemoryProvisioner } from "./noop.js";

/**
 * `BEAR_RUNTIME_PROVISIONER=local` (default) — canonical memfs is on the Letta volume; set
 * `LETTA_MEMFS_SERVICE_URL` on the server (e.g. `http://bears-memfs-manager:8285`). Letta Code uses
 * `$HOME/.letta` with `LETTA_MEMFS_LOCAL=1`.
 * Future: `http` — external provisioning service.
 */
export function createBearRuntimeProvisionerFromEnv(): BearRuntimeProvisioner {
  const mode = (process.env.BEAR_RUNTIME_PROVISIONER ?? "local")
    .trim()
    .toLowerCase();
  if (mode === "http") {
    throw new Error(
      "BEAR_RUNTIME_PROVISIONER=http is not implemented yet; use local or unset"
    );
  }
  return createNoopMemoryProvisioner();
}
