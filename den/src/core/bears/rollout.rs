//! Rollout notes for **existing** bears after deploying modern Letta + codepool memfs.
//!
//! 1. **Letta control plane:** Trigger a bear sync (any code path that calls [`super::sync::sync_bear_to_letta`])
//!    so each linked agent receives `include_base_tools: false`, `git_enabled: true`, filtered `tool_ids`,
//!    and default `letta_v1_agent` when the DB had an empty agent type. The sync also seeds
//!    [`runtime_plan`](super::model::Bear) when it was NULL.
//! 2. **Codepool runtime:** Mount a persistent volume at `BEAR_MEMORY_ROOT` and ensure the `git`
//!    package is installed in the codepool image. The first chat per bear runs local provisioning
//!    (git init or clone + seeds) under `${BEAR_MEMORY_ROOT}/{bear_id}/`.
