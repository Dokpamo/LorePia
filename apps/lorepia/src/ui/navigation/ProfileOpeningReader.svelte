<script lang="ts">
    import { ChevronDown } from '@lucide/svelte';
    import type { CharacterGreetingDetailDto } from '../../lib/ipc/contracts/character';
    import { tr, locale } from '../../lib/i18n';
    import type { CharacterRenderProfileDto, LorepiaClient } from '../../lib/ipc/contracts';
    import SettingsPanel from '../workspace/SettingsPanel.svelte';
    import ChoiceSheet from '../workspace/ChoiceSheet.svelte';
    import { ChoiceSheetState } from '../workspace/choice-sheet.svelte';
    import ProfileOpeningText from './ProfileOpeningText.svelte';
    import { openingLanguage } from '../../lib/i18n/opening-language';
    import { preferredOpening, type ProfileOpeningGroup } from './profile-openings';
    import './profile-opening-reader.css';

    let {
        client,
        characterId,
        revisionId,
        activeRevisionId,
        group,
        selectedId,
        recommendedLanguage,
        profile,
        covered = false,
        onclose,
        onstart,
    }: {
        client: LorepiaClient;
        characterId: string;
        revisionId: string | null | undefined;
        activeRevisionId: string | null | undefined;
        group: ProfileOpeningGroup;
        selectedId: string | null;
        recommendedLanguage?: string | null;
        profile: CharacterRenderProfileDto | null;
        covered?: boolean;
        onclose: () => void;
        onstart: (id: string, trigger: HTMLButtonElement) => void;
    } = $props();
    let languageId = $state<string | null>(null);
    const choices = new ChoiceSheetState();
    const languageLabelId = $props.id();
    const variant = $derived(
        group.variants.find((item) => item.id === languageId) ??
            preferredOpening(group, $locale, recommendedLanguage, selectedId),
    );
    const title = $derived(
        variant.title ??
            (group.kind === 'default'
                ? $tr('navigation.defaultStart')
                : $tr('navigation.numberedStart', { number: group.number })),
    );
    let loaded = $state.raw<CharacterGreetingDetailDto | null>(null);
    let failed = $state(false);
    let retry = $state(0);
    const current = $derived(
        loaded?.greeting_id === variant.id &&
            loaded.character_id === characterId &&
            loaded.character_content_revision_id === revisionId &&
            revisionId === activeRevisionId
            ? loaded
            : null,
    );
    $effect(() => {
        const id = variant.id;
        const character = characterId;
        const revision = revisionId;
        const active = activeRevisionId;
        void retry;
        loaded = null;
        failed = false;
        if (!revision || revision !== active || !client.getCharacterGreetingDetail) {
            failed = true;
            return;
        }
        let disposed = false;
        void client
            .getCharacterGreetingDetail({
                character_id: character,
                character_content_revision_id: revision,
                greeting_id: id,
            })
            .then((value) => {
                if (disposed) return;
                if (
                    value.character_id !== character ||
                    value.character_content_revision_id !== revision ||
                    value.greeting_id !== id ||
                    !value.text
                ) {
                    failed = true;
                    return;
                }
                loaded = value;
            })
            .catch(() => {
                if (!disposed) failed = true;
            });
        return () => {
            disposed = true;
        };
    });
</script>

<SettingsPanel {title} kind="profile-resource" {onclose} covered={covered || choices.present}>
    {#snippet footer()}
        <button
            type="button"
            class="ui-submit seed-primary ui-pressable"
            disabled={!current || covered || choices.present}
            onclick={(event) => {
                if (current) onstart(current.greeting_id, event.currentTarget);
            }}
        >
            <span class="ui-press-visual">{$tr('navigation.startChat')}</span>
        </button>
    {/snippet}
    {#if group.variants.length > 1}
        <button
            type="button"
            class="seed-opening-language ui-pressable"
            aria-label={$tr('navigation.openingLanguage')}
            aria-describedby={languageLabelId}
            aria-haspopup="dialog"
            aria-expanded={choices.present}
            onclick={(event) => {
                const opener = event.currentTarget;
                choices.open(
                    {
                        label: $tr('navigation.openingLanguage'),
                        value: variant.id,
                        options: group.variants.map((option) => ({
                            value: option.id,
                            label: openingLanguage(option.language),
                        })),
                        onselect: (id: string) => {
                            if (id === variant.id) return;
                            languageId = id;
                            const body = opener.closest('.ui-overlay-body');
                            if (body) body.scrollTop = 0;
                        },
                    },
                    opener,
                );
            }}
        >
            <span class="ui-press-visual">
                <span>{$tr('navigation.openingLanguage')}</span>
                <strong id={languageLabelId}>{openingLanguage(variant.language)}</strong>
                <ChevronDown aria-hidden="true" />
            </span>
        </button>
    {/if}
    {#if current}
        <article class="seed-opening-reader" lang={variant.language ?? undefined}>
            <ProfileOpeningText
                text={current.text}
                {client}
                assets={profile && profile.character_content_revision_id === revisionId
                    ? profile.assets
                    : []}
            />
        </article>
    {:else if failed}
        <p class="seed-profile-caption" role="status">{$tr('navigation.openingUnavailable')}</p>
        <button type="button" class="seed-profile-more ui-pressable" onclick={() => retry++}>
            <span class="ui-press-visual">{$tr('navigation.openingRetry')}</span>
        </button>
    {:else}<p class="seed-profile-caption" role="status">{$tr('workspace.loading')}</p>{/if}
</SettingsPanel>

{#if choices.request}
    <ChoiceSheet
        request={choices.request}
        onclose={() => choices.close()}
        onclosed={() => choices.finish()}
    />
{/if}
