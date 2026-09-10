<script lang="ts">
    import { importedLicenseLabel, importedText } from '../../lib/import-display';
    import { tr } from '../../lib/i18n';
    import type {
        ContentModuleCapabilityDto,
        ContentModuleLifecycleCandidateDto,
        ContentModuleSourceKindDto,
    } from './module-lifecycle-contracts';

    interface Props {
        candidate: ContentModuleLifecycleCandidateDto;
        busy: boolean;
        onReview: (moduleId: string) => void;
    }

    let { candidate, busy, onReview }: Props = $props();

    function shortHash(value: string): string {
        return value.length <= 24 ? value : `${value.slice(0, 12)}…${value.slice(-8)}`;
    }

    function sourceKindLabel(kind: ContentModuleSourceKindDto): string {
        const keys = {
            application_built_in: 'content.source.application_built_in',
            generated: 'content.source.generated',
            imported_package: 'content.source.imported_package',
            imported_standard: 'content.source.imported_standard',
            user_created: 'content.source.user_created',
        } as const;
        return $tr(keys[kind]);
    }

    function capabilityLabel(capability: ContentModuleCapabilityDto): string {
        const keys = {
            attachment_assets: 'content.capability.attachment_assets',
            audio_assets: 'content.capability.audio_assets',
            declarative_interactions: 'content.capability.declarative_interactions',
            high_risk_assets: 'content.capability.high_risk_assets',
            image_assets: 'content.capability.image_assets',
            knowledge: 'content.capability.knowledge',
            portable_runtime: 'content.capability.portable_runtime',
            prompt_fragments: 'content.capability.prompt_fragments',
            transforms: 'content.capability.transforms',
            variables: 'content.capability.variables',
            video_assets: 'content.capability.video_assets',
        } as const;
        return $tr(keys[capability]);
    }
</script>

<article class="candidate-card">
    <header>
        <div>
            <strong>{importedText(candidate.name)}</strong>
            <span>
                v{candidate.version} · {sourceKindLabel(candidate.source_kind)} ·
                {$tr('content.module.components', { count: candidate.component_count })}
            </span>
        </div>
        <code>{shortHash(candidate.revision_source_sha256)}</code>
    </header>
    <dl class="gate-grid">
        <div>
            <dt>{$tr('content.module.local_use')}</dt>
            <dd class:allowed={candidate.local_use_allowed}>
                {$tr(
                    candidate.local_use_allowed
                        ? 'content.module.allowed'
                        : 'content.module.blocked',
                )}
            </dd>
        </div>
        <div>
            <dt>{$tr('content.module.sharing')}</dt>
            <dd class:allowed={candidate.sharing_allowed}>
                {$tr(
                    candidate.sharing_allowed ? 'content.module.allowed' : 'content.module.blocked',
                )}
            </dd>
        </div>
    </dl>
    <p>
        {$tr('content.module.license')}:
        <strong>{importedLicenseLabel(candidate.license)}</strong>
        {#if candidate.author}
            · {importedText(candidate.author)}{/if}
    </p>
    {#if candidate.share_reasons.length > 0}
        <ul
            aria-label={$tr('content.module.share_reasons', {
                name: importedText(candidate.name),
            })}
        >
            {#each candidate.share_reasons as reason (reason)}
                <li>{importedText(reason)}</li>
            {/each}
        </ul>
    {/if}
    {#if candidate.required_capabilities.length > 0}
        <div class="capability-list" aria-label={$tr('content.capability.required')}>
            {#each candidate.required_capabilities as capability (capability)}
                <span>{capabilityLabel(capability)}</span>
            {/each}
        </div>
    {/if}
    {#if candidate.source_kind === 'imported_package'}
        <p class="lifecycle-note">
            {$tr('content.module.completed_approvals', {
                count: candidate.completed_package_approvals.length,
            })}
        </p>
    {/if}
    <button
        class="primary candidate-action"
        type="button"
        disabled={busy ||
            !candidate.local_use_allowed ||
            candidate.source_kind === 'application_built_in'}
        onclick={() => onReview(candidate.module_id)}
    >
        {$tr(
            candidate.source_kind === 'application_built_in'
                ? 'content.module.activation_blocked_builtin'
                : 'content.module.activation_review',
        )}
    </button>
</article>
