use lorepia_shell_api::{CharacterRenderProfileDto, ShellApi};

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
