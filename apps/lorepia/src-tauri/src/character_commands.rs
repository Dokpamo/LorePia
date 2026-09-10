use lorepia_shell_api::{CharacterRenderProfileDto, ShellApi};
use serde::Deserialize;

use crate::{
    contract::CharacterRenderProfileRequest,
    error::{CommandError, CommandResult},
};

pub(crate) fn get_character_render_profile(
    shell: &ShellApi,
    request: CharacterRenderProfileRequest,
) -> CommandResult<CharacterRenderProfileDto> {
    match (request.conversation_id, request.branch_id) {
        (Some(conversation_id), Some(branch_id)) => shell
            .get_character_render_profile_for_room(
                &request.character_id,
                &conversation_id,
                &branch_id,
            )
            .map_err(Into::into),
        (None, None) => shell
            .get_character_render_profile(&request.character_id)
            .map_err(Into::into),
        _ => Err(CommandError::invalid_input()),
    }
}

#[tauri::command]
pub fn get_character_greeting_detail(
    state: tauri::State<'_, crate::state::AppState>,
    request: CharacterGreetingDetailRequest,
) -> CommandResult<lorepia_shell_api::CharacterGreetingDetailDto> {
    state
        .shell()?
        .get_character_greeting_detail(
            &request.character_id,
            &request.character_content_revision_id,
            &request.greeting_id,
        )
        .map_err(Into::into)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_field_names,
    reason = "Field names match the renderer IPC contract"
)]
pub struct CharacterGreetingDetailRequest {
    pub character_id: String,
    pub character_content_revision_id: String,
    pub greeting_id: String,
}

#[cfg(test)]
mod tests {
    use super::CharacterGreetingDetailRequest;
    #[test]
    fn opening_reader_requires_revision_and_rejects_text_and_extra_fields() {
        let input = serde_json::json!({"character_id":"character", "character_content_revision_id":"revision", "greeting_id":"alternate-0"});
        assert!(serde_json::from_value::<CharacterGreetingDetailRequest>(input.clone()).is_ok());
        let mut extra = input.clone();
        extra["text"] = "renderer authored".into();
        assert!(serde_json::from_value::<CharacterGreetingDetailRequest>(extra).is_err());
        let mut missing = input;
        missing
            .as_object_mut()
            .unwrap()
            .remove("character_content_revision_id");
        assert!(serde_json::from_value::<CharacterGreetingDetailRequest>(missing).is_err());
    }
}
