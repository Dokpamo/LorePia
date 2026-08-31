use lorepia_core::HealthReport;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapDto {
    pub shell_api_version: u32,
    pub core_api_version: u32,
    pub chat_event_version: u32,
    pub health: HealthDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct HealthDto {
    pub core_version: String,
    pub database_open: bool,
    pub schema_version: u32,
    pub data_root_writable: bool,
    pub staging_writable: bool,
    pub recovery_pending: bool,
    pub active_jobs: u32,
}

impl From<HealthReport> for HealthDto {
    fn from(value: HealthReport) -> Self {
        Self {
            core_version: value.core_version,
            database_open: value.database_open,
            schema_version: value.schema_version,
            data_root_writable: value.data_root_writable,
            staging_writable: value.staging_writable,
            recovery_pending: value.recovery_pending,
            active_jobs: value.active_jobs,
        }
    }
}
