//! Revision envelope shared by storage-owned document reads.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A typed object together with its compare-and-swap storage revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredRevision<T> {
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
