import type { InteractionEffectEventDto, MemorySupervisorStatusDto } from './contracts';

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null;
}

function isBoundedText(value: unknown, maximumCharacters = 8192): value is string {
    return typeof value === 'string' && value.length <= maximumCharacters;
}

function isBoundedNativeInteractionText(value: unknown, maximumScalars = 8192): value is string {
    if (typeof value !== 'string') return false;
    let scalars = 0;
    for (const character of value) {
        const codePoint = character.codePointAt(0);
        if (
            codePoint === undefined ||
            (codePoint >= 0xd800 && codePoint <= 0xdfff) ||
            (codePoint <= 0x1f && codePoint !== 0x09 && codePoint !== 0x0a && codePoint !== 0x0d) ||
            (codePoint >= 0x7f && codePoint <= 0x9f)
        ) {
            return false;
        }
        scalars += 1;
        if (scalars > maximumScalars) return false;
    }
    if (new TextEncoder().encode(value).byteLength > 16 * 1024) return false;
    const normalized = value.toLowerCase();
    return !(
        (normalized.includes('<') && normalized.includes('>')) ||
        normalized.includes('javascript:') ||
        normalized.includes('data:text/html')
    );
}

function hasOnlyKeys(value: Record<string, unknown>, allowed: readonly string[]): boolean {
    const allowedKeys = new Set(allowed);
    return Object.keys(value).every((key) => allowedKeys.has(key));
}

function isSafeAssetDescriptor(value: unknown): boolean {
    if (!isRecord(value)) return false;
    if (
        !hasOnlyKeys(value, [
            'asset_id',
            'sha256',
            'media_type',
            'kind',
            'size_bytes',
            'width',
            'height',
            'duration_ms',
            'url',
        ]) ||
        !isBoundedText(value.asset_id, 512) ||
        value.asset_id === '' ||
        typeof value.sha256 !== 'string' ||
        !/^[0-9a-f]{64}$/.test(value.sha256) ||
        value.url !== `lorepia-asset://sha256/${value.sha256}` ||
        !Number.isSafeInteger(value.size_bytes) ||
        Number(value.size_bytes) <= 0 ||
        Number(value.size_bytes) > 64 * 1024 * 1024
    ) {
        return false;
    }
    const dimensionsAreSafe = [value.width, value.height, value.duration_ms].every(
        (dimension) =>
            dimension === null ||
            (typeof dimension === 'number' && Number.isSafeInteger(dimension) && dimension > 0),
    );
    if (!dimensionsAreSafe) return false;
    const mediaTypes = {
        image: ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/avif'],
        audio: ['audio/mpeg', 'audio/wav', 'audio/ogg'],
        video: ['video/mp4', 'video/webm'],
    } as const;
    if (value.kind !== 'image' && value.kind !== 'audio' && value.kind !== 'video') return false;
    const mediaType = value.media_type;
    if (typeof mediaType !== 'string') return false;
    return (
        mediaTypes[value.kind].some((allowedMediaType) => allowedMediaType === mediaType) &&
        (value.kind !== 'image' || Number(value.size_bytes) <= 16 * 1024 * 1024)
    );
}

export function isMemorySupervisorStatus(value: unknown): value is MemorySupervisorStatusDto {
    if (typeof value !== 'object' || value === null) return false;
    const candidate = value as Record<string, unknown>;
    return (
        hasOnlyKeys(candidate, [
            'sequence',
            'phase',
            'recovered_interrupted_jobs',
            'completed_jobs',
        ]) &&
        Number.isSafeInteger(candidate.sequence) &&
        Number(candidate.sequence) >= 0 &&
        typeof candidate.phase === 'string' &&
        ['not_started', 'recovered', 'running', 'failed'].includes(candidate.phase) &&
        Number.isSafeInteger(candidate.recovered_interrupted_jobs) &&
        Number(candidate.recovered_interrupted_jobs) >= 0 &&
        Number.isSafeInteger(candidate.completed_jobs) &&
        Number(candidate.completed_jobs) >= 0
    );
}

