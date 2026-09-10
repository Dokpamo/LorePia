import { describe, expect, it } from 'vitest';

import type { CreatorPromptPresetDocumentDto, ProviderWorkspaceDto } from '../../lib/ipc/contracts';
import {
    buildImportedGenerationPreset,
    buildImportedMemoryProfile,
    buildImportedSummaryTask,
    hintString,
    readImportedCompatibilityHints,
    recommendImportedRoute,
} from './imported-compatibility';

function promptWithHints(
    kind: 'generation' | 'memory',
    hints: Record<string, boolean | number | string>,
): CreatorPromptPresetDocumentDto {
    const values: Record<string, boolean | number | string> = { import_kind: kind, ...hints };
    return {
        id: 'imported-source',
        name: 'external source',
        schema_version: 1,
        blocks: [],
        controls: [],
        default_values: {
            values: Object.entries(values).map(([id, value]) => ({
                variable: { scope: 'app', namespace: null, id: `lorepia_imported_${id}` },
                value:
                    typeof value === 'boolean'
                        ? { type: 'bool', value }
                        : typeof value === 'number'
                          ? { type: Number.isInteger(value) ? 'integer' : 'decimal', value }
                          : { type: 'text', value },
            })),
        },
        default_generation_preset_id: null,
        memory_profile_id: null,
        knowledge_book_ids: [],
        transform_set_ids: [],
        module_ids: [],
        cache_boundaries: [],
        metadata: {
            description: '',
            tags: ['compatibility'],
            provenance: {
                source_kind: 'imported_standard',
                source_id: null,
                source_hash: null,
                author: null,
                license: null,
                imported_at: null,
            },
            created_at: '1970-01-01T00:00:00Z',
            updated_at: '1970-01-01T00:00:00Z',
            local_override_of: null,
        },
    };
}

function providerWorkspace(): ProviderWorkspaceDto {
    return {
        templates: [
            {
                id: 'gemini',
                display_name: 'Gemini',
                manifest_version: 1,
                source: 'built_in',
                api_family: 'gemini_generate_content',
                connection_fields: [],
                default_network_mode: 'public',
                default_api_origin: null,
                credential_required: true,
                supports_model_listing: true,
                auth_binding: { kind: 'none' },
                parameters: [
                    {
                        id: 'temperature',
                        label_key: 'temperature',
                        description_key: null,
                        value_type: 'number',
                        allowed_values: [],
                        minimum: 0,
                        maximum: 2,
                        step: null,
                        default_mode: 'provider_default',
                        visibility: null,
                        conflicts: [],
                        provider_mapping: { target: 'body', field_name: 'temperature' },
                        level: 'basic',
                    },
                    {
                        id: 'max_output_tokens',
                        label_key: 'max_output_tokens',
                        description_key: null,
                        value_type: 'integer',
                        allowed_values: [],
                        minimum: 1,
                        maximum: 4_096,
                        step: 1,
                        default_mode: 'provider_default',
                        visibility: null,
                        conflicts: [],
                        provider_mapping: { target: 'body', field_name: 'maxOutputTokens' },
                        level: 'basic',
                    },
                ],
            },
        ],
        connections: [
            {
                id: 'connection-gemini',
                template_id: 'gemini',
                template_version: 1,
                display_name: 'Gemini connection',
                api_origin: 'https://example.invalid',
                api_base_path: null,
                network_mode: 'public',
                local_network_approval: null,
                config_values: [],
                credential_binding_required: true,
                credential_scope: null,
                approved_credential_origins: [],
                timeout_seconds: 60,
                status: 'active',
                created_at: '1970-01-01T00:00:00Z',
                updated_at: '1970-01-01T00:00:00Z',
            },
        ],
        routes: [
            {
                id: 'route-gemini',
                connection_id: 'connection-gemini',
                api_family: 'gemini_generate_content',
                model_id: 'gemini-3.1-pro-preview',
                display_name: null,
                route_config: {
                    deployment_id: null,
                    region: null,
                    endpoint_path: null,
                    values: [],
                },
                status: 'available',
                miss_count: 0,
                metadata_source: 'test',
                metadata_observed_at: null,
                first_seen_at: '1970-01-01T00:00:00Z',
                last_seen_at: null,
            },
        ],
        presets: [],
        legacy_profiles: [],
        settings: {
            preserve_partial_generations: true,
            selected_provider_profile_id: null,
            selected_model_route_id: null,
            selected_generation_preset_id: null,
        },
        credential_statuses: {},
        request_preview: null,
        selected_capability_model_route_id: null,
        capability_observations: [],
        capability_parameter_specs: [],
        effective_capability: null,
        model_sync_jobs: [],
        selected_model_sync_job_id: null,
        model_sync_event: null,
        discoveries: [],
        selected_discovery_id: null,
        discovery_candidates: [],
        discovery_evidence: [],
        discovery_approvals: [],
        discovery_review: null,
        discovery_approval_proposal: null,
        discovery_review_proposal: null,
        discovery_assistant_resume_boundary: null,
        discovery_assistant_host_action: null,
        discovery_event: null,
        discovery_compensation_steps: [],
        discovery_recovery_results: [],
        catalog_status: null,
        catalog_history: null,
        pending_catalog_import: null,
        pending_catalog_rollback: null,
        catalog_diff: null,
    };
}

