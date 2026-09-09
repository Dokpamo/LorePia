<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import { t, tr, locale } from '../../lib/i18n';
    import { openingLanguage } from '../../lib/i18n/opening-language';
    import type {
        CharacterGreetingCatalogDto,
        CharacterRenderProfileDto,
        LorepiaClient,
    } from '../../lib/ipc/contracts';
    import type { PersonaClientApi } from '../../features/personas/persona-contracts';
    import { PersonaController } from '../../features/personas/persona-controller';
    import ChoiceField from '../../ui/workspace/ChoiceField.svelte';
    import { useChoiceSheet } from '../../ui/workspace/choice-sheet.svelte';
    import {
        openingGroups,
        preferredOpening,
        type ProfileOpeningGroup,
    } from '../../ui/navigation/profile-openings';

    let {
        client,
        catalog,
        personaId = $bindable(''),
        greetingId = $bindable<string | null>(null),
        disabled = false,
    }: {
        client: LorepiaClient;
        catalog: CharacterGreetingCatalogDto | null;
        personaId?: string;
        greetingId?: string | null;
        disabled?: boolean;
    } = $props();
    const choices = useChoiceSheet();
    const personas = new PersonaController(untrack(() => client as Partial<PersonaClientApi>));
    const personaState = personas.state;
    let profile = $state.raw<CharacterRenderProfileDto | null>(null);
    const selectedPersona = $derived(
        $personaState.personas.find((item) => item.value.id === personaId),
    );
    const groups = $derived(
        openingGroups(catalog?.greetings ?? [], {
            introductions: Object.fromEntries(
                (profile?.greeting_previews ?? []).map((item) => [
                    item.id,
                    {
                        title: item.title ?? undefined,
                        body: item.excerpt,
                        language: item.language,
                        groupId: item.group_id,
                    },
                ]),
            ),
        }),
    );
    const selectedGroup = $derived(
        groups.find((group) => group.variants.some((item) => item.id === greetingId)),
    );
    const selectedVariant = $derived(
        selectedGroup?.variants.find((item) => item.id === greetingId),
    );

    onMount(() => {
        void personas.loadContext(null);
        return () => personas.destroy();
    });
    $effect(() => {
        const characterId = catalog?.character_id;
        const revisionId = catalog?.character_content_revision_id;
        profile = null;
        if (!characterId || !client.getCharacterRenderProfile) return;
        let current = true;
        void client
            .getCharacterRenderProfile(characterId)
            .then((value) => {
                if (
                    current &&
                    value.character_id === characterId &&
                    value.character_content_revision_id === revisionId
                )
                    profile = value;
            })
            .catch(() => {
                /* Reading aids are optional; the identity-bound catalog remains usable. */
            });
        return () => {
            current = false;
        };
    });
    function groupTitle(group: ProfileOpeningGroup) {
        return (
            preferredOpening(group, $locale, profile?.recommended_language, greetingId).title ??
            (group.kind === 'default'
                ? t('navigation.defaultStart')
                : t('navigation.numberedStart', { number: group.number }))
        );
    }
</script>

<div class="start-fields">
    <ChoiceField
        label={$tr('persona.title')}
        value={selectedPersona?.value.name ?? $tr('workspace.noPersona')}
        disabled={disabled ||
            $personaState.phase === 'loading' ||
            $personaState.phase === 'unavailable'}
        onopen={(opener: HTMLButtonElement) =>
            choices.open(
                {
                    label: t('persona.title'),
                    value: personaId,
                    options: [
                        { value: '', label: t('workspace.noPersona') },
                        ...$personaState.personas.map((item) => ({
                            value: item.value.id,
                            label: item.value.name,
                        })),
                    ],
                    onselect: (value) => {
                        personaId = value;
                    },
                },
                opener,
            )}
    />
    {#if selectedPersona?.value.description}
        <p class="field-hint">{selectedPersona.value.description}</p>
    {:else if $personaState.phase === 'loading'}
        <p class="field-hint" role="status">{$tr('workspace.loading')}</p>
    {:else if $personaState.phase === 'ready' && !$personaState.personas.length}
        <p class="field-hint">{$tr('persona.list.empty')}</p>
    {/if}
    {#if $personaState.error}
        <p class="ui-field-error" role="alert">{$personaState.error}</p>
        {#if $personaState.phase !== 'unavailable'}
            <button
                type="button"
                class="ui-action-button ui-pressable"
                {disabled}
                onclick={() => personas.loadContext(null)}
                ><span class="ui-press-visual">{$tr('workspace.retry')}</span></button
            >
        {/if}
    {:else if $personaState.next_cursor}
        <button
            type="button"
            class="ui-action-button ui-pressable"
            disabled={disabled || $personaState.phase === 'loading'}
            onclick={() => personas.loadMore()}
            ><span class="ui-press-visual">{$tr('persona.list.load_more')}</span></button
        >
    {/if}
    <ChoiceField
        label={$tr('workspace.startingScene')}
        value={selectedGroup
            ? (selectedVariant?.title ?? groupTitle(selectedGroup))
            : $tr('workspace.noGreeting')}
        disabled={disabled || !groups.length}
        onopen={(opener: HTMLButtonElement) =>
            choices.open(
                {
                    label: t('workspace.startingScene'),
                    value: selectedGroup?.id ?? '',
                    options: groups.map((group) => ({ value: group.id, label: groupTitle(group) })),
                    onselect: (value) => {
                        const group = groups.find((item) => item.id === value);
                        if (group)
                            greetingId = preferredOpening(
                                group,
                                $locale,
                                profile?.recommended_language,
                            ).id;
                    },
                },
                opener,
            )}
    />
    {#if selectedGroup && selectedGroup.variants.length > 1}
        <ChoiceField
            label={$tr('navigation.openingLanguage')}
            value={openingLanguage(selectedVariant?.language)}
            {disabled}
            onopen={(opener: HTMLButtonElement) =>
                choices.open(
                    {
                        label: t('navigation.openingLanguage'),
                        value: greetingId ?? '',
                        options: selectedGroup.variants.map((item) => ({
                            value: item.id,
                            label: openingLanguage(item.language),
                        })),
                        onselect: (value) => {
                            greetingId = value;
                        },
                    },
                    opener,
                )}
        />
    {/if}
    {#if selectedVariant?.body}<p class="field-hint scene-preview">{selectedVariant.body}</p>{/if}
</div>

<style>
    .start-fields {
        display: grid;
        gap: var(--ui-space-2);
        margin-bottom: var(--ui-space-4);
    }
    .start-fields :global(.ui-choice-field > .ui-press-visual) {
        padding-inline: var(--ui-space-4);
        background: var(--ui-paper);
    }
    .field-hint {
        margin: 0 var(--ui-space-4) var(--ui-space-2);
        color: var(--ui-muted);
        font-size: var(--ui-type-secondary);
        line-height: 1.5;
        overflow-wrap: anywhere;
        display: -webkit-box;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 2;
        line-clamp: 2;
        overflow: hidden;
    }
    .scene-preview {
        white-space: pre-wrap;
        -webkit-line-clamp: 3;
        line-clamp: 3;
    }
</style>
