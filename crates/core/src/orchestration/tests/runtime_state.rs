#[test]
fn module_render_content_keeps_a_storage_valid_runtime_state_revision() {
    let root = tempdir().expect("temporary runtime state root");
    let core = Core::open(CoreConfig::new(root.path())).expect("Core");
    let character_id = import_synthetic_character(&core);
    let conversation = core.open_conversation(&character_id).expect("room");
    let branch = core.get_conversation_state(&conversation.id).expect("branch").active_branch_id;
    activate_app_module(
        &core,
        &prompt_marker_module(),
        ContentModuleRuntimeTarget { conversation_id: conversation.id.clone(), branch_id: branch.clone() },
        "synthetic.runtime-state.module",
    );
    let base = core.get_character_content(&character_id).expect("stored card");
    let effective = core.get_effective_character_content(&character_id, &conversation.id, &branch).expect("merged card");
    assert_eq!(effective.revision_id, base.revision_id);
    let scope = lorepia_core::PortableRuntimeStateScope {
        character_id,
        character_content_revision_id: effective.revision_id,
        conversation_id: conversation.id.0,
        branch_id: branch.0,
    };
    let empty = core.get_portable_runtime_state(&scope).expect("valid runtime scope");
    let saved = core.put_portable_runtime_state(lorepia_core::PortableRuntimeStateWrite {
        scope: scope.clone(), expected_scope_epoch: empty.scope_epoch, expected_revision: None,
        payload: lorepia_core::PortableRuntimeStatePayload {
            schema_version: 1,
            value: serde_json::json!({"options":{},"chatVars":{"lang":"0","lore":"1"},"state":{},"messageOverrides":{},"background":"","auxiliarySelection":null}),
        },
    }).expect("persist settings with approved module active");
    assert!(matches!(saved, lorepia_core::PortableRuntimeStateSaveResult::Saved { .. }));
    drop(core);
    let core = Core::open(CoreConfig::new(root.path())).expect("reopen");
    let restored = core.get_portable_runtime_state(&scope).expect("restored scope").record.expect("saved row");
    assert_eq!(restored.payload.value["chatVars"]["lang"], "0");
    assert_eq!(restored.payload.value["chatVars"]["lore"], "1");
    let mut forged = scope;
    forged.character_content_revision_id = Some("ab".repeat(32));
    assert!(core.get_portable_runtime_state(&forged).is_err(), "unknown revisions remain rejected");
}
