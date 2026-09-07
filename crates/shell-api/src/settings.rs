use serde::{Deserialize, Serialize};

use crate::{ShellApi, ShellError, ShellResult};

/// Aggregate counts only; storage paths and rows stay behind Core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageOverviewDto {
    pub characters: u64,
    pub conversations: u64,
    pub messages: u64,
    pub import_jobs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use lorepia_core::CoreConfig;

    #[test]
    fn storage_overview_reports_empty_store_without_host_information() {
        let root = tempfile::tempdir().expect("temporary data root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open shell");
        let value = shell.get_storage_overview().expect("storage overview");
        assert_eq!(value.characters, 0);
        assert_eq!(value.conversations, 0);
        assert_eq!(value.messages, 0);
        let json = serde_json::to_value(value).expect("serialize overview");
        assert_eq!(json.as_object().expect("object").len(), 4);
        assert!(!json.to_string().contains(root.path().to_str().expect("path")));
    }
}

impl ShellApi {
    pub fn get_storage_overview(&self) -> ShellResult<StorageOverviewDto> {
        let stats = self.core.database_stats().map_err(ShellError::from)?;
        Ok(StorageOverviewDto {
            characters: stats.characters,
            conversations: stats.conversations,
            messages: stats.messages,
            import_jobs: stats.pending_imports,
        })
    }
}
