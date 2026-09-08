<script lang="ts">
    import type { LorepiaAppState } from '../../app-controller';
    import { tr } from '../../../lib/i18n';
    let { appState }: { appState: LorepiaAppState } = $props();
</script>

{#if appState.providers.workspace.request_preview}
    {@const preview = appState.providers.workspace.request_preview}
    <section class="ui-settings-group" aria-label={$tr('workspaceAi.preview')}>
        <h2>{$tr('workspaceAi.preview')}</h2>
        {#each [preview.method, preview.origin, preview.path, preview.header_names.join(', ')] as value, index (index)}<div
                class="ui-settings-row"
            >
                <span>{value}</span>
            </div>{/each}
    </section>
{/if}
