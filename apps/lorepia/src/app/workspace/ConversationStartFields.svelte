<script lang="ts">
    import { t, tr, locale } from '../../lib/i18n';
    import { openingLanguage } from '../../lib/i18n/opening-language';
    import type {
        CharacterGreetingCatalogDto,
        CharacterRenderProfileDto,
        ConversationMode,
        LorepiaClient,
    } from '../../lib/ipc/contracts';
    import type { PersonaController } from '../../features/personas/persona-controller';
    import ConversationPersonaField from './ConversationPersonaField.svelte';
    import ChoiceField from '../../ui/workspace/ChoiceField.svelte';
    import { useChoiceSheet } from '../../ui/workspace/choice-sheet.svelte';
    import { useTextEditor } from '../../ui/workspace/text-editor.svelte';
    import {
        openingGroups,
        preferredOpening,
        type ProfileOpeningGroup,
    } from '../../ui/navigation/profile-openings';

    let {
        client,
        catalog,
        name = $bindable(''),
        mode = $bindable<ConversationMode>('chat'),
        personaId = $bindable(''),
        greetingId = $bindable<string | null>(null),
        disabled = false,
        onaddpersona,
    }: {
        client: LorepiaClient;
        catalog: CharacterGreetingCatalogDto | null;
        name?: string;
        mode?: ConversationMode;
        personaId?: string;
        greetingId?: string | null;
        disabled?: boolean;
        onaddpersona: (controller: PersonaController) => void;
    } = $props();
    const choices = useChoiceSheet();
    const editor = useTextEditor();
    let profile = $state.raw<CharacterRenderProfileDto | null>(null);
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
        label={$tr('uiPreview.chatName')}
        value={name}
        {disabled}
        onopen={(opener: HTMLButtonElement) =>
            editor.open(
                {
                    label: t('uiPreview.chatName'),
                    value: name,
                    placeholder: t('uiPreview.newChat'),
                    maxlength: 60,
                    hint: t('uiPreview.settingsEditorHint'),
                    onchange: (value) => {
                        name = value.replaceAll('\n', ' ');
                    },
                },
                opener,
            )}
    />
    <ConversationPersonaField {client} bind:value={personaId} {disabled} onadd={onaddpersona} />
    <ChoiceField
        label={$tr('workspace.startingScene')}
        value={selectedGroup
            ? (selectedVariant?.title ?? groupTitle(selectedGroup))
            : $tr('workspace.noGreeting')}
        disabled={disabled || !groups.length}
        readonly={groups.length === 1}
        onopen={(opener: HTMLButtonElement) =>
            choices.open(
                {
                    label: t('workspace.startingScene'),
                    value: selectedGroup?.id ?? '',
                    options: groups.map((group) => ({
                        value: group.id,
                        label: groupTitle(group),
                        description: preferredOpening(
                            group,
                            $locale,
                            profile?.recommended_language,
                            greetingId,
                        ).body,
                    })),
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
    <ChoiceField
        label={$tr('uiPreview.conversationMode')}
        value={$tr(mode === 'chat' ? 'uiPreview.chatMode' : 'uiPreview.storyMode')}
        {disabled}
        onopen={(opener: HTMLButtonElement) =>
            choices.open(
                {
                    label: t('uiPreview.conversationMode'),
                    value: mode,
                    options: [
                        {
                            value: 'chat',
                            label: t('uiPreview.chatMode'),
                            description: t('uiPreview.chatModeHint'),
                        },
                        {
                            value: 'story',
                            label: t('uiPreview.storyMode'),
                            description: t('uiPreview.storyModeHint'),
                        },
                    ],
                    onselect: (value) => {
                        mode = value === 'story' ? 'story' : 'chat';
                    },
                },
                opener,
            )}
    />
</div>

<style>
    .start-fields {
        display: grid;
        gap: 0;
    }
    .start-fields :global(.ui-choice-field > .ui-press-visual) {
        min-height: var(--ui-row-compact);
        padding: var(--ui-space-4);
    }
    .start-fields :global(.ui-choice-label) {
        flex-shrink: 0;
    }
    .start-fields :global(.ui-choice-value) {
        min-width: 0;
        overflow-wrap: anywhere;
        display: -webkit-box;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 2;
        line-clamp: 2;
        overflow: hidden;
    }
    .start-fields :global(.ui-choice-field[role='group'] .ui-choice-value) {
        margin-right: calc(var(--ui-space-5) + var(--ui-space-3));
    }
    .start-fields :global(.ui-field-error) {
        margin-inline: var(--ui-space-4);
    }
</style>
