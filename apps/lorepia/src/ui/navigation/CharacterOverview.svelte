<script lang="ts">
    import { MessageSquare, Blocks } from '@lucide/svelte';
    import { getContext, tick } from 'svelte';
    import { t, tr } from '../../lib/i18n';
    import type {
        CharacterGreetingCatalogDto,
        CharacterRenderProfileDto,
        LorepiaClient,
    } from '../../lib/ipc/contracts';
    import type { SampleCharacter, Overlay } from '../workspace/view-types';
    import CharacterProfileHero from './CharacterProfileHero.svelte';
    import CharacterProfileAbout from './CharacterProfileAbout.svelte';
    import CharacterProfileStarts from './CharacterProfileStarts.svelte';
    import CharacterProfileResources from './CharacterProfileResources.svelte';
    import ProfileResourcePage from './ProfileResourcePage.svelte';
    import ProfilePluginsPage from './ProfilePluginsPage.svelte';
    import ProfileConversationsPage from './ProfileConversationsPage.svelte';
    import type { ConversationListItem } from './navigation-types';
    import type { ContentModuleRuntimeTargetInput } from '../../features/orchestration/module-lifecycle-contracts';
    import ProfileGallery from './ProfileGallery.svelte';
    import { profileImageGroups } from './profile-image-groups';
    import ProfileOpeningReader from './ProfileOpeningReader.svelte';
    import type { ProfileOpeningGroup } from './profile-openings';
    import type {
        CharacterProfilePresentation,
        ProfileResourceDetail,
        ProfileImage,
    } from './character-profile-types';
    import TrustedAsset from '../../features/assets/TrustedAsset.svelte';
    import {
        assetPresentationContext,
        type AssetPresentation,
    } from '../workspace/asset-presentation';
    import './character-profile.css';
    import './character-profile-content.css';
    import './character-profile-gallery.css';
    import SettingsPanel from '../workspace/SettingsPanel.svelte';
    import NavigationRow from './NavigationRow.svelte';
    const AssetView = getContext<AssetPresentation | undefined>(assetPresentationContext);
    let {
        character,
        client,
        onclose,
        onaction,
        onchat,
        conversations = [],
        conversationsLoading = false,
        conversationsError = null,
        onretryConversations = () => undefined,
        runtimeTarget,
        presentation,
        greetings = [],
        greetingRevisionId,
        selectedGreetingId = null,
        onselectGreeting,
        greetingsLoading = false,
        covered = false,
    }: {
        covered?: boolean;
        character: SampleCharacter;
        client: LorepiaClient;
        onclose: () => void;
        onaction: (kind: Overlay, trigger: HTMLButtonElement) => void;
        onchat: (id: string) => void;
        conversations?: ConversationListItem[];
        conversationsLoading?: boolean;
        conversationsError?: string | null;
        onretryConversations?: () => void;
        runtimeTarget?: ContentModuleRuntimeTargetInput;
        presentation?: CharacterProfilePresentation;
        greetings?: CharacterGreetingCatalogDto['greetings'];
        greetingRevisionId?: string | null;
        selectedGreetingId?: string | null;
        onselectGreeting: (id: string) => void;
        greetingsLoading?: boolean;
    } = $props();
    let chats = $state<HTMLButtonElement | null>(null);
    function closeChats() {
        const trigger = chats;
        chats = null;
        void tick().then(() =>
            requestAnimationFrame(() => {
                if (!covered && trigger?.isConnected && !trigger.closest('[inert]'))
                    trigger.focus({ preventScroll: true });
            }),
        );
    }
    let plugins = $state<HTMLButtonElement | null>(null);
    function closePlugins() {
        const trigger = plugins;
        plugins = null;
        void tick().then(() =>
            requestAnimationFrame(() => {
                if (!covered && trigger?.isConnected && !trigger.closest('[inert]'))
                    trigger.focus({ preventScroll: true });
            }),
        );
    }
    let resource = $state<ProfileResourceDetail | null>(null);
    let opening = $state<{
        group: ProfileOpeningGroup;
        revisionId: string | null | undefined;
        trigger: HTMLButtonElement;
    } | null>(null);
    let gallery = $state<{
        initialAlbum?: { title: string; images: ProfileImage[] };
        directView?: { images: ProfileImage[]; assetId: string };
        trigger: HTMLButtonElement;
    } | null>(null);
    let importedProfile = $state.raw<CharacterRenderProfileDto | null>(null);
    let heroAssetId = $state<string | null>(null);
    const currentProfile = $derived(
        importedProfile?.character_id === character.id ? importedProfile : null,
    );
    const imageGroups = $derived(
        profileImageGroups(currentProfile?.assets ?? [], presentation?.imageGroups),
    );
    const heroImages = $derived.by(() => {
        const cover = character.avatarAssetId
            ? [{ assetId: character.avatarAssetId, title: character.name }]
            : [];
        const declared =
            presentation?.heroAssetIds?.flatMap((id) => {
                const image = imageGroups
                    .flatMap((group) => group.images)
                    .find((item) => item.assetId === id);
                return image ? [image] : [];
            }) ?? [];
        const representatives = declared.length
            ? declared
            : imageGroups.map((group) => ({ assetId: group.coverAssetId, title: group.title }));
        return [
            ...new Map(
                [...cover, ...representatives].map((image) => [image.assetId, image]),
            ).values(),
        ];
    });
    function closeGallery() {
        const opener = gallery?.trigger;
        const trigger = opener?.matches('.seed-profile-slide[aria-hidden="true"]')
            ? opener.parentElement?.querySelector<HTMLButtonElement>(
                  '.seed-profile-slide[aria-hidden="false"]',
              )
            : opener;
        gallery = null;
        void tick().then(() =>
            requestAnimationFrame(() => {
                if (!covered && trigger?.isConnected && !trigger.closest('[inert]'))
                    trigger.focus({ preventScroll: true });
            }),
        );
    }
    const displayPresentation = $derived(
        presentation ??
            (currentProfile
                ? {
                      creator: { name: currentProfile.creator ?? '' },
                      note: currentProfile.creator_notes ?? '',
                      tags: currentProfile.tags ?? [],
                      recommendedLanguage: currentProfile.recommended_language,
                      introductions:
                          greetingRevisionId !== undefined &&
                          currentProfile.character_content_revision_id === greetingRevisionId
                              ? Object.fromEntries(
                                    (currentProfile.greeting_previews ?? []).map((item) => [
                                        item.id,
                                        {
                                            title: item.title ?? undefined,
                                            body: item.excerpt,
                                            language: item.language,
                                            groupId: item.group_id,
                                        },
                                    ]),
                                )
                              : undefined,
                  }
                : undefined),
    );
    function readOpening(group: ProfileOpeningGroup, trigger: HTMLButtonElement) {
        opening = { group, revisionId: greetingRevisionId, trigger };
    }
    function closeOpening() {
        const trigger = opening?.trigger;
        opening = null;
        void tick().then(() =>
            requestAnimationFrame(() => {
                if (!covered && trigger?.isConnected && !trigger.closest('[inert]'))
                    trigger.focus({ preventScroll: true });
            }),
        );
    }
    function receiveProfile(value: CharacterRenderProfileDto) {
        importedProfile = value;
    }
    let resourceTrigger: HTMLButtonElement | undefined;
    function openResource(detail: ProfileResourceDetail, trigger: HTMLButtonElement) {
        if (detail.imageGroups || detail.images) {
            gallery = {
                trigger,
                initialAlbum: detail.images
                    ? { title: detail.title, images: detail.images }
                    : undefined,
            };
            return;
        }
        resourceTrigger = trigger;
        resource = detail;
    }
    function closeResource() {
        resource = null;
        void tick().then(() =>
            requestAnimationFrame(() => {
                if (
                    !covered &&
                    !resource &&
                    resourceTrigger?.isConnected &&
                    !resourceTrigger.closest('[inert]')
                )
                    resourceTrigger.focus({ preventScroll: true });
            }),
        );
    }
