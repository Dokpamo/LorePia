//! Verified message presentation commands, including bounded terminal refresh.
use lorepia_shell_api::MessageDto;
use serde::Deserialize;
use tauri::State;

use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationRequest {
    pub conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchMessagesRequest {
    pub branch_id: String,
}

#[tauri::command]
pub fn list_branch_messages(
    state: State<'_, AppState>,
    request: BranchMessagesRequest,
) -> CommandResult<Vec<MessageDto>> {
    state
        .shell()?
        .list_branch_messages(&request.branch_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_messages(
    state: State<'_, AppState>,
    request: ConversationRequest,
) -> CommandResult<Vec<MessageDto>> {
    state
        .shell()?
        .list_messages(&request.conversation_id)
        .map_err(Into::into)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_field_names,
    reason = "route identity fields match the existing IPC naming convention"
)]
pub struct GenerationMessagesRequest {
    pub conversation_id: String,
    pub branch_id: String,
    pub generation_id: String,
}

#[tauri::command]
pub fn list_generation_messages(
    state: State<'_, AppState>,
    request: GenerationMessagesRequest,
) -> CommandResult<Vec<MessageDto>> {
    state
        .shell()?
        .list_generation_messages(
            &request.conversation_id,
            &request.branch_id,
            &request.generation_id,
        )
        .map_err(CommandError::from)
}

#[cfg(test)]
mod tests {
    use super::GenerationMessagesRequest;

    #[test]
    fn generation_message_route_is_strict_and_requires_all_identities() {
        let valid = r#"{"conversation_id":"c","branch_id":"b","generation_id":"g"}"#;
        assert!(serde_json::from_str::<GenerationMessagesRequest>(valid).is_ok());
        for invalid in [
            r#"{"conversation_id":"c","branch_id":"b"}"#,
            r#"{"conversation_id":"c","branch_id":"b","generation_id":"g","limit":999}"#,
        ] {
            assert!(serde_json::from_str::<GenerationMessagesRequest>(invalid).is_err());
        }
    }
}
