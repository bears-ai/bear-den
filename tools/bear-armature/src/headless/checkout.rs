use anyhow::{anyhow, Context, Result};
use bearwire_protocol::session::ExpectedWorkSource;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

pub(super) struct WorkCheckout {
    pub source: ExpectedWorkSource,
    pub prompt: String,
    pub deadline: Duration,
}

#[derive(Deserialize)]
struct CheckoutResponse {
    ok: bool,
    work_run_id: Uuid,
    gate: CheckoutGate,
    execution_attempt_id: Option<Uuid>,
    execution_attempt_fence_epoch: Option<i64>,
    #[serde(default)]
    prompt: String,
    deadline_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum CheckoutGate {
    Allowed {
        binding: CheckoutBinding,
    },
    Rejected,
    #[serde(other)]
    Unsupported,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CheckoutBinding {
    WorkRun {
        work_run_id: Uuid,
    },
    #[serde(other)]
    Unsupported,
}

pub(super) fn decode(
    result: Value,
    expected_run_id: Uuid,
    deadline: Duration,
) -> Result<WorkCheckout> {
    let checkout: CheckoutResponse =
        serde_json::from_value(result).context("decode BearWire work.checkout result")?;
    if !checkout.ok {
        return Err(anyhow!("work.checkout denied Work startup (ok is false)"));
    }
    let CheckoutGate::Allowed {
        binding: CheckoutBinding::WorkRun { work_run_id },
    } = checkout.gate
    else {
        return Err(anyhow!(
            "work.checkout did not allow dispatch with a Work run binding"
        ));
    };
    if checkout.work_run_id != expected_run_id || work_run_id != expected_run_id {
        return Err(anyhow!(
            "work.checkout returned a different Work run: expected {expected_run_id}, returned {}, gate binding {work_run_id}",
            checkout.work_run_id
        ));
    }
    let execution_attempt_id = checkout
        .execution_attempt_id
        .filter(|id| !id.is_nil())
        .ok_or_else(|| anyhow!("work.checkout returned no valid execution attempt UUID"))?;
    let fence_epoch = checkout
        .execution_attempt_fence_epoch
        .filter(|epoch| *epoch > 0)
        .ok_or_else(|| {
            anyhow!("work.checkout requires a positive execution attempt fence epoch")
        })?;
    if checkout.prompt.trim().is_empty() {
        return Err(anyhow!("work.checkout returned no prompt"));
    }
    Ok(WorkCheckout {
        source: ExpectedWorkSource {
            work_run_id: checkout.work_run_id,
            execution_attempt_id,
            fence_epoch,
        },
        prompt: checkout.prompt,
        deadline: checkout
            .deadline_secs
            .map(|secs| Duration::from_secs(secs.max(30)))
            .unwrap_or(deadline)
            .min(deadline),
    })
}
