<script lang="ts">
    import { BookOpen, FileCode, SlidersHorizontal } from '@lucide/svelte';
    import { tick } from 'svelte';
    import { tr } from '../../lib/i18n';
    import type { CharacterRenderProfileDto } from '../../lib/ipc/contracts';
    import SettingsPanel from '../workspace/SettingsPanel.svelte';
    import NavigationRow from './NavigationRow.svelte';

    let {
        kind,
        profile,
        onclose,
        covered = false,
    }: {
        kind: 'lorebook' | 'scripts';
        profile: CharacterRenderProfileDto;
        onclose: () => void;
        covered?: boolean;
    } = $props();
    interface Entry {
        title: string;
        text?: string;
        code?: string;
    }
    let detail = $state<Entry | null>(null);
    let opener: HTMLButtonElement | undefined;
    const title = $derived(
        $tr(kind === 'lorebook' ? 'navigation.profileLorebook' : 'navigation.profileScripts'),
    );
    const lore = $derived(profile.runtime_knowledge.filter((item) => !item.folder));
    const rules = $derived([
        ...profile.display_transforms.map((rule) => ({
            ...rule,
            scope: $tr('navigation.displayRules'),
        })),
        ...profile.output_transforms.map((rule) => ({
            ...rule,
            scope: $tr('navigation.outputRules'),
        })),
    ]);
    function read(entry: Entry, trigger: HTMLButtonElement) {
        opener = trigger;
        detail = entry;
    }
    function closeDetail() {
        detail = null;
        void tick().then(() =>
            requestAnimationFrame(() => {
                if (!covered && opener?.isConnected && !opener.closest('[inert]'))
                    opener.focus({ preventScroll: true });
            }),
        );
    }
</script>

<SettingsPanel {title} kind="profile-resource" {onclose} covered={covered || detail !== null}>
    <div class="seed-profile-resource-list">
        {#if kind === 'lorebook'}
            {#each lore as entry (entry.id)}
                <NavigationRow
                    title={entry.name || $tr('navigation.profileLorebook')}
                    description={entry.primary_keys.join(' · ')}
                    onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                        read(
                            {
                                title: entry.name || $tr('navigation.profileLorebook'),
                                text: entry.content,
                            },
                            event.currentTarget,
                        )}
                >
                    {#snippet prefix()}<BookOpen />{/snippet}
                </NavigationRow>
            {:else}<p class="seed-profile-empty">{$tr('navigation.noLorebook')}</p>{/each}
        {:else}
            {#each profile.runtime_scripts as script (script.id)}
                <NavigationRow
                    title={script.name || $tr('navigation.profileScripts')}
                    description={script.language}
                    onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                        read(
                            {
                                title: script.name || $tr('navigation.profileScripts'),
                                code: script.source,
                            },
                            event.currentTarget,
                        )}
                >
                    {#snippet prefix()}<FileCode />{/snippet}
                </NavigationRow>
            {:else}<p class="seed-profile-empty">{$tr('navigation.noScripts')}</p>{/each}

            <section class="seed-profile-rules" aria-label={$tr('navigation.profileRules')}>
                <h2>{$tr('navigation.profileRules')}</h2>
                <p class="seed-profile-caption">{$tr('navigation.rulesExplanation')}</p>
                {#each rules as rule, i (i)}
                    <NavigationRow
                        title={$tr('navigation.numberedRule', { number: i + 1 })}
                        description={`${rule.scope} · ${rule.pattern}`}
                        onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                            read(
                                {
                                    title: $tr('navigation.numberedRule', { number: i + 1 }),
                                    text: rule.scope,
                                    code: `/${rule.pattern}/${rule.flags}\n→ ${rule.replacement}`,
                                },
                                event.currentTarget,
                            )}
                    >
                        {#snippet prefix()}<SlidersHorizontal />{/snippet}
                    </NavigationRow>
                {:else}<p class="seed-profile-empty">{$tr('navigation.noRules')}</p>{/each}
            </section>
        {/if}
    </div>
</SettingsPanel>

{#if detail}
    <SettingsPanel title={detail.title} kind="profile-resource" onclose={closeDetail} {covered}>
        {#if detail.text}<p class="seed-profile-resource-text">{detail.text}</p>{/if}
        {#if detail.code !== undefined}<pre
                class="seed-profile-resource-code">{detail.code}</pre>{/if}
    </SettingsPanel>
{/if}
