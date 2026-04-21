//! Rollout notes for **existing** bears after deploying modern Letta + codepool memfs.
//!
//! 1. **Letta control plane:** Trigger a bear sync (any code path that calls [`super::sync::sync_bear_to_letta`])
//!    so each linked agent receives `include_base_tools: false`, `git_enabled: true`, filtered `tool_ids`,
//!    and default `letta_v1_agent` when the DB had an empty agent type. The sync also seeds
//!    [`runtime_plan`](super::model::Bear) when it was NULL.
//! 2. **Upstream local memfs:** Set `LETTA_MEMFS_SERVICE_URL=local` on the Letta server and
//!    `LETTA_MEMFS_LOCAL=1` on codepool. Canonical git-backed memory lives on **Letta** (`bear-letta-data`);
//!    Letta Code mirrors under `/home/node/.letta` in the codepool container.
