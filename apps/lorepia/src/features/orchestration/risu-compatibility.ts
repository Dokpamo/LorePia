import type {
    CreatorMemoryProfileDocumentDto,
    CreatorPromptPresetDocumentDto,
    GenerationParameterDto,
    GenerationPresetInput,
    ParameterLiteralDto,
    ParameterSpecDto,
    ProviderWorkspaceDto,
    TaskProfileDocumentDto,
} from '../../lib/ipc/contracts';

export type RisuCompatibilityKind = 'generation' | 'memory';

type HintScalar = boolean | number | string;

export interface RisuCompatibilityHints {
    kind: RisuCompatibilityKind;
    values: ReadonlyMap<string, HintScalar>;
}

const HINT_PREFIX = 'lorepia_risu_';

export function readRisuCompatibilityHints(
    preset: CreatorPromptPresetDocumentDto | null,
): RisuCompatibilityHints | null {
    if (preset === null) return null;
    const values = new Map<string, HintScalar>();
    for (const binding of preset.default_values.values) {
        if (!binding.variable.id.startsWith(HINT_PREFIX)) continue;
        const value = binding.value;
        if (value.type === 'string_list') continue;
        values.set(binding.variable.id.slice(HINT_PREFIX.length), value.value);
    }
    const kind = values.get('import_kind');
    return kind === 'generation' || kind === 'memory' ? { kind, values } : null;
}

export function hintString(hints: RisuCompatibilityHints, key: string): string | null {
    const value = hints.values.get(key);
    return typeof value === 'string' && value.trim() !== '' ? value : null;
}

