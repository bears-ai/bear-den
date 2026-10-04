use super::*;
use crate::tools::{
    context::DenToolInvocationContext,
    entity::{self, EntityOps},
};
use crate::{DenError, RuntimeContextLabel};
use serde_json::{json, Value};
use uuid::Uuid;

struct NoStorage;

impl MemoryReviewStore for NoStorage {
    async fn find_observation(
        &self,
        _: Uuid,
        _: &str,
    ) -> Result<Option<ObservationRecord>, DenError> {
        panic!("retired helper touched observation storage")
    }
    async fn record_observation(
        &self,
        _: ObservationWriteRequest,
    ) -> Result<ObservationRecord, DenError> {
        panic!("retired helper wrote an observation")
    }
    async fn list_proposals(
        &self,
        _: Uuid,
        _: Option<MemoryProposalStatus>,
        _: i64,
    ) -> Result<Value, DenError> {
        panic!("retired helper listed proposals")
    }
    async fn get_proposal(&self, _: Uuid, _: Uuid) -> Result<Option<Value>, DenError> {
        panic!("retired helper read a proposal")
    }
    async fn resolve_proposal(&self, _: ResolveProposalRequest) -> Result<Value, DenError> {
        panic!("retired helper resolved a proposal")
    }
    async fn request_review(&self, _: RequestReviewRequest) -> Result<Value, DenError> {
        panic!("retired helper requested review")
    }
    async fn mark_memory_lifecycle(
        &self,
        _: MarkMemoryLifecycleRequest,
    ) -> Result<Value, DenError> {
        panic!("retired helper changed lifecycle")
    }
}

impl EntityOps for NoStorage {
    async fn browse_entities(
        &self,
        _: &DenToolInvocationContext,
        _: RuntimeContextLabel,
        _: Value,
    ) -> Result<Value, DenError> {
        panic!("retired helper browsed entities")
    }
    async fn resolve_entity(
        &self,
        _: &DenToolInvocationContext,
        _: RuntimeContextLabel,
        _: Value,
    ) -> Result<Value, DenError> {
        panic!("retired helper resolved an entity")
    }
    async fn link_memory_entity(
        &self,
        _: &DenToolInvocationContext,
        _: RuntimeContextLabel,
        _: Value,
    ) -> Result<Value, DenError> {
        panic!("retired helper wrote a relation")
    }
}

fn immediate<T>(future: impl std::future::Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("retired helper unexpectedly yielded"),
    }
}

#[test]
fn retired_direct_helpers_deny_all_labels_before_validation_or_storage() {
    immediate(async {
        check_retired_helpers().await;
    });
}

async fn check_retired_helpers() {
    let mut context: DenToolInvocationContext = serde_json::from_value(json!({
        "bear_id": Uuid::new_v4(), "bear_slug": "test", "binding_id": "claimed-worker",
        "user_id": 1, "membership_role": "owner", "conversation_id": "claimed-conversation",
        "session_id": "claimed-session"
    }))
    .unwrap();
    for label in [
        RuntimeContextLabel::ChannelConversation,
        RuntimeContextLabel::ArmatureConversation,
        RuntimeContextLabel::JobRun,
        RuntimeContextLabel::Curation,
        RuntimeContextLabel::Observation,
    ] {
        context.profile = Some(label);
        for arguments in [
            Value::Null,
            json!({"proposal_id": Uuid::new_v4(), "status": "rejected", "summary": "claimed observation"}),
        ] {
            let results = [
                list_memory_proposals(&NoStorage, &context, label, arguments.clone()).await,
                read_memory_proposal(&NoStorage, &context, label, arguments.clone()).await,
                resolve_memory_proposal(&NoStorage, &context, label, arguments.clone()).await,
                mark_memory_lifecycle(&NoStorage, &context, label, arguments.clone()).await,
                write_observation(&NoStorage, &context, label, arguments.clone()).await,
                entity::entity_merge(&NoStorage, &context, label, arguments.clone()).await,
                entity::entity_split(&NoStorage, &context, label, arguments.clone()).await,
                entity::entity_write_access_rule(&NoStorage, &context, label, arguments.clone())
                    .await,
                entity::entity_write_anchor(&NoStorage, &context, label, arguments).await,
            ];
            for result in results {
                assert!(
                    matches!(result, Err(DenError::NotFound(_))),
                    "{label:?}: {result:?}"
                );
            }
        }
    }
}
