import type {
    RoomOrchestrationConfigDto,
    OrchestrationWorkspaceDto,
} from '../../../lib/ipc/contracts';

function emptyRoomConfig(conversationId = '', branchId = ''): RoomOrchestrationConfigDto {
    return {
        conversation_id: conversationId,
        branch_id: branchId,
        prompt_preset_id: null,
        generation_preset_id: null,
        response_length: 'balanced',
        creativity: 50,
        reasoning_effort: 'provider_default',
        memory_enabled: true,
        knowledge_enabled: true,
        creator_values: {},
        variable_overrides: { values: [] },
        user_name_override: null,
        author_note: null,
        group_context: null,
        template_slots: [],
        supported_fields: {
            prompt_preset_id: true,
            generation_preset_id: true,
            creator_values: true,
            variable_overrides: false,
            response_length: true,
            creativity: true,
            reasoning_effort: true,
            memory_enabled: true,
            knowledge_enabled: true,
            user_name_override: true,
            author_note: true,
            group_context: true,
            template_slots: true,
        },
    };
}

export function emptyOrchestrationWorkspace(
    conversationId = '',
    branchId = '',
): OrchestrationWorkspaceDto {
    return {
        expected_head: null,
        room_config_revision: null,
        prompt_preset_revision: null,
        interaction_state_revision: null,
        generation_target: null,
        prompt_presets: [],
        room_config: emptyRoomConfig(conversationId, branchId),
        prompt_blocks: [],
        creator_controls: [],
        knowledge_book_ids: [],
        task_profiles: [],
        memory_records: [],
        selection_evidence: [],
        interaction_state: [],
        interaction_proposals: [],
        content_modules: [],
        module_diff: null,
        plan_preview: null,
    };
}
