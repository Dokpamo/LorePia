//! Core-owned revision envelope for values crossing the application boundary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A typed value together with its compare-and-swap revision metadata.
///
/// Storage keeps its own persistence row type. Core projects that row into
/// this envelope before returning it to Shell or any other public caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revisioned<T> {
    pub value: T,
    pub revision: u64,
    /// Exact immutable content revision when the value is backed by the
    /// generic content registry. Mutable binding/job records have no immutable
    /// content revision and return `None`.
    #[serde(default)]
    pub revision_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Projects a storage-owned persistence row at the private Core boundary.
pub(crate) fn project_revision<T>(stored: lorepia_storage::StoredRevision<T>) -> Revisioned<T> {
    Revisioned {
        value: stored.value,
        revision: stored.revision,
        revision_id: stored.revision_id,
        created_at: stored.created_at,
        updated_at: stored.updated_at,
        deleted_at: stored.deleted_at,
    }
}

pub(crate) fn project_revisions<T>(
    stored: Vec<lorepia_storage::StoredRevision<T>>,
) -> Vec<Revisioned<T>> {
    stored.into_iter().map(project_revision).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_projection_preserves_the_serialized_revision_envelope() {
        let stored = lorepia_storage::StoredRevision {
            value: "payload".to_owned(),
            revision: 7,
            revision_id: Some("immutable-revision".to_owned()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: Some(Utc::now()),
        };
        let expected = serde_json::to_value(&stored).expect("serialize storage revision");

        let projected = project_revision(stored);

        assert_eq!(
            serde_json::to_value(projected).expect("serialize Core revision"),
            expected
        );
    }
}
