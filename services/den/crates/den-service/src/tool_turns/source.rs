//! Cross-edge, cross-process exclusion keyed by canonical conversation UUID.
use den_core::DenError;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use sqlx::{postgres::PgTransactionManager, PgConnection, PgPool, TransactionManager};
use uuid::Uuid;

#[derive(Debug)]
pub struct ConversationTurnLease {
    conversation_id: Uuid,
    // Arc holders include live streams and waiting BearWire runs. Cancellation
    // cannot release the database lease while an old controller still owns it.
    _connection: Mutex<PgConnection>,
}
impl ConversationTurnLease {
    pub async fn acquire(pool: &PgPool, conversation_id: Uuid) -> Result<Arc<Self>, DenError> {
        // A waiting client must not reserve a pool slot needed to resolve its
        // own obligations. A detached connection owns only this transaction;
        // dropping the last Arc closes it and PostgreSQL releases the lock.
        let mut connection = pool.acquire().await?.detach();
        PgTransactionManager::begin(&mut connection, None).await?;
        let key = format!("den-conversation-turn:{conversation_id}");
        tokio::time::timeout(
            Duration::from_millis(500),
            sqlx::query!("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))", key)
                .execute(&mut connection),
        )
        .await
        .map_err(|_| {
            DenError::ValidationError(
                "a turn is already active for this canonical conversation".into(),
            )
        })??;
        Ok(Arc::new(Self {
            conversation_id,
            _connection: Mutex::new(connection),
        }))
    }
    pub fn conversation_id(&self) -> Uuid {
        self.conversation_id
    }
}
