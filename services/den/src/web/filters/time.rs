use minijinja::Value;
use time::{Duration, OffsetDateTime};

pub fn timeago(value: Value) -> String {
    if let Some(datetime_str) = value.as_str() {
        // Try to parse the datetime string
        if let Ok(datetime) =
            OffsetDateTime::parse(datetime_str, &time::format_description::well_known::Rfc3339)
        {
            return format_relative_time(datetime);
        }
    }

    // If we can't parse it, return the original value as string
    value.to_string()
}

pub fn is_future(value: Value) -> bool {
    let now = OffsetDateTime::now_utc();

    // Handle string values (RFC3339 formatted strings)
    if let Some(datetime_str) = value.as_str() {
        if let Ok(datetime) =
            OffsetDateTime::parse(datetime_str, &time::format_description::well_known::Rfc3339)
        {
            return datetime > now;
        }
    }

    // Handle serialized OffsetDateTime objects by trying to deserialize from the value
    if let Ok(datetime) = serde_json::from_value::<OffsetDateTime>(
        serde_json::to_value(&value).unwrap_or(serde_json::Value::Null),
    ) {
        return datetime > now;
    }

    // If we can't parse it, assume it's not in the future
    false
}

pub fn format_relative_time(datetime: OffsetDateTime) -> String {
    let now = OffsetDateTime::now_utc();
    let duration = now - datetime;

    if duration < Duration::ZERO {
        return "in the future".to_string();
    }

    let total_seconds = duration.whole_seconds();

    if total_seconds < 60 {
        return "just now".to_string();
    }

    let minutes = total_seconds / 60;
    if minutes < 60 {
        return if minutes == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{minutes} minutes ago")
        };
    }

    let hours = minutes / 60;
    if hours < 24 {
        return if hours == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{hours} hours ago")
        };
    }

    let days = hours / 24;
    if days < 30 {
        return if days == 1 {
            "1 day ago".to_string()
        } else {
            format!("{days} days ago")
        };
    }

    let months = days / 30;
    if months < 12 {
        return if months == 1 {
            "1 month ago".to_string()
        } else {
            format!("{months} months ago")
        };
    }

    let years = months / 12;
    if years == 1 {
        "1 year ago".to_string()
    } else {
        format!("{years} years ago")
    }
}