</script>

<SettingsPanel
    title={$tr('navigation.characterInfo')}
    kind="character-profile"
    showTitle={false}
    collapsedTitle={character.name}
    {onclose}
    covered={covered ||
        resource !== null ||
        opening !== null ||
        gallery !== null ||
        chats !== null ||
        plugins !== null}
>
    {#snippet footer()}
        <button
            type="button"
            class="ui-submit seed-primary ui-pressable"
            onclick={(event) => onaction('new-chat', event.currentTarget)}
        >
            <span class="ui-press-visual">{$tr('navigation.startChat')}</span>
        </button>
    {/snippet}
    <CharacterProfileHero
        images={heroImages}
        bind:selectedAssetId={heroAssetId}
        onview={(assetId: string, trigger: HTMLButtonElement) => {
            gallery = { trigger, directView: { images: heroImages, assetId } };
        }}
        {character}
        {client}
        ondetails={(trigger: HTMLButtonElement) => onaction('card-info', trigger)}
    />
    <div class="seed-profile-content">
        <CharacterProfileAbout
            {client}
            description={character.description}
            presentation={displayPresentation}
            onread={(trigger: HTMLButtonElement) =>
                openResource(
                    { title: t('navigation.workDescription'), text: character.description },
                    trigger,
                )}
        />
        <CharacterProfileStarts
            {greetings}
            selectedId={selectedGreetingId}
            presentation={displayPresentation}
            onread={readOpening}
            loading={greetingsLoading}
            onshowall={(trigger: HTMLButtonElement) =>
                openResource(
                    { title: t('navigation.profileIntroduction'), openings: true },
                    trigger,
                )}
        />
        <CharacterProfileResources
            {character}
            {client}
            {presentation}
            onprofile={receiveProfile}
            onopen={openResource}
        />
        <div class="seed-group seed-profile-play-links">
            <NavigationRow
                title={$tr('navigation.viewChats')}
                onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                    (chats = event.currentTarget)}
                >{#snippet prefix()}<MessageSquare />{/snippet}</NavigationRow
            >
            <NavigationRow
                title={$tr('settings.section.plugins.title')}
                description={$tr('navigation.profilePluginsHint')}
                onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                    (plugins = event.currentTarget)}
            >
                {#snippet prefix()}<Blocks />{/snippet}
            </NavigationRow>
        </div>
    </div>
</SettingsPanel>
{#if chats}<ProfileConversationsPage
        {character}
        {conversations}
        loading={conversationsLoading}
        error={conversationsError}
        {covered}
        onclose={closeChats}
        onopen={onchat}
        onnew={(trigger: HTMLButtonElement) => onaction('new-chat', trigger)}
        onretry={onretryConversations}
    />{/if}
{#if plugins}<ProfilePluginsPage
        {client}
        characterId={character.id}
        characterName={character.name}
        {runtimeTarget}
        {covered}
        onclose={closePlugins}
    />{/if}
{#if resource?.collection}
    <ProfileResourcePage
        kind={resource.collection.kind}
        profile={resource.collection.profile}
        {covered}
        onclose={closeResource}
    />
{:else if resource}
    <SettingsPanel
        title={resource.title}
        kind="profile-resource"
        onclose={closeResource}
        covered={covered || opening !== null}
    >
        {#if resource.openings}
            <CharacterProfileStarts
                {greetings}
                selectedId={selectedGreetingId}
                presentation={displayPresentation}
                onread={readOpening}
                loading={greetingsLoading}
                fullPage
            />
        {:else if resource.assetId}<div class="seed-profile-resource-media">
                {#if AssetView}<AssetView
                        assetId={resource.assetId}
                        name={resource.title}
                        fit="contain"
                    />{:else}
                    <TrustedAsset
                        {client}
                        selector={/^[a-f0-9]{64}$/.test(resource.assetId)
                            ? { kind: 'sha256', sha256: resource.assetId }
                            : { kind: 'asset_id', asset_id: resource.assetId }}
                        alt={resource.title}
                    />
                {/if}
            </div>
        {:else if resource.code}<pre class="seed-profile-resource-code">{resource.code}</pre>
        {:else}<p class="seed-profile-resource-text">{resource.text}</p>{/if}
    </SettingsPanel>
{/if}

{#if opening}
    <ProfileOpeningReader
        {client}
        characterId={character.id}
        revisionId={opening.revisionId}
        activeRevisionId={greetingRevisionId}
        group={opening.group}
        selectedId={selectedGreetingId}
        recommendedLanguage={displayPresentation?.recommendedLanguage}
        profile={currentProfile}
        {covered}
        onclose={closeOpening}
        onstart={(id: string, trigger: HTMLButtonElement) => {
            onselectGreeting(id);
            onaction('new-chat', trigger);
        }}
    />
{/if}

{#if gallery}
    <ProfileGallery
        {client}
        groups={imageGroups}
        initialAlbum={gallery.initialAlbum}
        directView={gallery.directView}
        onimagechange={(assetId: string) => (heroAssetId = assetId)}
        {covered}
        onclose={closeGallery}
    />
{/if}
