import { homedir } from "node:os";
import { join } from "node:path";
import type { BearRuntimeProvisioner } from "./types.js";
import { createLocalFilesystemMemoryProvisioner } from "./local-memory.js";

export * from "./types.js";
export { createLocalFilesystemMemoryProvisioner } from "./local-memory.js";

function joinDefaultDataDir(sub: string): string {
  return join(homedir(), ".cache", "bear-codepool", sub);
}

/**
 * `BEAR_RUNTIME_PROVISIONER=local` (default) — in-process filesystem + git.
 * Future: `http` — call out to an external provisioning service with the same JSON contract.
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
  const root =
    process.env.BEAR_MEMORY_ROOT?.trim() ||
    joinDefaultDataDir("bear-memory");
  return createLocalFilesystemMemoryProvisioner(root);
}
