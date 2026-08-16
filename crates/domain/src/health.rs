use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct HealthReport {
    pub core_version: String,
    pub database_open: bool,
    pub schema_version: u32,
    pub data_root_writable: bool,
    pub staging_writable: bool,
    pub recovery_pending: bool,
    pub active_jobs: u32,
}
