//! Conversation subsystem: canonical message persistence, typed write payloads,
//! event projection, and archived-conversation reads.

pub mod archived;
pub mod events;
pub mod message_types;
pub mod persistence;
pub mod viewer;

#[cfg(test)]
mod events_tests {
    include!("events_tests.rs");
}

// `persistence_idempotency_integration_tests` needs `bears::db` + flattened crate-root
// paths that only resolve in `den-runtime`, so it is wired in there (see
// `den-runtime/src/conversation/mod.rs`), not in this crate.