export function isInteractionEffectEvent(value: unknown): value is InteractionEffectEventDto {
    if (
        !isRecord(value) ||
        !hasOnlyKeys(value, [
            'delivery_id',
            'effect_id',
            'conversation_id',
            'branch_id',
            'resulting_state_revision',
            'event_created_at',
            'effect',
        ]) ||
        !isBoundedText(value.delivery_id, 512) ||
        value.delivery_id === ''
    ) {
        return false;
    }
    if (!isBoundedText(value.effect_id, 512) || value.effect_id === '' || !isRecord(value.effect)) {
        return false;
    }
    if (
        !isBoundedText(value.conversation_id, 512) ||
        value.conversation_id === '' ||
        !isBoundedText(value.branch_id, 512) ||
        value.branch_id === '' ||
        !Number.isSafeInteger(value.resulting_state_revision) ||
        Number(value.resulting_state_revision) < 0 ||
        !isBoundedText(value.event_created_at, 128) ||
        value.event_created_at === ''
    ) {
        return false;
    }
    const effect = value.effect;
    switch (effect.kind) {
        case 'state_changed':
            return hasOnlyKeys(effect, ['kind']);
        case 'knowledge_activated':
            return (
                hasOnlyKeys(effect, ['kind', 'entry_id']) &&
                isBoundedText(effect.entry_id, 512) &&
                effect.entry_id !== ''
            );
        case 'show_asset':
            return (
                hasOnlyKeys(effect, ['kind', 'asset', 'region']) &&
                isSafeAssetDescriptor(effect.asset) &&
                typeof effect.region === 'string' &&
                ['message', 'background', 'character_portrait', 'status_panel', 'audio'].includes(
                    effect.region,
                )
            );
        case 'play_audio':
            return (
                hasOnlyKeys(effect, ['kind', 'asset']) &&
                isSafeAssetDescriptor(effect.asset) &&
                isRecord(effect.asset) &&
                effect.asset.kind === 'audio'
            );
        case 'present_choices':
            return (
                hasOnlyKeys(effect, ['kind', 'choices']) &&
                Array.isArray(effect.choices) &&
                effect.choices.length > 0 &&
                effect.choices.length <= 64 &&
                effect.choices.every(
                    (choice) =>
                        isRecord(choice) &&
                        hasOnlyKeys(choice, ['id', 'label']) &&
                        isBoundedText(choice.id, 512) &&
                        choice.id !== '' &&
                        isBoundedNativeInteractionText(choice.label),
                )
            );
        case 'visible_system_event':
            return (
                hasOnlyKeys(effect, ['kind', 'text']) && isBoundedNativeInteractionText(effect.text)
            );
        case 'dice_rolled':
            return (
                hasOnlyKeys(effect, ['kind', 'count', 'sides', 'modifier', 'rolls', 'total']) &&
                Number.isSafeInteger(effect.count) &&
                Number(effect.count) > 0 &&
                Number(effect.count) <= 65_535 &&
                Number.isSafeInteger(effect.sides) &&
                Number(effect.sides) > 0 &&
                Number(effect.sides) <= 4_294_967_295 &&
                Number.isSafeInteger(effect.modifier) &&
                Array.isArray(effect.rolls) &&
                effect.rolls.length <= 100 &&
                effect.rolls.every(
                    (roll) =>
                        typeof roll === 'number' &&
                        Number.isSafeInteger(roll) &&
                        roll > 0 &&
                        roll <= Number(effect.sides),
                ) &&
                Number.isSafeInteger(effect.total)
            );
        case 'approval_pending':
            return (
                hasOnlyKeys(effect, ['kind', 'title', 'body', 'expires_after_seconds']) &&
                isBoundedNativeInteractionText(effect.title, 1024) &&
                effect.title !== '' &&
                isBoundedNativeInteractionText(effect.body) &&
                effect.body !== '' &&
                (effect.expires_after_seconds === null ||
                    (Number.isSafeInteger(effect.expires_after_seconds) &&
                        Number(effect.expires_after_seconds) > 0))
            );
        case 'projection_rejected':
            return (
                hasOnlyKeys(effect, ['kind', 'reason']) &&
                typeof effect.reason === 'string' &&
                ['unsafe_native_text', 'invalid_stored_effect', 'asset_unavailable'].includes(
                    effect.reason,
                )
            );
        default:
            return false;
    }
}
