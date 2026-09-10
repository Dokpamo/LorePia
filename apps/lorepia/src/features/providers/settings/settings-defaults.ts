import { t } from '../../../lib/i18n';
import type {
    SafePromptTemplateDto,
    CreatorMemoryProfileDocumentDto,
    CreatorOrchestrationProvenanceDto,
    CreatorPromptBlockDocumentDto,
    CreatorPromptPresetDocumentDto,
    TaskProfileDocumentDto,
} from '../../../lib/ipc/contracts';

const provenance = (): CreatorOrchestrationProvenanceDto => ({
    source_kind: 'user_created',
    source_id: null,
    source_hash: null,
    author: null,
    license: null,
    imported_at: null,
});
export function newTask(kind: 'memory_summary' | 'memory_embedding'): TaskProfileDocumentDto {
    return {
        id: `task-${crypto.randomUUID()}`,
        kind,
        route_id: '',
        generation_preset_id: '',
        fallback_route_ids: [],
        embedding_dimensions: kind === 'memory_embedding' ? 1536 : null,
        timeout_ms: 60000,
        rate_limit: { requests: kind === 'memory_embedding' ? 100 : 20, per_seconds: 60 },
        concurrency_limit: kind === 'memory_embedding' ? 3 : 2,
    };
}
export function newMemory(): CreatorMemoryProfileDocumentDto {
    return {
        id: `memory-${crypto.randomUUID()}`,
        name: '',
        summary_task: '',
        embedding_task: null,
        turns_per_summary: 8,
        recent_raw_budget: { max_tokens: 2048 },
        episodic_budget: { max_tokens: 1024 },
        semantic_budget: { max_tokens: 1024 },
        retrieval_count: 8,
        recency_weight: 0.6,
        similarity_weight: 0.4,
        importance_weight: 0,
        preserve_invalidated_records: true,
        summary_schema: 'lorepia.memory_summary.v1',
        summary_template: null,
    };
}
export function promptBlock(
    kind: CreatorPromptBlockDocumentDto['kind'],
    name: string,
    source: CreatorPromptBlockDocumentDto['source'],
    zone: CreatorPromptBlockDocumentDto['placement_zone'],
): CreatorPromptBlockDocumentDto {
    return {
        id: `block-${crypto.randomUUID()}`,
        name,
        kind,
        enabled: true,
        role_hint:
            kind === 'latest_user_turn'
                ? 'user'
                : kind === 'history_slice'
                  ? 'provider_default'
                  : 'system',
        authority: kind === 'history_slice' ? 'conversation' : 'user',
        template:
            source.kind === 'template'
                ? { parts: [{ kind: 'text', value: '' }], max_output_chars: 32000 }
                : null,
        condition: null,
        source,
        placement_zone: zone,
        history_selector:
            source.kind === 'history' ? { kind: 'excluding_latest_user', count: 40 } : null,
        token_policy: {
            priority: kind === 'latest_user_turn' ? 65535 : kind === 'history_slice' ? 62000 : 100,
            min_tokens: kind === 'latest_user_turn' ? 1 : null,
            max_tokens: null,
            reserve_tokens: null,
        },
        overflow_policy:
            kind === 'latest_user_turn'
                ? 'reject'
                : kind === 'history_slice'
                  ? 'keep_latest_items'
                  : 'trim_tail',
        merge_policy: 'separate_message',
        provenance: provenance(),
    };
}
export function newPrompt(): CreatorPromptPresetDocumentDto {
    const timestamp = new Date().toISOString();
    return {
        id: `prompt-${crypto.randomUUID()}`,
        name: '',
        schema_version: 1,
        blocks: [
            promptBlock(
                'static_instruction',
                t('settingsUi.promptName'),
                { kind: 'template' },
                'preset_instruction',
            ),
            promptBlock(
                'character_description',
                t('settingsLive.characterContext'),
                { kind: 'character_field', field: 'description' },
                'character_context',
            ),
            promptBlock(
                'user_persona',
                t('settingsUi.personas'),
                { kind: 'user_persona' },
                'character_context',
            ),
            promptBlock(
                'retrieved_memory',
                t('settingsUi.memory'),
                { kind: 'selected_memory' },
                'retrieved_context',
            ),
            promptBlock(
                'history_slice',
                t('settingsLive.history'),
                { kind: 'history' },
                'recent_history',
            ),
            promptBlock(
                'latest_user_turn',
                t('settingsLive.latestMessage'),
                { kind: 'latest_user' },
                'latest_user',
            ),
        ],
        controls: [],
        default_values: { values: [] },
        default_generation_preset_id: null,
        memory_profile_id: null,
        knowledge_book_ids: [],
        transform_set_ids: [],
        module_ids: [],
        cache_boundaries: [],
        metadata: {
            description: '',
            tags: [],
            provenance: provenance(),
            created_at: timestamp,
            updated_at: timestamp,
            local_override_of: null,
        },
    };
}

/** Keep the conversation slot intact when editing summary instructions. */
export function summaryTemplate(guidance: string): SafePromptTemplateDto | null {
    return guidance.trim()
        ? {
              parts: [
                  { kind: 'text', value: guidance },
                  { kind: 'text', value: '\n\n' },
                  { kind: 'slot', name: 'memory_source' },
              ],
              max_output_chars: 32000,
          }
        : null;
}
export function summaryGuidance(template: SafePromptTemplateDto | null | undefined): string | null {
    if (!template) return '';
    const [text, separator, source] = template.parts;
    if (
        template.parts.length === 3 &&
        text?.kind === 'text' &&
        separator?.kind === 'text' &&
        separator.value === '\n\n' &&
        source?.kind === 'slot' &&
        source.name === 'memory_source'
    )
        return text.value;
    return null;
}

/** Reserved preset identities seeded by Storage; Rust remains the write authority. */
export function isBuiltInPrompt(id: string): boolean {
    return (
        id === 'lorepia.builtin.chat-compatible.v1' || id === 'lorepia.builtin.story-compatible.v1'
    );
}
