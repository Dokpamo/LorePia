<script lang="ts">
    import { untrack } from 'svelte';
    import { convertFileSrc } from '@tauri-apps/api/core';
    import { SvelteMap, SvelteSet, SvelteURL } from 'svelte/reactivity';

    import type {
        CharacterRenderAssetDto,
        CharacterRenderProfileDto,
        LorepiaClient,
    } from '../../lib/ipc/contracts';
    import { t } from '../../lib/i18n';
    import MarkdownText from './MarkdownText.svelte';
    import { escapeHtml, portableFrameDocument } from './portable-renderer-frame';
    import {
        applyPortableTransforms,
        hasPortableDisplayTransform,
        mergePortableDisplayVariables,
        type PortableRegexDiagnostic,
        renderPortableDisplay,
        renderPortableMacros,
    } from './portable-display';
    import { selectPortableRoomMarkup } from './portable-room-markup';
    import { applyPortableFrameLayout } from './portable-frame-layout';
    import {
        type PortableSurface,
        sanitizePortableCss,
        sanitizePortableTree,
    } from './portable-renderer-policy';
    import { isPortableRendererMessage } from './portable-renderer-protocol';

    const MAX_PORTABLE_SOURCE_CHARS = 262_144;
    const MAX_PORTABLE_MARKUP_TAGS = 4_096;
    const MAX_PORTABLE_ASSET_REFERENCES = 128;
    const MAX_PORTABLE_ASSET_ALIASES = 32_768;
    const MAX_PORTABLE_ASSET_CONCURRENCY = 8;

    interface IndexedAssetAlias {
        asset: CharacterRenderAssetDto;
        alias: string;
    }

    interface Props {
        text: string;
        client?: LorepiaClient;
        profile: CharacterRenderProfileDto | null;
        enabled?: boolean;
        surface?: PortableSurface;
        floating?: boolean;
        expandMacros?: boolean;
        messageIndex?: number;
        lastMessageId?: number;
        variables?: Record<string, string>;
        backgroundMarkup?: string;
        lastCharacterMessage?: string;
        characterName?: string;
        userName?: string;
        onAction?: (action: string) => void;
    }

    let {
        text,
        client,
        profile,
        enabled = true,
        surface = 'message',
        floating = false,
        expandMacros = enabled,
        messageIndex,
        lastMessageId,
        variables,
        backgroundMarkup,
        lastCharacterMessage = '',
        characterName,
        userName,
        onAction,
    }: Props = $props();
    let frame = $state<HTMLIFrameElement | null>(null);
    let frameWidth = $state(393);
    let normalizedText = $state('');
    let regexWarning = $state<string | null>(null);

    function reportRegexDiagnostic(diagnostic: PortableRegexDiagnostic): void {
        const rule = diagnostic.ruleIndex + 1;
        regexWarning =
            diagnostic.reason === 'execution_timeout'
                ? t('chat.portable.regex.timeout', { rule })
                : diagnostic.reason === 'invalid_pattern'
                  ? t('chat.portable.regex.invalid', { rule })
                  : t('chat.portable.regex.unavailable', { rule });
    }

    $effect(() => {
        const source = text;
        const activeProfile = profile;
        const active = enabled;
        let cancelled = false;
        const exceedsLimit =
            active && activeProfile !== null && source.length > MAX_PORTABLE_SOURCE_CHARS;
        normalizedText = exceedsLimit ? t('chat.portable.content_too_large') : source;
        regexWarning = null;
        if (activeProfile !== null && active && !exceedsLimit) {
            const ruleScope = `${activeProfile.character_id}:${activeProfile.character_content_revision_id ?? 'legacy'}`;
            void applyPortableTransforms(source, activeProfile.output_transforms, {
                phase: 'provider_output',
                ruleScope,
                onRegexDiagnostic: (diagnostic) => {
                    if (!cancelled) reportRegexDiagnostic(diagnostic);
                },
                isCancelled: () => cancelled,
            }).then((result) => {
                if (!cancelled)
                    normalizedText =
                        result.length > MAX_PORTABLE_SOURCE_CHARS
                            ? t('chat.portable.content_too_large')
                            : result;
            });
        }
        return () => {
            cancelled = true;
        };
    });
    const usesPortableMarkup = $derived(
        enabled &&
            profile !== null &&
            ((surface === 'room' && (backgroundMarkup ?? profile.background_markup) !== '') ||
                /<(?:style|details|article|div|section|header|pre|button)\b/i.test(
                    normalizedText,
                ) ||
                /<img\s*=/i.test(normalizedText) ||
                /\{\{(?:raw|audio|bgm)::/i.test(normalizedText) ||
                hasPortableDisplayTransform(normalizedText, profile.display_transforms)),
    );
    const displayVariables = $derived(
        mergePortableDisplayVariables(profile?.initial_variables ?? {}, variables ?? {}),
    );
    const displayVariablesKey = $derived(JSON.stringify(displayVariables));
    const portableText = $derived(
        !expandMacros || profile === null
            ? normalizedText
            : renderPortableMacros(
                  normalizedText,
                  {
                      variables: displayVariables,
                      characterName,
                      userName,
                  },
                  normalizedText,
              ),
    );

    $effect(() => {
        const target = frame;
        if (target === null) return;
        const measure = () => {
            const width = Math.round(target.clientWidth);
            if (width > 0) frameWidth = width;
        };
        measure();
        if (typeof ResizeObserver === 'function') {
            const observer = new ResizeObserver(measure);
            observer.observe(target);
            return () => observer.disconnect();
        }
        globalThis.addEventListener('resize', measure);
        return () => globalThis.removeEventListener('resize', measure);
    });

    $effect(() => {
        const target = frame;
        const activeProfile = profile;
        const activeClient = client;
        const source = normalizedText;
        const activeMessageIndex = messageIndex;
        const activeLastMessageId = lastMessageId;
        const active = usesPortableMarkup;
        const activeSurface = surface;
        const floatingRoom = floating;
        const screenWidth = frameWidth;
        void displayVariablesKey;
        const activeVariables = untrack(() => displayVariables);
        const activeBackgroundMarkup = backgroundMarkup ?? activeProfile?.background_markup ?? '';
        if (!active || target === null || activeProfile === null || activeClient === undefined) {
            return;
        }
        let cancelled = false;
        const isCancelled = (): boolean => cancelled;
        const runtimeId = globalThis.crypto.randomUUID();
        const handleMessage = (event: MessageEvent<unknown>): void => {
            if (
                cancelled ||
                event.origin !== 'null' ||
                target.contentWindow === null ||
                event.source !== target.contentWindow ||
                !isPortableRendererMessage(event.data, runtimeId)
            ) {
                return;
            }
            if (event.data.type === 'portable_action') {
                onAction?.(event.data.action);
                return;
            }
            applyPortableFrameLayout(target, event.data, activeSurface === 'room', floatingRoom);
        };
        globalThis.addEventListener('message', handleMessage);
        void buildPortableDocument(
            source,
            activeProfile,
            activeClient,
            activeMessageIndex,
            activeLastMessageId,
            activeVariables,
            activeSurface,
            screenWidth,
            activeBackgroundMarkup,
            isCancelled,
        ).then(async (rendered) => {
            if (isCancelled()) return;
            const importedStyle = extractStyleText(
                await renderPortableDisplay(
                    activeBackgroundMarkup.length > MAX_PORTABLE_SOURCE_CHARS
                        ? ''
                        : activeBackgroundMarkup,
                    [],
                    {
                        variables: activeVariables,
                        chatIndex: activeMessageIndex,
                        lastMessageId: activeLastMessageId,
                        lastCharacterMessage,
                        characterName,
                        userName,
                        screenWidth,
                    },
                ),
                activeSurface,
            );
            if (isCancelled()) return;
            target.srcdoc = portableFrameDocument(
                rendered,
                importedStyle,
                runtimeId,
                activeSurface,
            );
        });
        return () => {
            cancelled = true;
            globalThis.removeEventListener('message', handleMessage);
            // Keep the last complete frame visible while its replacement is built.
            // Its old bridge loses its listener immediately; unmount unloads the frame.
        };
    });

    async function buildPortableDocument(
        source: string,
        activeProfile: CharacterRenderProfileDto,
        activeClient: LorepiaClient,
        activeMessageIndex: number | undefined,
        activeLastMessageId: number | undefined,
        activeVariables: Record<string, string>,
        activeSurface: PortableSurface,
        screenWidth: number,
        activeBackgroundMarkup: string,
        isCancelled: () => boolean,
    ): Promise<string> {
        if (
            source.length > MAX_PORTABLE_SOURCE_CHARS ||
            activeBackgroundMarkup.length > MAX_PORTABLE_SOURCE_CHARS
        )
            return portableLimitMarkup();
        const displaySource = await renderPortableDisplay(
            source,
            activeProfile.display_transforms,
            {
                variables: activeVariables,
                chatIndex: activeMessageIndex,
                lastMessageId: activeLastMessageId,
                lastCharacterMessage,
                characterName,
                userName,
                onRegexDiagnostic: (diagnostic) => {
                    if (!isCancelled()) reportRegexDiagnostic(diagnostic);
                },
                isCancelled,
                screenWidth,
                regexRuleScope: `${activeProfile.character_id}:${activeProfile.character_content_revision_id ?? 'legacy'}`,
            },
        );
        if (isCancelled()) return '';
        const background = renderPortableMacros(
            activeBackgroundMarkup,
            {
                variables: activeVariables,
                chatIndex: activeMessageIndex,
                lastMessageId: activeLastMessageId,
                lastCharacterMessage,
                characterName,
                userName,
                screenWidth,
            },
            source,
        );
        const assetSource =
            activeSurface === 'room' ? `${displaySource}\n${background}` : displaySource;
        if (
            assetSource.length > MAX_PORTABLE_SOURCE_CHARS ||
            background.length > MAX_PORTABLE_SOURCE_CHARS ||
            markupTagCount(assetSource) > MAX_PORTABLE_MARKUP_TAGS ||
            markupTagCount(background) > MAX_PORTABLE_MARKUP_TAGS
        ) {
            return portableLimitMarkup();
        }
        const references = collectAssetReferences(assetSource);
        if (references === null) return portableLimitMarkup();
        const resolved = new SvelteMap<string, string | null>();
        const aliases = indexAssetAliases(activeProfile.assets);
        for (let offset = 0; offset < references.length; offset += MAX_PORTABLE_ASSET_CONCURRENCY) {
            if (isCancelled()) return '';
            await Promise.all(
                references
                    .slice(offset, offset + MAX_PORTABLE_ASSET_CONCURRENCY)
                    .map(async (reference) => {
                        const asset = selectAsset(aliases, reference, displaySource);
                        if (asset === null) {
                            resolved.set(reference, null);
                            return;
                        }
                        try {
                            const delivery = await activeClient.resolveAssetDelivery({
                                selector: { kind: 'asset_id', asset_id: asset.asset_id },
                            });
                            if (delivery.asset_id !== asset.asset_id) {
                                resolved.set(reference, null);
                                return;
                            }
                            resolved.set(reference, rendererAssetUrl(delivery.sha256));
                        } catch {
                            resolved.set(reference, null);
                        }
                    }),
            );
        }

        const template = document.createElement('template');
        template.innerHTML = `<div class="portable-message">${resolveMarkupAssets(displaySource, resolved)}</div>`;
        const root = template.content.firstElementChild;
        if (!(root instanceof HTMLElement)) {
            const fallback = document.createElement('div');
            fallback.className = 'portable-message';
            fallback.textContent = displaySource;
            return fallback.outerHTML;
        }
        selectPortableRoomMarkup(
            root,
            activeSurface === 'room' ? resolveMarkupAssets(background, resolved) : background,
            activeSurface,
        );
        if (root.querySelectorAll('*').length > MAX_PORTABLE_MARKUP_TAGS) {
            return portableLimitMarkup();
        }
        sanitizePortableTree(
            root,
            new SvelteSet([...resolved.values()].filter(isString)),
            activeSurface,
        );
        return root.outerHTML;
    }

    function resolveMarkupAssets(
        source: string,
        resolved: ReadonlyMap<string, string | null>,
    ): string {
        let html = source.replace(
            /<img\s*=\s*(?:"([^"]+)"|'([^']+)'|([^>\s]+))\s*\/?\s*>/gi,
            (
                _match,
                doubleQuoted: string | undefined,
                singleQuoted: string | undefined,
                bare: string | undefined,
            ) => {
                const reference = (doubleQuoted ?? singleQuoted ?? bare ?? '').trim();
                const url = resolved.get(reference);
                return url === undefined || url === null
                    ? `<span class="portable-asset-missing">${escapeHtml(reference)}</span>`
                    : `<span class="portable-asset-frame"><img src="${escapeHtml(url)}" alt="${escapeHtml(reference)}" loading="lazy" decoding="async"></span>`;
            },
        );
        html = html.replace(/\{\{raw::([^{}]+)}}/gi, (_match, rawReference: string) => {
            const reference = rawReference.trim();
            return escapeHtml(resolved.get(reference) ?? reference);
        });
        html = html.replace(
            /\{\{(audio|bgm)::([^{}]+)}}/gi,
            (_match, kind: string, rawReference: string) => {
                const reference = rawReference.trim();
                const url = resolved.get(reference);
                return url === undefined || url === null
                    ? `<span class="portable-asset-missing">${escapeHtml(reference)}</span>`
                    : `<audio class="portable-audio" src="${escapeHtml(url)}" autoplay loop preload="auto" aria-label="${escapeHtml(kind === 'bgm' ? t('chat.portable.audio.background') : t('chat.portable.audio.clip'))}"></audio>`;
            },
        );

        return html;
    }

    function collectAssetReferences(source: string): string[] | null {
        const references = new SvelteSet<string>();
        for (const match of source.matchAll(
            /<img\s*=\s*(?:"([^"]+)"|'([^']+)'|([^>\s]+))\s*\/?\s*>/gi,
        )) {
            const reference = (match[1] ?? match[2] ?? match[3] ?? '').trim();
            if (reference !== '') references.add(reference);
            if (references.size > MAX_PORTABLE_ASSET_REFERENCES) return null;
        }
        for (const match of source.matchAll(/\{\{(?:raw|audio|bgm)::([^{}]+)}}/gi)) {
            const reference = (match[1] ?? '').trim();
            if (reference !== '') references.add(reference);
            if (references.size > MAX_PORTABLE_ASSET_REFERENCES) return null;
        }
        return [...references];
    }

    function selectAsset(
        aliases: readonly IndexedAssetAlias[],
        reference: string,
        source: string,
    ): CharacterRenderAssetDto | null {
        const wanted = normalizedAlias(reference);
        if (wanted === '') return null;
        let ranked = aliases.filter(
            ({ alias }) =>
                alias === wanted ||
                alias.startsWith(`${wanted}_`) ||
                alias.startsWith(`${wanted}.`),
        );
        if (ranked.length === 0 && wanted.includes('_')) {
            const fallback = `${wanted.slice(0, wanted.indexOf('_'))}_default`;
            ranked = aliases.filter(
                ({ alias }) => alias === fallback || alias.startsWith(`${fallback}.`),
            );
        }
        ranked = ranked.sort(
            (left, right) =>
                left.alias.length - right.alias.length ||
                left.alias.localeCompare(right.alias) ||
                left.asset.asset_id.localeCompare(right.asset.asset_id),
        );
        if (ranked.length === 0) return null;
        const exact = ranked.filter(({ alias }) => alias === wanted);
        const candidates = exact.length > 0 ? exact : ranked;
        return candidates[stableIndex(`${source}\0${reference}`, candidates.length)]?.asset ?? null;
    }

    function indexAssetAliases(assets: readonly CharacterRenderAssetDto[]): IndexedAssetAlias[] {
        const aliases: IndexedAssetAlias[] = [];
        for (const asset of assets) {
            for (const sourceAlias of asset.aliases) {
                const alias = normalizedAlias(sourceAlias);
                if (alias !== '') aliases.push({ asset, alias });
                if (aliases.length >= MAX_PORTABLE_ASSET_ALIASES) return aliases;
            }
        }
        return aliases;
    }

    function markupTagCount(value: string): number {
        let count = 0;
        for (const character of value) {
            if (character === '<' && ++count > MAX_PORTABLE_MARKUP_TAGS) break;
        }
        return count;
    }

    function portableLimitMarkup(): string {
        return `<div class="portable-message">${escapeHtml(t('chat.portable.content_too_large'))}</div>`;
    }

    function normalizedAlias(value: string): string {
        return (
            value
                .trim()
                .replace(/^['"]|['"]$/g, '')
                .replaceAll('\\', '/')
                .split('/')
                .at(-1)
                ?.toLocaleLowerCase() ?? ''
        );
    }

    function stableIndex(value: string, length: number): number {
        let hash = 2166136261;
        for (let index = 0; index < value.length; index += 1) {
            hash ^= value.charCodeAt(index);
            hash = Math.imul(hash, 16777619);
        }
        return length === 0 ? 0 : (hash >>> 0) % length;
    }

    function rendererAssetUrl(sha256: string): string | null {
        if (!/^[0-9a-f]{64}$/.test(sha256)) return null;
        let convertedValue: string;
        try {
            convertedValue = convertFileSrc(sha256, 'lorepia-asset');
        } catch {
            return null;
        }
        try {
            const converted = new SvelteURL(convertedValue);
            const origin =
                converted.protocol === 'lorepia-asset:' && converted.hostname === 'localhost'
                    ? 'lorepia-asset://localhost'
                    : (converted.protocol === 'http:' || converted.protocol === 'https:') &&
                        converted.hostname === 'lorepia-asset.localhost'
                      ? `${converted.protocol}//lorepia-asset.localhost`
                      : null;
            if (origin === null || converted.pathname !== `/${sha256}`) return null;
            return `${origin}/sha256/${sha256}`;
        } catch {
            return null;
        }
    }

    function extractStyleText(markup: string, activeSurface: PortableSurface): string {
        if (markup === '') return '';
        const template = document.createElement('template');
        template.innerHTML = markup;
        return [...template.content.querySelectorAll('style')]
            .map((style) => sanitizePortableCss(style.textContent, document, activeSurface))
            .join('\n');
    }

    function isString(value: string | null): value is string {
        return value !== null;
    }
</script>

{#if usesPortableMarkup && client !== undefined}
    <div class="portable-boundary" class:portable-room={surface === 'room'}>
        <iframe
            class="portable-frame"
            class:portable-floating={floating}
            bind:this={frame}
            title="카드 콘텐츠"
            sandbox="allow-scripts"
            referrerpolicy="no-referrer"
            allow="autoplay"
        ></iframe>
    </div>
{:else}
    <MarkdownText text={portableText} />
{/if}
{#if regexWarning !== null}
    <p class="portable-regex-warning" role="status">{regexWarning}</p>
{/if}

<style>
    .portable-boundary {
        position: relative;
        display: block;
        contain: layout paint style;
        isolation: isolate;
        max-width: 100%;
        overflow: clip;
    }

    .portable-room {
        height: 100%;
        max-height: none;
        overflow: hidden;
    }
    .portable-room .portable-frame {
        height: 100%;
    }

    .portable-frame {
        display: block;
        width: 100%;
        height: 32px;
        min-width: 0;
        max-width: 100%;
        border: 0;
        overflow: hidden;
    }
    .portable-floating {
        clip-path: path('M0,0Z');
    }

    .portable-regex-warning {
        margin: 6px 0 0;
        color: var(--color-text-muted, currentColor);
        font-size: 0.78rem;
    }
</style>
