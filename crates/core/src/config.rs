use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Selects the layer that owns crash recovery for unfinished provider discovery.
///
/// Standalone Core consumers must keep the default `Core` owner so unfinished
/// external effects are conservatively classified before the Core becomes
/// observable. Native application hosts may select `NativePlatform` only when
/// they reconcile the durable operation against their credential vault before
/// publishing the Core to callers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiscoveryRecoveryOwner {
    #[default]
    Core,
    NativePlatform,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreConfig {
    pub data_root: PathBuf,
}

impl CoreConfig {
    pub fn new(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
        }
    }
}
