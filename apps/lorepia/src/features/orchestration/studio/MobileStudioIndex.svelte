<script lang="ts">
    import MobileMenuRow from '../../../components/mobile/MobileMenuRow.svelte';
    import { tr } from '../../../lib/i18n';
    import type { StudioSection } from '../studio-contracts';

    interface Props {
        onSelect: (section: StudioSection) => void;
    }
    let { onSelect }: Props = $props();
    const groups = [
        { title: 'mobile.studio.conversation', sections: ['prompt', 'memory'] },
        { title: 'mobile.studio.manage', sections: ['content', 'diagnostics'] },
    ] as const;
</script>

<div class="mobile-studio-index">
    {#each groups as group (group.title)}
        <section class="mobile-card mobile-menu-card">
            <h2>{$tr(group.title)}</h2>
            {#each group.sections as id (id)}
                <MobileMenuRow
                    label={$tr(
                        id === 'prompt'
                            ? 'studio.feature.prompt.title'
                            : `studio.section.${id}.title`,
                    )}
                    onSelect={() => onSelect(id)}
                />
            {/each}
        </section>
    {/each}
</div>
