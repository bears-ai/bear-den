//! Den-minted opaque Cabinet refs.
//!
//! Every ref is a fixed prefix plus a 32-character lowercase-hex suffix,
//! following the artifact-ref convention. Refs are minted only by Den,
//! validated on parse and deserialize, and stable for the entity lifetime.
//! They are not object keys, URLs, paths, titles, or slugs.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ContractViolation;

pub(crate) const REF_SUFFIX_LEN: usize = 32;

fn valid_suffix(suffix: &str) -> bool {
    suffix.len() == REF_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn parse_prefixed(
    kind: &'static str,
    prefix: &'static str,
    value: &str,
) -> Result<(), ContractViolation> {
    let malformed = || ContractViolation::MalformedRef {
        expected: kind,
        value: value.to_string(),
    };
    let suffix = value.strip_prefix(prefix).ok_or_else(malformed)?;
    if valid_suffix(suffix) {
        Ok(())
    } else {
        Err(malformed())
    }
}

macro_rules! cabinet_ref {
    ($(#[$doc:meta])* $name:ident, $kind:literal, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// The wire prefix for this ref kind.
            pub const PREFIX: &'static str = $prefix;

            /// Mint a new ref. Only Den mints refs; models, clients, and
            /// providers parse and echo them.
            #[must_use]
            pub fn mint() -> Self {
                Self(format!("{}{}", $prefix, Uuid::new_v4().simple()))
            }

            /// Validate and adopt an existing ref string.
            pub fn parse(value: &str) -> Result<Self, ContractViolation> {
                parse_prefixed($kind, $prefix, value)?;
                Ok(Self(value.to_string()))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = ContractViolation;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = ContractViolation;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                parse_prefixed($kind, $prefix, &value)?;
                Ok(Self(value))
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

cabinet_ref!(
    /// A Cabinet item — the durable knowledge object. The protocol field name
    /// for this ref is `cabinet_ref` (ADR-0004).
    CabinetItemRef,
    "cabinet item",
    "cabinet_item_"
);

cabinet_ref!(
    /// An immutable item version — the citation and revision-history unit.
    CabinetVersionRef,
    "cabinet version",
    "cabinet_version_"
);

cabinet_ref!(
    /// An organizational collection within the Cabinet.
    CabinetCollectionRef,
    "cabinet collection",
    "cabinet_collection_"
);

cabinet_ref!(
    /// A Cabinet Mission — the shared cross-Bear work/knowledge container.
    MissionRef,
    "mission",
    "mission_"
);

cabinet_ref!(
    /// A source link: provenance from an item to material outside Cabinet.
    CabinetSourceRef,
    "cabinet source link",
    "cabinet_source_"
);

cabinet_ref!(
    /// An attachment link: binding from an item to a Den artifact ref.
    CabinetAttachmentRef,
    "cabinet attachment link",
    "cabinet_attachment_"
);

cabinet_ref!(
    /// An append-only discussion entry on a Cabinet page, not a page revision.
    CabinetDiscussionRef,
    "cabinet discussion entry",
    "cabinet_discussion_"
);

cabinet_ref!(
    /// A review record accompanying a review-state transition (Phase 2).
    CabinetReviewRef,
    "cabinet review",
    "cabinet_review_"
);
