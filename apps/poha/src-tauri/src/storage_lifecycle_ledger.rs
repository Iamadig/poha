use chrono::{DateTime, Utc};
use serde::Serialize;
use std::io::Write;

use super::{StorageError, StorageMaintenanceAction};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageMaintenanceLedgerEntry<'a> {
    generated_at: String,
    kind: &'a str,
    session_id: &'a Option<String>,
    path: &'a str,
    bytes: u64,
    reason: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    validation_facts: Option<&'a super::validation::StorageValidationFacts>,
}

pub(super) fn write_entry<W: Write>(
    writer: &mut W,
    generated_at: DateTime<Utc>,
    action: &StorageMaintenanceAction,
) -> Result<(), StorageError> {
    let entry = StorageMaintenanceLedgerEntry {
        generated_at: generated_at.to_rfc3339(),
        kind: &action.kind,
        session_id: &action.session_id,
        path: &action.path,
        bytes: action.bytes,
        reason: &action.reason,
        validation_facts: action.validation_facts.as_ref(),
    };
    serde_json::to_writer(&mut *writer, &entry)
        .map_err(|error| StorageError::new("storageLedgerFailed", error.to_string()))?;
    writeln!(writer).map_err(|error| StorageError::new("storageLedgerFailed", error.to_string()))
}