export function hintNumber(hints: RisuCompatibilityHints, key: string): number | null {
    const value = hints.values.get(key);
    return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

export function hintBoolean(hints: RisuCompatibilityHints, key: string): boolean | null {
    const value = hints.values.get(key);
    return typeof value === 'boolean' ? value : null;
}

export function recommendRisuRoute(
    workspace: ProviderWorkspaceDto,
    modelHint: string | null,
): string {
    const normalizedHint = normalizeModelLabel(modelHint ?? '');
    const hinted =
        normalizedHint === ''
            ? undefined
            : workspace.routes.find((route) => {
                  const model = normalizeModelLabel(route.model_id);
                  return model.includes(normalizedHint) || normalizedHint.includes(model);
              });
    if (hinted !== undefined) return hinted.id;
    const selected = workspace.settings.selected_model_route_id;
    if (selected !== null && workspace.routes.some((route) => route.id === selected)) {
        return selected;
    }
    return (
        workspace.routes.find((route) => route.status === 'available')?.id ??
        workspace.routes[0]?.id ??
        ''
    );
}

export function buildRisuGenerationPreset(
    workspace: ProviderWorkspaceDto,
    hints: RisuCompatibilityHints,
    modelRouteId: string,
    id: string,
    displayName: string,
): GenerationPresetInput {
    const specifications = parameterSpecifications(workspace, modelRouteId);
    const values = GENERATION_HINTS.flatMap(({ hint, parameter, scale }) => {
        const raw = hintNumber(hints, hint);
        const specification = specifications.find((candidate) => candidate.id === parameter);
        if (raw === null || raw === -1_000 || specification === undefined) return [];
        const normalized = scale === 'ratio' ? normalizeRisuRatio(raw) : raw;
        const value = parameterLiteral(specification, normalized);
        return value === null
            ? []
            : [
                  {
                      parameter_id: parameter,
                      state: { state: 'explicit', value },
                  } satisfies GenerationParameterDto,
              ];
    });
    return {
        id,
        model_route_id: modelRouteId,
        display_name: displayName,
        values,
        reasoning: {
            mode: 'provider_default',
            effort: null,
            budget_tokens: null,
            summary: 'provider_default',
            preserve_opaque_state: false,
        },
        prompt_cache: {
            mode: 'provider_default',
            ttl_kind: 'provider_default',
            ttl_seconds: null,
            context_reference: null,
        },
    };
}

export function buildRisuSummaryTask(
    hints: RisuCompatibilityHints,
    routeId: string,
    generationPresetId: string,
): TaskProfileDocumentDto | null {
    const id = hintString(hints, 'summary_task_id');
    if (id === null) return null;
    return {
        id,
        kind: 'memory_summary',
        route_id: routeId,
        generation_preset_id: generationPresetId,
        fallback_route_ids: [],
        embedding_dimensions: null,
        timeout_ms: 120_000,
        rate_limit: {
            requests: boundedPositiveInteger(
                hintNumber(hints, 'memory_summarization_requests_per_minute'),
                20,
                10_000,
            ),
            per_seconds: 60,
        },
        concurrency_limit: boundedPositiveInteger(
            hintNumber(hints, 'memory_summarization_max_concurrent'),
            2,
            1_024,
        ),
    };
}

export function buildRisuMemoryProfile(
    preset: CreatorPromptPresetDocumentDto,
    hints: RisuCompatibilityHints,
    summaryTaskId: string,
    displayName: string,
): CreatorMemoryProfileDocumentDto | null {
    const id = hintString(hints, 'memory_profile_id');
    const summaryTemplate = importedMemorySummaryTemplate(preset);
    if (hints.kind !== 'memory' || id === null || summaryTemplate === null) return null;
    const recencyWeight = boundedNonnegative(hintNumber(hints, 'memory_recent_memory_ratio'), 0.6);
    return {
        id,
        name: displayName,
        summary_task: summaryTaskId,
        embedding_task: null,
        turns_per_summary: boundedPositiveInteger(
            hintNumber(hints, 'memory_max_chats_per_summary'),
            8,
            10_000,
        ),
        recent_raw_budget: { max_tokens: 4_096 },
        episodic_budget: { max_tokens: 2_048 },
        semantic_budget: { max_tokens: 2_048 },
        retrieval_count: boundedPositiveInteger(
            hintNumber(hints, 'memory_query_chat_count'),
            8,
            10_000,
        ),
        recency_weight: recencyWeight > 0 ? recencyWeight : 1,
        // Risu does not provide a provider-neutral embedding route and vector
        // width. Similarity retrieval stays off until those are configured.
        similarity_weight: 0,
        importance_weight: boundedNonnegative(
            hintNumber(hints, 'memory_extra_summarization_ratio'),
            0,
        ),
        preserve_invalidated_records: hintBoolean(hints, 'memory_preserve_orphaned_memory') ?? true,
        summary_schema: 'lorepia.memory-summary.v1',
        summary_template: summaryTemplate,
    };
}

function importedMemorySummaryTemplate(
    preset: CreatorPromptPresetDocumentDto,
): NonNullable<CreatorMemoryProfileDocumentDto['summary_template']> | null {
    const block = preset.blocks.find((candidate) =>
        candidate.id.startsWith('risu-memory-summary-'),
    );
    const parts = block?.template?.parts;
    if (!parts?.every((part) => part.kind === 'text')) return null;
    const source = parts.map((part) => part.value).join('');
    const fragments = source.split('{{slot}}');
    if (fragments.length !== 2) return null;
    return {
        parts: [
            { kind: 'text', value: fragments[0] ?? '' },
            { kind: 'slot', name: 'memory_source' },
            { kind: 'text', value: fragments[1] ?? '' },
        ],
        max_output_chars: 262_144,
    };
}

function parameterSpecifications(
    workspace: ProviderWorkspaceDto,
    modelRouteId: string,
): ParameterSpecDto[] {
    const route = workspace.routes.find((candidate) => candidate.id === modelRouteId);
    const connection = workspace.connections.find(
        (candidate) => candidate.id === route?.connection_id,
    );
    return (
        workspace.templates.find((candidate) => candidate.id === connection?.template_id)
            ?.parameters ?? []
    );
}

const GENERATION_HINTS = [
    { hint: 'temperature_raw', parameter: 'temperature', scale: 'ratio' },
    { hint: 'top_p_raw', parameter: 'top_p', scale: 'ratio' },
    { hint: 'top_k_raw', parameter: 'top_k', scale: 'direct' },
    { hint: 'top_a_raw', parameter: 'top_a', scale: 'ratio' },
    { hint: 'min_p_raw', parameter: 'min_p', scale: 'ratio' },
    { hint: 'frequency_penalty_raw', parameter: 'frequency_penalty', scale: 'ratio' },
    { hint: 'presence_penalty_raw', parameter: 'presence_penalty', scale: 'ratio' },
    { hint: 'repetition_penalty_raw', parameter: 'repetition_penalty', scale: 'ratio' },
    { hint: 'max_response', parameter: 'max_output_tokens', scale: 'direct' },
] as const;

function parameterLiteral(
    specification: ParameterSpecDto,
    requested: number,
): ParameterLiteralDto | null {
    if (!Number.isFinite(requested)) return null;
    let value = requested;
    if (specification.minimum !== null) value = Math.max(specification.minimum, value);
    if (specification.maximum !== null) value = Math.min(specification.maximum, value);
    if (specification.step !== null && specification.step > 0) {
        const origin = specification.minimum ?? 0;
        value = origin + Math.round((value - origin) / specification.step) * specification.step;
    }
    if (specification.value_type === 'integer') {
        return { type: 'integer', value: Math.round(value) };
    }
    if (specification.value_type === 'number') return { type: 'number', value };
    return null;
}

function normalizeRisuRatio(value: number): number {
    return Math.abs(value) > 2 ? value / 100 : value;
}

function normalizeModelLabel(value: string): string {
    return value.toLowerCase().replaceAll(/[^a-z0-9]+/g, '');
}

function boundedPositiveInteger(value: number | null, fallback: number, maximum: number): number {
    if (value === null || value <= 0) return fallback;
    return Math.min(maximum, Math.max(1, Math.round(value)));
}

function boundedNonnegative(value: number | null, fallback: number): number {
    if (value === null || value < 0) return fallback;
    return value;
}
