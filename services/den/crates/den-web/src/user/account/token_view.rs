use crate::core::armature_tokens::ArmatureTokenListRow;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Serialize)]
pub(super) enum TokenStatus {
    Active,
    Expired,
    Revoked,
}

#[derive(Serialize)]
pub(super) struct AccountTokenView {
    #[serde(flatten)]
    token: ArmatureTokenListRow,
    status: TokenStatus,
}

impl AccountTokenView {
    pub(super) fn at(token: ArmatureTokenListRow, now: OffsetDateTime) -> Self {
        let status = if token.revoked_at.is_some() {
            TokenStatus::Revoked
        } else if token.expires_at.is_some_and(|expiry| expiry <= now) {
            TokenStatus::Expired
        } else {
            TokenStatus::Active
        };
        Self { token, status }
    }
}
