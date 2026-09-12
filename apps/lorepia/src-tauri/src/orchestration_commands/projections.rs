//! Room projection reads may drain durable module lifecycle work.

use lorepia_shell_api as shell;

use crate::{
    error::{CommandError, CommandResult},
    module_lifecycle_commands::run_module_read,
};

pub(super) async fn list_interaction_effect_history(
    api: shell::ShellApi,
    request: shell::ListInteractionEffectHistoryInput,
) -> CommandResult<shell::InteractionEffectHistoryPageDto> {
    run_module_read(move || {
        api.list_interaction_effect_history(request)
            .map_err(CommandError::from)
    })
    .await
}

pub(super) async fn list_reopen_interaction_effects(
    api: shell::ShellApi,
    request: shell::ListRecentReopenInteractionEffectsInput,
) -> CommandResult<shell::InteractionReopenSnapshotDto> {
    run_module_read(move || {
        api.list_reopen_interaction_effects(request)
            .map_err(CommandError::from)
    })
    .await
}