describe('external compatibility projection', () => {
    it('continues to read legacy stored hint prefixes', () => {
        const prompt = promptWithHints('generation', { model_hint: 'legacy-model' });
        for (const binding of prompt.default_values.values) {
            binding.variable.id = binding.variable.id.replace('lorepia_imported_', 'lorepia_risu_');
        }

        const hints = readImportedCompatibilityHints(prompt);
        expect(hints?.kind).toBe('generation');
        expect(hints === null ? null : hintString(hints, 'model_hint')).toBe('legacy-model');
    });

    it('matches a model hint and clamps scaled generation parameters to the route contract', () => {
        const hints = readImportedCompatibilityHints(
            promptWithHints('generation', {
                model_hint: 'Gemini 3.1 Pro Preview',
                temperature_raw: 100,
                max_response: 8_192,
            }),
        );
        expect(hints).not.toBeNull();
        if (hints === null) throw new Error('generation hints');
        const workspace = providerWorkspace();
        expect(recommendImportedRoute(workspace, 'Gemini 3.1 Pro Preview')).toBe('route-gemini');
        const preset = buildImportedGenerationPreset(
            workspace,
            hints,
            'route-gemini',
            'imported-provider',
            'external provider',
        );
        expect(preset.values).toEqual([
            {
                parameter_id: 'temperature',
                state: { state: 'explicit', value: { type: 'number', value: 1 } },
            },
            {
                parameter_id: 'max_output_tokens',
                state: { state: 'explicit', value: { type: 'integer', value: 4_096 } },
            },
        ]);
    });

    it('projects Hypa rate and concurrency settings into a bounded summary task', () => {
        const prompt = promptWithHints('memory', {
            memory_profile_id: 'imported-memory-profile',
            summary_task_id: 'imported-summary-task',
            memory_max_chats_per_summary: 7,
            memory_query_chat_count: 3,
            memory_recent_memory_ratio: 0.6,
            memory_extra_summarization_ratio: 0.2,
            memory_preserve_orphaned_memory: true,
            memory_summarization_requests_per_minute: 20,
            memory_summarization_max_concurrent: 2,
        });
        prompt.blocks = [
            {
                id: 'imported-memory-summary-fixture',
                name: 'Summary template',
                kind: 'static_instruction',
                enabled: true,
                role_hint: 'system',
                authority: 'imported_content',
                template: {
                    parts: [
                        {
                            kind: 'text',
                            value: 'Extract durable facts.\n{{slot}}\nReturn structured memory.',
                        },
                    ],
                    max_output_chars: 262_144,
                },
                condition: null,
                source: { kind: 'template' },
                placement_zone: 'preset_instruction',
                history_selector: null,
                token_policy: {
                    priority: 1,
                    min_tokens: null,
                    max_tokens: null,
                    reserve_tokens: null,
                },
                overflow_policy: 'drop_block',
                merge_policy: 'separate_message',
                provenance: prompt.metadata.provenance,
            },
        ];
        const hints = readImportedCompatibilityHints(prompt);
        if (hints === null) throw new Error('memory hints');
        const task = buildImportedSummaryTask(hints, 'route-gemini', 'summary-generation');
        expect(task).toMatchObject({
            id: 'imported-summary-task',
            kind: 'memory_summary',
            route_id: 'route-gemini',
            generation_preset_id: 'summary-generation',
            rate_limit: { requests: 20, per_seconds: 60 },
            concurrency_limit: 2,
        });
        if (task === null) throw new Error('summary task');
        expect(buildImportedMemoryProfile(prompt, hints, task.id, 'Imported memory')).toEqual({
            id: 'imported-memory-profile',
            name: 'Imported memory',
            summary_task: 'imported-summary-task',
            embedding_task: null,
            turns_per_summary: 7,
            recent_raw_budget: { max_tokens: 4_096 },
            episodic_budget: { max_tokens: 2_048 },
            semantic_budget: { max_tokens: 2_048 },
            retrieval_count: 3,
            recency_weight: 0.6,
            similarity_weight: 0,
            importance_weight: 0.2,
            preserve_invalidated_records: true,
            summary_schema: 'lorepia.memory-summary.v1',
            summary_template: {
                parts: [
                    { kind: 'text', value: 'Extract durable facts.\n' },
                    { kind: 'slot', name: 'memory_source' },
                    { kind: 'text', value: '\nReturn structured memory.' },
                ],
                max_output_chars: 262_144,
            },
        });
    });
});
