//! Shared API utility functions
//!
//! This module contains utility functions that are shared between the web API
//! and standalone API services, such as date deserialization helpers.

use serde::Deserialize;
use time::{Date, OffsetDateTime};

/// Deserialize empty datetime strings as None
///
/// This helper function allows API endpoints to accept empty strings for optional
/// datetime parameters and treat them as None values.
pub fn deserialize_empty_datetime_as_none<'de, D>(
    deserializer: D,
) -> Result<Option<OffsetDateTime>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(raw) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    OffsetDateTime::parse(&raw, &time::format_description::well_known::Rfc3339)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

/// Deserialize empty date strings as None
///
/// This helper function allows API endpoints to accept empty strings for optional
/// date parameters and treat them as None values.
/// Accepts dates in YYYY-MM-DD format.
pub fn deserialize_empty_date_as_none<'de, D>(deserializer: D) -> Result<Option<Date>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(raw) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    const FORMAT: &[time::format_description::FormatItem] =
        time::macros::format_description!("[year]-[month]-[day]");
    Date::parse(&raw, &FORMAT)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

/// Serde serialization and deserialization for time::Date
pub mod date_format {
    use serde::{self, Deserialize, Deserializer, Serializer};
    use time::Date;

    const FORMAT: &[time::format_description::FormatItem] =
        time::macros::format_description!("[year]-[month]-[day]");

    pub fn serialize<S>(date: &Date, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = date.format(&FORMAT).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&s)
    }

    #[allow(unused)]
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Date, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Date::parse(&s, &FORMAT).map_err(serde::de::Error::custom)
    }
}
