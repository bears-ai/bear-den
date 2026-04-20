# Letta Code local patch (`buildAgentInfoReminder`)

## Why this exists

Bear Codepool calls the Letta Code CLI via `@letta-ai/letta-code-sdk` with **`systemInfoReminder: false`**, which maps to **`--no-system-info-reminder`**. That flag correctly disables the **session-context** harness reminder because it flows through **`sessionContextReminderEnabled`**.

The **agent-info** reminder (“This is an automated message providing information about you…”, built in `buildAgentInfo`) was **not** gated on that flag. It only checked in-process state **`hasSentAgentInfo`**, so every **new CLI subprocess** (e.g. after Codepool restart or pool eviction) injected agent-info `<system-reminder>` blocks again, including for long-running conversations.

## What we changed (bundled `letta.js`)

In the vendored bundle, `buildAgentInfoReminder` (source: `src/reminders/engine.ts` in upstream) now returns early when harness context has disabled system-info reminders:

```ts
if (context3.sessionContextReminderEnabled === false) {
  return null;
}
```

This aligns agent-info behavior with **`buildSessionContextReminder`**, which already respects the same field.

## Where it lives in this repo

| Mechanism | Location |
|-----------|----------|
| Patch file | `patches/@letta-ai+letta-code+<VERSION>.patch` (today: `0.23.8`) |
| Re-apply on install | `package.json` → `"postinstall": "patch-package"` |
| Single CLI version | `package.json` → `"overrides": { "@letta-ai/letta-code": "..." }` so `@letta-ai/letta-code-sdk` does not resolve a **different** nested `letta-code` copy (otherwise the patch would not run for the subprocess the SDK spawns). |

## Upgrade checklist (bump `@letta-ai/letta-code` or `@letta-ai/letta-code-sdk`)

1. Update dependency versions in `package.json` as needed.
2. Run `rm -rf node_modules && npm install` (or CI-equivalent).
3. **If `patch-package` fails:** the upstream bundle line numbers or surrounding code shifted. Either:
   - **Prefer:** upstream merged the same fix — delete `patches/@letta-ai+letta-code+*.patch`, remove `postinstall` if no patches remain, and drop `overrides` only if deduplication is no longer required; **or**
   - Re-diff: edit `node_modules/@letta-ai/letta-code/letta.js` to reapply the guard, then run `npx patch-package @letta-ai/letta-code` to regenerate the patch file and commit.
4. Confirm resolution: `node -e "const r=require('module').createRequire('.../letta-code-sdk/dist/index.js'); console.log(r.resolve('@letta-ai/letta-code'))"` should point at the **same** patched `letta.js` you ship.
5. Run `npm run build` and exercise a chat round-trip after a Codepool restart.

---

## Contributing this fix upstream

Target repository: **`letta-ai/letta-code`** (the CLI package that contains `src/reminders/engine.ts` — not the published `letta.js` bundle; implement the change in TypeScript source).

Suggested steps:

1. **Open an issue** (optional but useful): describe that `--no-system-info-reminder` should suppress **agent-info** reminders on every subprocess resume, not only session-context; mention that `buildSessionContextReminder` already honors `sessionContextReminderEnabled` / the same option, while `buildAgentInfoReminder` only used `hasSentAgentInfo` in memory.
2. **Fork** https://github.com/letta-ai/letta-code and create a branch.
3. **Edit** `buildAgentInfoReminder` in `src/reminders/engine.ts` (or the current path for that function) to return `null` when system-info reminders are disabled — mirror the guard used for session-context, using whatever field name the shared reminder context uses for that flag (in the bundle it is `sessionContextReminderEnabled`).
4. **Run** their test / lint workflow (`npm test`, `bun test`, etc., per upstream `CONTRIBUTING` if present).
5. **Open a PR** referencing the issue; in the description, note that **consumers embedding the CLI** (e.g. `@letta-ai/letta-code-sdk` in a warm pool) spawn new processes where `hasSentAgentInfo` resets, so the flag must gate agent-info as well.

After upstream merges and releases, bump `@letta-ai/letta-code` here, remove this patch file if the release includes the fix, and re-run the upgrade checklist above.
