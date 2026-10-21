//! Human-authorized attachment of host-local accounts to portable people.
//!
//! This is a trusted service boundary, not a model tool or an authentication API.
//! The caller must hold fresh destination authorization (verified Bear Admin and,
//! for reconnection, verified destination Bear membership) through `commit_review`.
//! Browser input must never deserialize a review: keep it in server-side session state.
//! Imported labels and receipts are informational history, never destination grants.

mod commit;
mod origins;
mod receipts;
mod review;
mod types;

pub use commit::commit_review;
pub use origins::{record_export_origins, ExportUserLabel};
pub use receipts::{list_receipts, receipt_history};
pub use review::{list_identities, review_reconnection, review_revocation};
pub use types::{
    IdentityReceiptHistory, IdentityReconnectionReceipt, IdentityReconnectionReview,
    ReplacementConsent, UserIdentityOrigin, UserIdentitySummary,
};

#[cfg(test)]
mod tests;
