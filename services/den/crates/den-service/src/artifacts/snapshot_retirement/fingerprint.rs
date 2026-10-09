use den_core::DenError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InventoryFingerprint(String);
impl InventoryFingerprint {
    pub(crate) fn of(material: &str) -> Self {
        Self(format!("{:x}", Sha256::digest(material.as_bytes())))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for InventoryFingerprint {
    type Error = DenError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(DenError::ValidationError(
                "Review a fresh preview before confirming.".into(),
            ));
        }
        Ok(Self(value))
    }
}
impl From<InventoryFingerprint> for String {
    fn from(value: InventoryFingerprint) -> Self {
        value.0
    }
}
