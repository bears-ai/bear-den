//! Rollout notes for **existing** bears after deploying modern Letta + codepool memfs.
//!
//! 1. **Letta control plane:** Trigger a bear sync (any code path that calls [`super::sync::sync_bear_to_letta`])
//!    so each linked agent receives `include_base_tools: false`, `git_enabled: true`, filtered `tool_ids`,
//!    and default `letta_v1_agent` when the DB had an empty agent type. The sync also seeds
//!    [`runtime_plan`](super::model::Bear) when it was NULL.
//! 2. **Git HTTP sidecar:** Root compose points Letta at `http://bear-memfs:8285` and `LETTA_MEMFS_LOCAL=1` on
//!    codepool. Canonical on-disk memory lives on **Letta’s volume** (`bear-letta-data`,
//!    `~/.letta/memfs/repository/...`); Letta Code mirrors under `/home/node/.letta` in the codepool container.
