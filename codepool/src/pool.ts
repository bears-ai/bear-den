import { resumeSession, type Session } from "@letta-ai/letta-code-sdk";
import type { SDKMessage } from "@letta-ai/letta-code-sdk";
import type {
  BearRuntimePlan,
  BearRuntimeProvisioner,
  EnsureResult,
} from "./provisioning/types.js";

export type PoolKey = string;

export function makePoolKey(agentId: string, conversationId: string): PoolKey {
  return `${agentId}\n${conversationId}`;
}

function resumeTargetFor(agentId: string, conversationId: string): string {
  return conversationId === "default" ? agentId : conversationId;
}

type Entry = {
  session: Session;
  lastUsed: number;
};

export type StreamUserOpts = {
  bearId: string;
  plan: BearRuntimePlan;
};

export type ConversationPoolStats = {
  kind: "conversation";
  warm: number;
  maxEntries: number;
  ttlSecs: number;
  keys: string[];
};

/** Self-hosted Letta cannot use Letta Cloud git sync (`--memfs`); local cwd repos use `--no-memfs`. */
function useLettaCloudMemfsFromEnv(): boolean {
  const v = (process.env.CODEPOOL_USE_LETTA_CLOUD_MEMFS ?? "").trim().toLowerCase();
  return v === "1" || v === "true" || v === "yes";
}

export class ConversationSessionPool {
  private readonly map = new Map<PoolKey, Entry>();
  private readonly ttlMs: number;
  private readonly maxEntries: number;
  private readonly includePartialMessages: boolean;
  private readonly provisioner: BearRuntimeProvisioner;
  private readonly lettaCloudMemfs: boolean;
  /** One ensure result per bear (shared across conversations). */
  private readonly ensureByBear = new Map<string, EnsureResult>();
  private sweepTimer: ReturnType<typeof setInterval> | undefined;
  /** Exclusive lock per conversation (one active run at a time). */
  private tail = new Map<PoolKey, Promise<void>>();

  constructor(opts: {
    ttlSecs: number;
    maxEntries: number;
    includePartialMessages?: boolean;
    provisioner: BearRuntimeProvisioner;
    /** Only for Letta Cloud (`api.letta.com`); self-hosted must stay false. */
    lettaCloudMemfs?: boolean;
  }) {
    this.ttlMs = opts.ttlSecs * 1000;
    this.maxEntries = opts.maxEntries;
    this.includePartialMessages = opts.includePartialMessages ?? true;
    this.provisioner = opts.provisioner;
    this.lettaCloudMemfs = opts.lettaCloudMemfs ?? useLettaCloudMemfsFromEnv();
    if (this.lettaCloudMemfs) {
      console.warn(
        "bear-codepool: Letta Cloud memfs (--memfs) is enabled; use only with api.letta.com. Self-hosted Letta needs the default (no CODEPOOL_USE_LETTA_CLOUD_MEMFS)."
      );
    }
    this.sweepTimer = setInterval(() => this.evictIdle(), Math.min(60_000, this.ttlMs / 2));
    this.sweepTimer.unref?.();
  }

  stats(): ConversationPoolStats {
    return {
      kind: "conversation",
      warm: this.map.size,
      maxEntries: this.maxEntries,
      ttlSecs: Math.round(this.ttlMs / 1000),
      keys: [...this.map.keys()].map((k) => k.replace("\n", " / ")),
    };
  }

  shutdown(): void {
    if (this.sweepTimer) clearInterval(this.sweepTimer);
    for (const e of this.map.values()) {
      try {
        e.session.close();
      } catch {
        /* ignore */
      }
    }
    this.map.clear();
    this.ensureByBear.clear();
  }

  private async acquireLock(key: PoolKey): Promise<() => void> {
    const prev = this.tail.get(key) ?? Promise.resolve();
    let release!: () => void;
    const done = new Promise<void>((r) => {
      release = r;
    });
    this.tail.set(key, prev.then(() => done));
    await prev;
    return () => {
      release();
    };
  }

  private evictIdle(): void {
    const now = Date.now();
    for (const [key, entry] of this.map) {
      if (now - entry.lastUsed > this.ttlMs) {
        try {
          entry.session.close();
        } catch {
          /* ignore */
        }
        this.map.delete(key);
      }
    }
    while (this.map.size > this.maxEntries) {
      let oldestKey: PoolKey | undefined;
      let oldest = Infinity;
      for (const [key, entry] of this.map) {
        if (entry.lastUsed < oldest) {
          oldest = entry.lastUsed;
          oldestKey = key;
        }
      }
      if (!oldestKey) break;
      const e = this.map.get(oldestKey);
      if (e) {
        try {
          e.session.close();
        } catch {
          /* ignore */
        }
      }
      this.map.delete(oldestKey);
    }
  }

  private async ensureRuntime(
    bearId: string,
    agentId: string,
    conversationId: string,
    plan: BearRuntimePlan
  ): Promise<EnsureResult> {
    let e = this.ensureByBear.get(bearId);
    if (!e) {
      e = await this.provisioner.ensure(
        { bearId, agentId, conversationId },
        plan
      );
      this.ensureByBear.set(bearId, e);
    }
    return e;
  }

  private getOrCreateSession(
    agentId: string,
    conversationId: string,
    ensure: EnsureResult
  ): Session {
    const key = makePoolKey(agentId, conversationId);
    const rt = resumeTargetFor(agentId, conversationId);
    let entry = this.map.get(key);
    const now = Date.now();
    if (entry) {
      entry.lastUsed = now;
      return entry.session;
    }
    this.evictIdle();
    const session = resumeSession(rt, {
      includePartialMessages: this.includePartialMessages,
      systemInfoReminder: false,
      memfs: this.lettaCloudMemfs,
      cwd: ensure.cwd,
    } as Parameters<typeof resumeSession>[1]);
    this.map.set(key, { session, lastUsed: now });
    return session;
  }

  /**
   * Send user text and stream SDK messages (map to SSE in server).
   */
  async *streamUserMessage(
    agentId: string,
    conversationId: string,
    userText: string,
    opts: StreamUserOpts
  ): AsyncGenerator<SDKMessage, void, unknown> {
    const key = makePoolKey(agentId, conversationId);
    const unlock = await this.acquireLock(key);
    try {
      const ensure = await this.ensureRuntime(
        opts.bearId,
        agentId,
        conversationId,
        opts.plan
      );
      const session = this.getOrCreateSession(agentId, conversationId, ensure);
      await session.send(userText);
      for await (const msg of session.stream()) {
        yield msg as SDKMessage;
      }
    } finally {
      unlock();
    }
  }
}
