//! Normalize Letta `GET /v1/agents/{id}` JSON shapes before reading fields.

use serde_json::Value;

/// Unwrap common envelopes (`data`, `agent`) so field reads match a flat agent object.
///
/// Some responses nest the agent under `agent` or `data` (or both); we peel up to a few levels.
pub fn unwrap_letta_agent_document(v: &Value) -> &Value {
    let mut cur = v;
    for _ in 0..5 {
        let Some(obj) = cur.as_object() else {
            break;
        };
        if let Some(inner) = obj.get("agent").filter(|x| x.is_object()) {
            cur = inner;
            continue;
        }
        if let Some(inner) = obj.get("data").filter(|x| x.is_object()) {
            cur = inner;
            continue;
        }
        break;
    }
    cur
}
