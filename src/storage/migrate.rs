//! Upgrades stored conversations from older formats.
//!
//! Each step takes the JSON of version N and returns version N+1. Data that is too
//! old to understand (for example from before positions were stored) is not
//! guessed at; it fails to load as corrupt.

use serde_json::Value;

use crate::domain::SCHEMA_VERSION;

use super::StorageError;

pub fn migrate(mut value: Value) -> Result<Value, StorageError> {
    let version = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .unwrap_or(1);

    if version > SCHEMA_VERSION {
        return Err(StorageError::UnsupportedVersion { found: version, supported: SCHEMA_VERSION });
    }

    // v1 -> v2: timing, run metadata and the questions snapshot were added. All are
    // optional on read, so only the version stamp changes.
    if version < 2
        && let Some(object) = value.as_object_mut()
    {
        object.insert("schema_version".to_owned(), Value::from(2));
    }

    Ok(value)
}
