<script lang="ts">
    import { Blocks } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { ReadPageCursor } from '../../lib/ipc/contracts/pagination';
    import type { ContentModuleRuntimeTargetInput } from '../../features/orchestration/module-lifecycle-contracts';
    import SettingsPanel from '../workspace/SettingsPanel.svelte';
    import { readProfilePlugins, type ProfilePlugin } from './profile-plugins';
    let {
        client,
        characterId,
        characterName,
        runtimeTarget,
        covered = false,
        onclose,
    }: {
        client: LorepiaClient;
        characterId: string;
        characterName?: string;
        runtimeTarget?: ContentModuleRuntimeTargetInput;
        covered?: boolean;
        onclose: () => void;
    } = $props();
    let items = $state<ProfilePlugin[]>([]);
    let phase = $state<'loading' | 'ready' | 'error'>('loading');
    let next = $state<ReadPageCursor | null>(null);
    let truncated = $state(false);
    let retry = $state(0);
    const conversationId = $derived(runtimeTarget?.conversation_id);
    const branchId = $derived(runtimeTarget?.branch_id);
    let loadMore: (() => void) | undefined;
    $effect(() => {
        void retry;
        const id = characterId;
        const target =
            conversationId && branchId
                ? { conversation_id: conversationId, branch_id: branchId }
                : undefined;
        const connection = client;
        let current = true;
        const isCurrent = () => current;
        let pending = false;
        let cursor: ReadPageCursor | null = null;
        items = [];
        next = null;
        truncated = false;
        async function load() {
            if (pending || !isCurrent()) return;
            pending = true;
            phase = 'loading';
            try {
                const page = await readProfilePlugins(connection, id, target, cursor);
                if (!isCurrent()) return;
                items = [
                    ...new Map([...items, ...page.items].map((item) => [item.id, item])).values(),
                ];
                cursor = page.next;
                next = cursor;
                truncated = page.truncated;
                phase = 'ready';
            } catch {
                if (isCurrent()) phase = 'error';
            } finally {
                pending = false;
            }
        }
        loadMore = () => {
            void load();
        };
        void load();
        return () => {
            current = false;
        };
    });
</script>

<SettingsPanel
    title={$tr('settings.section.plugins.title')}
    kind="profile-resource"
    inlineTitle
    {covered}
    {onclose}
>
    {#if characterName}<p class="seed-profile-caption">{characterName}</p>{/if}
    <p class="seed-profile-caption">{$tr('navigation.profilePluginsIntro')}</p>
    {#if !runtimeTarget}<p class="seed-profile-caption">
            {$tr('navigation.profilePluginsBeforeChat')}
        </p>{/if}
    <div class="seed-profile-plugin-list">
        {#each items as item (item.id)}
            <article class="seed-profile-plugin">
                <Blocks aria-hidden="true" />
                <div>
                    <h2>{item.name}</h2>
                    {#if item.description}<p>{item.description}</p>{/if}
                    <p>
                        {item.scopes
                            .map((scope) => $tr(`navigation.pluginScope.${scope}`))
                            .join(' · ')}{#if item.version}
                            · {item.version}{/if}
                    </p>
                    <p>{$tr(`navigation.pluginStatus.${item.status}`)}</p>
                </div>
            </article>
        {/each}
    </div>
    {#if phase === 'loading'}<p class="seed-profile-caption" role="status">
            {$tr('workspace.loading')}
        </p>
    {:else if phase === 'error'}
        <p class="seed-profile-caption" role="alert">{$tr('navigation.pluginsFailed')}</p>
        <button class="seed-secondary ui-pressable" onclick={() => (retry += 1)}
            ><span class="ui-press-visual">{$tr('workspace.retry')}</span></button
        >
    {:else if !items.length && !next}<p class="seed-profile-empty">
            {$tr('navigation.noProfilePlugins')}
        </p>{/if}
    {#if next}<button
            class="seed-secondary ui-pressable"
            disabled={phase === 'loading'}
            onclick={() => loadMore?.()}
            ><span class="ui-press-visual">{$tr('navigation.morePlugins')}</span></button
        >{/if}
    {#if truncated}<p class="seed-profile-caption">{$tr('navigation.pluginsTruncated')}</p>{/if}
</SettingsPanel>

<style>
    .seed-profile-plugin-list {
        margin-top: 24px;
    }
    .seed-profile-plugin {
        display: flex;
        gap: 12px;
        padding-block: 16px;
    }
    .seed-profile-plugin > :global(svg) {
        width: 22px;
        height: 22px;
        flex: 0 0 22px;
        color: var(--ui-icon-ink);
    }
    .seed-profile-plugin > div {
        min-width: 0;
    }
    .seed-profile-plugin h2 {
        margin: 0;
        font-size: var(--ui-body);
        line-height: 1.375;
        font-weight: 600;
        overflow-wrap: anywhere;
    }
    .seed-profile-plugin p {
        margin: 6px 0 0;
        font-size: var(--ui-caption);
        line-height: 1.5;
        color: var(--ui-muted);
        overflow-wrap: anywhere;
    }
</style>
