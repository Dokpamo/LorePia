<script lang="ts">
    import { untrack } from 'svelte';
    import { convertFileSrc } from '@tauri-apps/api/core';
    import { SvelteSet, SvelteURL } from 'svelte/reactivity';

    import type { CharacterRenderProfileDto, LorepiaClient } from '../../lib/ipc/contracts';
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
    import { observePortableMediaVisibility } from './portable-media-visibility';
    import {
        createPortableInputGate,
        portableBuildUsesContext,
    } from './portable-build-dependencies';
    import { PortableDocumentQueue } from './portable-document-queue';
    import { resolvePortableAssets } from './portable-asset-resolution';
    import { createPortableAssetSelector } from './portable-asset-selection';
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

    const selectNormalizationInputs = createPortableInputGate();
    const normalizationInputs = $derived(
        selectNormalizationInputs([text, profile, enabled] as const),
    );
    $effect(() => {
        const [source, activeProfile, active] = normalizationInputs;
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

    const documents = new PortableDocumentQueue();
    let publishedSurface: PortableSurface = 'message';
    let publishedFloating = false;
    $effect(() => {
        const target = frame;
        if (!target) return;
        const handleMessage = (event: MessageEvent<unknown>) => {
            const runtimeId = documents.activeRuntimeId;
            if (
                !documents.accepts(runtimeId) ||
                event.origin !== 'null' ||
                target.contentWindow === null ||
                event.source !== target.contentWindow ||
                !isPortableRendererMessage(event.data, runtimeId)
            )
                return;
            if (event.data.type === 'portable_action') onAction?.(event.data.action);
            else
                applyPortableFrameLayout(
                    target,
                    event.data,
                    publishedSurface === 'room',
                    publishedFloating,
                );
        };
        globalThis.addEventListener('message', handleMessage);
        return () => {
            documents.clear();
            globalThis.removeEventListener('message', handleMessage);
        };
    });
    $effect(() => {
        const target = frame;
        if (target && surface === 'message') {
            return observePortableMediaVisibility(target, () => documents.activeRuntimeId);
        }
    });
    const selectBuildInputs = createPortableInputGate();
    const buildInputs = $derived.by(() => {
        const background = backgroundMarkup ?? profile?.background_markup ?? '';
        const dynamic = portableBuildUsesContext(normalizedText, background, profile);
        return selectBuildInputs([
            frame,
            profile,
            client,
            normalizedText,
            messageIndex,
            dynamic ? lastMessageId : undefined,
            surface,
            floating,
            frameWidth,
            dynamic ? displayVariablesKey : '',
            dynamic ? lastCharacterMessage : '',
            characterName,
            userName,
            background,
            usesPortableMarkup,
        ] as const);
    });
    $effect(() => {
        const [
            target,
            activeProfile,
            activeClient,
            source,
            activeMessageIndex,
            activeLastMessageId,
            activeSurface,
            floatingRoom,
            screenWidth,
            ,
            activeLastCharacterMessage,
            activeCharacterName,
            activeUserName,
            activeBackgroundMarkup,
            portable,
        ] = buildInputs;
        const activeVariables = untrack(() => displayVariables);
        const names = {
            lastCharacterMessage: activeLastCharacterMessage,
            characterName: activeCharacterName,
            userName: activeUserName,
        };
        if (!portable || !target || !activeProfile || !activeClient) return;
        return documents.submit(
            [
                target,
                activeProfile,
                activeClient,
                activeSurface,
                floatingRoom,
                screenWidth,
                activeMessageIndex,
                activeCharacterName,
                activeUserName,
            ],
            async (signal) => {
                const rendered = await buildPortableDocument(
                    source,
                    activeProfile,
                    activeClient,
                    activeMessageIndex,
                    activeLastMessageId,
                    activeVariables,
                    activeSurface,
                    screenWidth,
                    activeBackgroundMarkup,
                    signal,
                    names,
                );
                signal.throwIfAborted();
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
                            ...names,
                            screenWidth,
                        },
                    ),
                    activeSurface,
                );
                signal.throwIfAborted();
                return { content: rendered, style: importedStyle };
            },
            (result, runtimeId, changed) => {
                publishedSurface = activeSurface;
                publishedFloating = floatingRoom;
                if (changed)
                    target.srcdoc = portableFrameDocument(
                        result.content,
                        result.style,
                        runtimeId,
                        activeSurface,
                    );
            },
        );
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
        signal: AbortSignal,
        names: {
            lastCharacterMessage: string | undefined;
            characterName: string | undefined;
            userName: string | undefined;
        },
    ): Promise<string> {
        const { lastCharacterMessage, characterName, userName } = names;
        const isCancelled = () => signal.aborted;
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
        const selectAsset = createPortableAssetSelector(activeProfile.assets, displaySource);
        const resolved = await resolvePortableAssets(
            references,
            selectAsset,
            activeClient,
            signal,
            rendererAssetUrl,
        );

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
