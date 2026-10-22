//! Server-owned, value-free context dependencies. Locators never establish reuse authority.

use std::{collections::BTreeSet, fmt};

use den_core::{ids::BearId, DenError};
use den_memory::personal_facts::PersonalFactId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

const METADATA_FIELD: &str = "den_context_dependencies";
const WIRE_VERSION: u8 = 1;

/// Conservative dependency union, not an access grant or a personal-context payload.
/// Decode only through the metadata boundary; no model/client input may clear this set.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct ContextDependencies {
    personal_facts: BTreeSet<PersonalFactWire>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersonalFactWire {
    bear_id: Uuid,
    fact_id: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContextDependenciesWire {
    version: u8,
    personal_facts: Vec<PersonalFactWire>,
}

fn invalid_metadata() -> DenError {
    DenError::Authorization("Context dependency metadata is invalid; reuse is blocked.".into())
}

impl ContextDependencies {
    pub fn personal_fact(bear_id: BearId, fact_id: PersonalFactId) -> Self {
        Self {
            personal_facts: BTreeSet::from([PersonalFactWire {
                bear_id: bear_id.as_uuid(),
                fact_id: fact_id.0,
            }]),
        }
    }

    pub fn union_from(&mut self, other: &Self) {
        self.personal_facts
            .extend(other.personal_facts.iter().cloned());
    }

    pub fn is_empty(&self) -> bool {
        self.personal_facts.is_empty()
    }

    pub fn require_unrestricted_reuse(&self) -> Result<(), DenError> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(DenError::Authorization(
                "Protected context cannot be reused by this operation.".into(),
            ))
        }
    }

    /// Legacy null/object metadata without the reserved field remains unannotated.
    /// Present invalid envelopes fail closed, including sequence-shaped serde structs.
    pub fn from_metadata(metadata: &Value) -> Result<Self, DenError> {
        let object = match metadata {
            Value::Null => return Ok(Self::default()),
            Value::Object(object) => object,
            _ => return Err(invalid_metadata()),
        };
        let Some(envelope) = object.get(METADATA_FIELD) else {
            return Ok(Self::default());
        };
        let envelope_object = envelope.as_object().ok_or_else(invalid_metadata)?;
        let facts = envelope_object
            .get("personal_facts")
            .and_then(Value::as_array)
            .ok_or_else(invalid_metadata)?;
        if facts.iter().any(|fact| !fact.is_object()) {
            return Err(invalid_metadata());
        }
        let wire: ContextDependenciesWire =
            serde_json::from_value(envelope.clone()).map_err(|_| invalid_metadata())?;
        if wire.version != WIRE_VERSION {
            return Err(invalid_metadata());
        }
        Ok(Self {
            personal_facts: wire.personal_facts.into_iter().collect(),
        })
    }

    /// Union with existing protection without replacing unrelated metadata.
    /// Invalid metadata is rejected without mutating the destination.
    pub fn attach_to_metadata(&self, metadata: &mut Value) -> Result<(), DenError> {
        let mut merged = Self::from_metadata(metadata)?;
        merged.union_from(self);
        let envelope = serde_json::to_value(ContextDependenciesWire {
            version: WIRE_VERSION,
            personal_facts: merged.personal_facts.into_iter().collect(),
        })
        .map_err(|_| invalid_metadata())?;
        if metadata.is_null() {
            *metadata = Value::Object(serde_json::Map::new());
        }
        metadata
            .as_object_mut()
            .ok_or_else(invalid_metadata)?
            .insert(METADATA_FIELD.to_string(), envelope);
        Ok(())
    }
}

impl fmt::Debug for ContextDependencies {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ContextDependencies([redacted])")
    }
}

pub fn require_unrestricted_metadata(metadata: &Value) -> Result<(), DenError> {
    ContextDependencies::from_metadata(metadata)?.require_unrestricted_reuse()
}

#[cfg(test)]
#[path = "dependencies_tests.rs"]
mod tests;
