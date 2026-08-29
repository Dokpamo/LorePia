use lorepia_shell_api::{
    GetPortableRuntimeStateInput, PortableRuntimeStateSaveResultDto,
    PortableRuntimeStateSnapshotDto, PutPortableRuntimeStateInput, ShellApi,
};
use tauri::State;

use crate::{error::CommandResult, state::AppState};

#[tauri::command]
pub fn get_portable_runtime_state(
    state: State<'_, AppState>,
    request: GetPortableRuntimeStateInput,
) -> CommandResult<PortableRuntimeStateSnapshotDto> {
    execute_get_portable_runtime_state(&state.shell()?, request)
}

pub(crate) fn execute_get_portable_runtime_state(
    shell: &ShellApi,
    request: GetPortableRuntimeStateInput,
) -> CommandResult<PortableRuntimeStateSnapshotDto> {
    shell
        .get_portable_runtime_state(request)
        .map_err(Into::into)
}

#[tauri::command]
pub fn put_portable_runtime_state(
    state: State<'_, AppState>,
    request: PutPortableRuntimeStateInput,
) -> CommandResult<PortableRuntimeStateSaveResultDto> {
    execute_put_portable_runtime_state(&state.shell()?, request)
}

pub(crate) fn execute_put_portable_runtime_state(
    shell: &ShellApi,
    request: PutPortableRuntimeStateInput,
) -> CommandResult<PortableRuntimeStateSaveResultDto> {
    shell
        .put_portable_runtime_state(request)
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use lorepia_shell_api as shell;
    use serde_json::json;
    use tempfile::tempdir;

    use super::{execute_get_portable_runtime_state, execute_put_portable_runtime_state};

    #[test]
    fn command_helpers_delegate_an_exact_payload_through_shell_and_sqlite() {
        let root = tempdir().expect("temporary portable runtime command data root");
        let fixture = shell::test_support::seed_synthetic_memory_record_fixture(root.path())
            .expect("seed bounded conversation fixture");
        let shell_api = shell::test_support::open_data_root_after_drop(root.path())
            .expect("open Shell after fixture seeding");
        let conversation = shell_api
            .get_conversation(&fixture.conversation_id)
            .expect("load fixture conversation");
        let scope = shell::PortableRuntimeStateScopeDto {
            character_id: conversation.character_id,
            character_content_revision_id: None,
            conversation_id: fixture.conversation_id,
            branch_id: fixture.branch_id,
        };

        let missing = execute_get_portable_runtime_state(
            &shell_api,
            shell::GetPortableRuntimeStateInput {
                scope: scope.clone(),
            },
        )
        .expect("load missing portable runtime state through command helper");
        assert_eq!(missing.scope_epoch, 0);
        assert!(missing.record.is_none());

        let mut future_payload = exact_payload();
        future_payload.schema_version = 2;
        let error = execute_put_portable_runtime_state(
            &shell_api,
            shell::PutPortableRuntimeStateInput {
                scope: scope.clone(),
                expected_scope_epoch: 0,
                expected_revision: None,
                payload: future_payload,
            },
        )
        .expect_err("future schema must be rejected through the Tauri delegation helper");
        assert_eq!(error.code, "invalid_input");

        let mut malformed_payload = exact_payload();
        malformed_payload.value["chatVars"] = json!([]);
        let error = execute_put_portable_runtime_state(
            &shell_api,
            shell::PutPortableRuntimeStateInput {
                scope: scope.clone(),
                expected_scope_epoch: 0,
                expected_revision: None,
                payload: malformed_payload,
            },
        )
        .expect_err("malformed state must be rejected through the Tauri delegation helper");
        assert_eq!(error.code, "invalid_input");

        let saved = execute_put_portable_runtime_state(
            &shell_api,
            shell::PutPortableRuntimeStateInput {
                scope: scope.clone(),
                expected_scope_epoch: 0,
                expected_revision: None,
                payload: exact_payload(),
            },
        )
        .expect("save exact portable runtime state through command helper");
        let shell::PortableRuntimeStateSaveResultDto::Saved { record, .. } = saved else {
            panic!("first exact portable runtime state write must be saved");
        };
        assert_eq!(record.revision, 1);
        assert_eq!(record.payload.value["state"]["visited"], true);

        let loaded = execute_get_portable_runtime_state(
            &shell_api,
            shell::GetPortableRuntimeStateInput { scope },
        )
        .expect("reload portable runtime state through command helper");
        let record = loaded.record.expect("saved command state must be readable");
        assert_eq!(record.revision, 1);
        assert_eq!(record.payload.value["state"]["visited"], true);
    }

    fn exact_payload() -> shell::PortableRuntimeStatePayloadDto {
        shell::PortableRuntimeStatePayloadDto {
            schema_version: 1,
            value: json!({
                "options": {"tone": "warm"},
                "chatVars": {},
                "state": {"visited": true},
                "messageOverrides": {},
                "background": "",
                "auxiliarySelection": null
            }),
        }
    }
}
