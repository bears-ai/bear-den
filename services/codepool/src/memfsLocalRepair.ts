import fs from "node:fs";
import path from "node:path";
import { homedir } from "node:os";

/** Letta Code uses this as the on-disk worktree for the memfs remote clone. */
export function localAgentMemfsMemoryPath(agentId: string): string {
  return path.join(homedir(), ".letta", "agents", agentId, "memory");
}

/** Remove a possibly broken / partial git worktree; server is source of truth. */
export function removeLettaCodeAgentMemoryWorktree(agentId: string): void {
  const dir = localAgentMemfsMemoryPath(agentId);
  try {
    fs.rmSync(dir, { recursive: true, force: true });
  } catch {
    /* ignore */
  }
}

/** Heuristic for memfs / git state that can be repaired by re-cloning under ~/.letta. */
export function isLikelyLettaCodeMemfsCorruption(err: unknown): boolean {
  const s = err instanceof Error ? err.message : String(err);
  return (
    s.includes("Memory git sync") ||
    s.includes("pathspec '.' did not match") ||
    s.includes("no init message received")
  );
}
