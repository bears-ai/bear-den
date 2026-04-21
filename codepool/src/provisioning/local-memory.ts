import {
  mkdirSync,
  existsSync,
  writeFileSync,
  accessSync,
  constants as FsConstants,
} from "node:fs";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import type {
  BearRuntimePlan,
  BearRuntimeProvisioner,
  EnsureResult,
  RuntimeProvisioningContext,
} from "./types.js";

function assertGitAvailable(): void {
  try {
    execFileSync("git", ["--version"], { stdio: "pipe" });
  } catch {
    throw new Error(
      "git is required for bear memory workspace (install git in the codepool image)"
    );
  }
}

function runGit(cwd: string, args: string[]): void {
  execFileSync("git", args, { cwd, stdio: "pipe" });
}

function seedDefaultMarkdown(memoryDir: string): void {
  const persona = join(memoryDir, "persona.md");
  if (!existsSync(persona)) {
    writeFileSync(
      persona,
      `---
description: "Who this bear is and how it collaborates (edit over time)."
limit: 50000
---

# Persona

This bear uses git-backed memory (memfs). Add notes here as you learn.
`,
      "utf8"
    );
  }
  const project = join(memoryDir, "project.md");
  if (!existsSync(project)) {
    writeFileSync(
      project,
      `---
description: "Project context and goals."
limit: 50000
---

# Project

`,
      "utf8"
    );
  }
}

/**
 * Local filesystem implementation: `${BEAR_MEMORY_ROOT}/{bearId}/` as the SDK working directory.
 */
export function createLocalFilesystemMemoryProvisioner(
  bearMemoryRoot: string
): BearRuntimeProvisioner {
  return {
    async ensure(
      ctx: RuntimeProvisioningContext,
      plan: BearRuntimePlan
    ): Promise<EnsureResult> {
      const root = bearMemoryRoot.trim();
      if (!root) {
        throw new Error("BEAR_MEMORY_ROOT is not set");
      }
      const memoryDir = join(root, ctx.bearId);
      mkdirSync(memoryDir, { recursive: true });
      try {
        accessSync(memoryDir, FsConstants.R_OK | FsConstants.W_OK);
      } catch (e) {
        throw new Error(
          `memory dir not writable: ${memoryDir}: ${e instanceof Error ? e.message : String(e)}`
        );
      }

      const gitDir = join(memoryDir, ".git");
      assertGitAvailable();

      if (!existsSync(gitDir)) {
        const remote = plan.memory.git_remote?.trim();
        if (remote) {
          runGit(memoryDir, [
            "clone",
            "--branch",
            plan.memory.git_ref || "main",
            "--depth",
            "1",
            remote,
            ".",
          ]);
        } else {
          runGit(memoryDir, ["init"]);
          seedDefaultMarkdown(memoryDir);
          runGit(memoryDir, ["add", "."]);
          try {
            runGit(memoryDir, [
              "commit",
              "-m",
              "chore: seed bear memory workspace",
            ]);
          } catch {
            /* empty commit ok */
          }
        }
      } else {
        runGit(memoryDir, ["rev-parse", "--is-inside-work-tree"]);
        seedDefaultMarkdown(memoryDir);
      }

      return {
        memoryDir,
        cwd: memoryDir,
        env: {},
        metadata: {
          bear_id: ctx.bearId,
          agent_id: ctx.agentId,
          template: plan.memory.seed_template,
        },
      };
    },
  };
}
