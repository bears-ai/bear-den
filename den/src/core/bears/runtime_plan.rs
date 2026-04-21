//! Default [`runtime_plan`](super::model::Bear) JSON for Den → codepool (versioned snapshot).
//!
//! With upstream local memfs (`LETTA_MEMFS_SERVICE_URL=local`), canonical git memory is on the
//! **Letta server**; optional `memory.git_remote` / `git_ref` remain for uncommon overrides only.

use serde_json::{json, Value};

pub const RUNTIME_PLAN_VERSION: u32 = 1;

/// Default plan when `bears.runtime_plan` is NULL (new bears, rollout).
pub fn default_runtime_plan() -> Value {
    json!({
        "version": RUNTIME_PLAN_VERSION,
        "memory": {
            "git_remote": null,
            "git_ref": "main",
            "seed_template": "default"
        }
    })
}

/// Merge DB column with defaults so codepool always receives a full v1 object.
pub fn effective_runtime_plan(stored: Option<&Value>) -> Value {
    let mut out = default_runtime_plan();
    let Some(st) = stored else {
        return out;
    };
    if let Some(v) = st.get("version") {
        out["version"] = v.clone();
    }
    if let (Some(m), Some(sm)) = (out.get_mut("memory"), st.get("memory")) {
        if let (Some(ma), Some(sa)) = (m.as_object_mut(), sm.as_object()) {
            for (k, v) in sa {
                ma.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_merges_memory_git_remote() {
        let stored = serde_json::json!({
            "memory": { "git_remote": "https://example.com/repo.git" }
        });
        let v = effective_runtime_plan(Some(&stored));
        assert_eq!(v["memory"]["git_remote"], "https://example.com/repo.git");
        assert_eq!(v["memory"]["git_ref"], "main");
        assert_eq!(v["version"], RUNTIME_PLAN_VERSION);
    }
}
