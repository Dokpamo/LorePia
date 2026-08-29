<script lang="ts">
    import { convertFileSrc } from '@tauri-apps/api/core';
    import { SvelteMap, SvelteSet, SvelteURL } from 'svelte/reactivity';

    import type {
        CharacterRenderAssetDto,
        CharacterRenderProfileDto,
        LorepiaClient,
    } from '../../lib/ipc/contracts';
    import { t } from '../../lib/i18n';
    import MarkdownText from './MarkdownText.svelte';
    import {
        applyPortableTransforms,
        hasPortableDisplayTransform,
        type PortableRegexDiagnostic,
        renderPortableDisplay,
    } from './portable-display';
    import { sanitizePortableCss, sanitizePortableTree } from './portable-renderer-policy';
    import {
        isPortableRendererMessage,
        MAX_PORTABLE_RENDERER_HEIGHT,
        MIN_PORTABLE_RENDERER_HEIGHT,
        PORTABLE_RENDERER_CHANNEL,
    } from './portable-renderer-protocol';

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
        messageIndex?: number;
        lastMessageId?: number;
        variables?: Record<string, string>;
        backgroundMarkup?: string;
        lastCharacterMessage?: string;
        onAction?: (action: string) => void;
    }

    let {
        text,
        client,
        profile,
        enabled = true,
        messageIndex,
        lastMessageId,
        variables,
        backgroundMarkup,
        lastCharacterMessage = '',
        onAction,
    }: Props = $props();
    let frame = $state<HTMLIFrameElement | null>(null);
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
                onRegexDiagnostic: reportRegexDiagnostic,
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
            (/<(?:style|details|article|div|section|header|pre|button)\b/i.test(normalizedText) ||
                /<img\s*=/i.test(normalizedText) ||
                /\{\{(?:raw|audio|bgm)::/i.test(normalizedText) ||
                hasPortableDisplayTransform(normalizedText, profile.display_transforms)),
    );

    $effect(() => {
        const target = frame;
        const activeProfile = profile;
        const activeClient = client;
        const source = normalizedText;
        const activeMessageIndex = messageIndex;
        const activeLastMessageId = lastMessageId;
        const active = usesPortableMarkup;
        const actionHandler = onAction;
        const activeVariables = variables ?? activeProfile?.initial_variables ?? {};
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
                actionHandler?.(event.data.action);
                return;
            }
            target.style.height = `${String(event.data.height)}px`;
        };
        globalThis.addEventListener('message', handleMessage);
        void buildPortableDocument(
            source,
            activeProfile,
            activeClient,
            activeMessageIndex,
            activeLastMessageId,
            activeVariables,
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
                    },
                ),
            );
            if (isCancelled()) return;
            target.style.height = `${String(MIN_PORTABLE_RENDERER_HEIGHT)}px`;
            target.srcdoc = portableFrameDocument(rendered, importedStyle, runtimeId);
        });
        return () => {
            cancelled = true;
            globalThis.removeEventListener('message', handleMessage);
            target.removeAttribute('srcdoc');
        };
    });

    const BASE_STYLE = `
        html, body { display: block; min-width: 0; max-width: 100%; margin: 0; color: inherit;
            position: relative; contain: layout paint style; isolation: isolate; overflow: hidden; }
        .portable-message { max-width: 100%; white-space: pre-wrap; overflow: hidden;
            overflow-wrap: anywhere; }
        .portable-asset-frame { display: block; width: min(400px, 100%); margin: 12px auto; }
        .portable-asset-frame img { display: block; width: 100%; height: auto; border-radius: 10px; }
        .portable-audio { display: block; width: min(420px, 100%); margin: 10px auto; }
        .portable-asset-missing { display: block; padding: 8px 10px; border: 1px dashed currentColor;
            border-radius: 8px; opacity: .7; font-size: .85em; }
        details { white-space: normal; }
        button, input { font: inherit; }
    `;

    async function buildPortableDocument(
        source: string,
        activeProfile: CharacterRenderProfileDto,
        activeClient: LorepiaClient,
        activeMessageIndex: number | undefined,
        activeLastMessageId: number | undefined,
        activeVariables: Record<string, string>,
    ): Promise<string> {
        if (source.length > MAX_PORTABLE_SOURCE_CHARS) return portableLimitMarkup();
        const displaySource = await renderPortableDisplay(
            source,
            activeProfile.display_transforms,
            {
                variables: activeVariables,
                chatIndex: activeMessageIndex,
                lastMessageId: activeLastMessageId,
                lastCharacterMessage,
                onRegexDiagnostic: reportRegexDiagnostic,
                regexRuleScope: `${activeProfile.character_id}:${activeProfile.character_content_revision_id ?? 'legacy'}`,
            },
        );
        if (
            displaySource.length > MAX_PORTABLE_SOURCE_CHARS ||
            markupTagCount(displaySource) > MAX_PORTABLE_MARKUP_TAGS
        ) {
            return portableLimitMarkup();
        }
        const references = collectAssetReferences(displaySource);
        if (references === null) return portableLimitMarkup();
        const resolved = new SvelteMap<string, string | null>();
        const aliases = indexAssetAliases(activeProfile.assets);
        for (let offset = 0; offset < references.length; offset += MAX_PORTABLE_ASSET_CONCURRENCY) {
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

        let html = displaySource.replace(
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

        const template = document.createElement('template');
        template.innerHTML = `<div class="portable-message">${html}</div>`;
        const root = template.content.firstElementChild;
        if (!(root instanceof HTMLElement)) {
            const fallback = document.createElement('div');
            fallback.className = 'portable-message';
            fallback.textContent = displaySource;
            return fallback.outerHTML;
        }
        if (root.querySelectorAll('*').length > MAX_PORTABLE_MARKUP_TAGS) {
            return portableLimitMarkup();
        }
        sanitizePortableTree(root, new SvelteSet([...resolved.values()].filter(isString)));
        return root.outerHTML;
    }

    function portableFrameDocument(
        content: string,
        importedStyle: string,
        runtimeId: string,
    ): string {
        const nonce = globalThis.crypto.randomUUID().replaceAll('-', '');
        const scriptClose = '</scr' + 'ipt>';
        const mediaSources =
            'lorepia-asset: http://lorepia-asset.localhost https://lorepia-asset.localhost';
        const csp = [
            "default-src 'none'",
            `script-src 'nonce-${nonce}'`,
            "style-src 'unsafe-inline'",
            `img-src ${mediaSources}`,
            `media-src ${mediaSources}`,
            "connect-src 'none'",
            "font-src 'none'",
            "object-src 'none'",
            "base-uri 'none'",
            "form-action 'none'",
            "frame-src 'none'",
            "frame-ancestors 'none'",
        ].join('; ');
        return [
            '<!doctype html><html><head><meta charset="utf-8">',
            `<meta http-equiv="Content-Security-Policy" content="${escapeHtml(csp)}">`,
            '<meta name="referrer" content="no-referrer">',
            `<style>${BASE_STYLE}${importedStyle}</style>`,
            '</head><body>',
            content,
            `<script nonce="${nonce}">${portableFrameBridge(runtimeId)}${scriptClose}`,
            '</body></html>',
        ].join('');
    }

    function portableFrameBridge(runtimeId: string): string {
        const channel = JSON.stringify(PORTABLE_RENDERER_CHANNEL);
        const id = JSON.stringify(runtimeId);
        return String.raw`(() => {
            'use strict';
            const channel = ${channel};
            const runtimeId = ${id};
            const actionPattern = /^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,511}$/;
            const publish = (message) => parent.postMessage({ channel, runtimeId, ...message }, '*');
            const reportHeight = () => {
                const raw = Math.ceil(Math.max(
                    document.documentElement?.scrollHeight || 0,
                    document.body?.scrollHeight || 0,
                    ${String(MIN_PORTABLE_RENDERER_HEIGHT)}
                ));
                publish({
                    type: 'portable_resize',
                    height: Math.min(${String(MAX_PORTABLE_RENDERER_HEIGHT)}, Math.max(${String(MIN_PORTABLE_RENDERER_HEIGHT)}, raw))
                });
            };
            const playMedia = () => {
                for (const media of document.querySelectorAll('audio[autoplay]')) {
                    if (media instanceof HTMLMediaElement) void media.play().catch(() => undefined);
                }
            };
            document.addEventListener('click', (event) => {
                if (event.isTrusted !== true || !(event.target instanceof Element)) return;
                const control = event.target.closest('[data-portable-action]');
                if (!(control instanceof HTMLButtonElement) && !(control instanceof HTMLInputElement)) return;
                const action = control.getAttribute('data-portable-action')?.trim() || '';
                if (!actionPattern.test(action)) return;
                event.preventDefault();
                publish({ type: 'portable_action', action });
            }, true);
            document.addEventListener('pointerdown', playMedia, { once: true, capture: true });
            if (typeof ResizeObserver === 'function') {
                new ResizeObserver(reportHeight).observe(document.documentElement);
            }
            globalThis.addEventListener('load', () => { playMedia(); reportHeight(); }, { once: true });
            reportHeight();
        })();`;
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

    function extractStyleText(markup: string): string {
        if (markup === '') return '';
        const template = document.createElement('template');
        template.innerHTML = markup;
        return sanitizePortableCss(
            [...template.content.querySelectorAll('style')]
                .map((style) => style.textContent)
                .join('\n'),
        );
    }

    function escapeHtml(value: string): string {
        return value
            .replaceAll('&', '&amp;')
            .replaceAll('<', '&lt;')
            .replaceAll('>', '&gt;')
            .replaceAll('"', '&quot;')
            .replaceAll("'", '&#39;');
    }

    function isString(value: string | null): value is string {
        return value !== null;
    }
</script>

{#if usesPortableMarkup && client !== undefined}
    <div class="portable-boundary">
        <iframe
            class="portable-frame"
            bind:this={frame}
            title="카드 콘텐츠"
            sandbox="allow-scripts"
            referrerpolicy="no-referrer"
            allow="autoplay"
        ></iframe>
    </div>
{:else}
    <MarkdownText text={normalizedText} />
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
        max-height: min(70vh, 720px);
        overflow: auto;
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

    .portable-regex-warning {
        margin: 6px 0 0;
        color: var(--color-text-muted, currentColor);
        font-size: 0.78rem;
    }
</style>
