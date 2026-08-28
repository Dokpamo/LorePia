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
        renderPortableDisplay,
    } from './portable-display';

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
        messageIndex = 0,
        lastMessageId = 0,
        variables,
        backgroundMarkup,
        lastCharacterMessage = '',
        onAction,
    }: Props = $props();
    let host = $state<HTMLDivElement | null>(null);
    let normalizedText = $state('');
    $effect(() => {
        const source = text;
        const activeProfile = profile;
        const active = enabled;
        let cancelled = false;
        normalizedText = source;
        if (activeProfile !== null && active) {
            void applyPortableTransforms(source, activeProfile.output_transforms).then((result) => {
                if (!cancelled) normalizedText = result;
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
        const target = host;
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
        const wasCancelled = (): boolean => cancelled;
        let activeShadow: ShadowRoot | null = null;
        const deferredMedia = new SvelteSet<HTMLMediaElement>();
        const resumeDeferredMedia = (): void => {
            const media = [...deferredMedia];
            deferredMedia.clear();
            document.removeEventListener('pointerdown', resumeDeferredMedia, true);
            for (const item of media) void item.play().catch(() => undefined);
        };
        const playPortableMedia = (media: HTMLMediaElement): void => {
            void media.play().catch(() => {
                if (cancelled) return;
                deferredMedia.add(media);
                document.addEventListener('pointerdown', resumeDeferredMedia, {
                    capture: true,
                    once: true,
                });
            });
        };
        const handleClick = (event: Event): void => {
            const element = event.target instanceof Element ? event.target : null;
            const actionElement = element?.closest('[data-portable-action]');
            const action =
                actionElement instanceof HTMLElement
                    ? actionElement.dataset.portableAction?.trim()
                    : undefined;
            if (action !== undefined && action !== '') actionHandler?.(action);
        };
        void buildPortableDocument(
            source,
            activeProfile,
            activeClient,
            activeMessageIndex,
            activeLastMessageId,
            activeVariables,
        ).then(async (rendered) => {
            if (wasCancelled()) return;
            const shadow = target.shadowRoot ?? target.attachShadow({ mode: 'open' });
            const baseStyle = document.createElement('style');
            baseStyle.textContent = BASE_STYLE;
            const importedStyle = document.createElement('style');
            importedStyle.textContent = extractStyleText(
                await renderPortableDisplay(activeBackgroundMarkup, [], {
                    variables: activeVariables,
                    chatIndex: activeMessageIndex,
                    lastMessageId: activeLastMessageId,
                    lastCharacterMessage,
                }),
            );
            if (wasCancelled()) return;
            shadow.replaceChildren(importedStyle, baseStyle, rendered);
            activeShadow = shadow;
            shadow.addEventListener('click', handleClick);
            for (const media of shadow.querySelectorAll<HTMLMediaElement>(
                '[data-portable-autoplay]',
            )) {
                playPortableMedia(media);
            }
        });
        return () => {
            cancelled = true;
            activeShadow?.removeEventListener('click', handleClick);
            document.removeEventListener('pointerdown', resumeDeferredMedia, true);
            deferredMedia.clear();
        };
    });

    const BASE_STYLE = `
        :host { display: block; min-width: 0; max-width: 100%; color: inherit;
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
        activeMessageIndex: number,
        activeLastMessageId: number,
        activeVariables: Record<string, string>,
    ): Promise<HTMLElement> {
        const displaySource = await renderPortableDisplay(
            source,
            activeProfile.display_transforms,
            {
                variables: activeVariables,
                chatIndex: activeMessageIndex,
                lastMessageId: activeLastMessageId,
                lastCharacterMessage,
            },
        );
        const references = collectAssetReferences(displaySource);
        const resolved = new SvelteMap<string, string | null>();
        await Promise.all(
            [...references].map(async (reference) => {
                const asset = selectAsset(activeProfile.assets, reference, displaySource);
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
                    : `<span class="portable-asset-frame"><img src="${escapeHtml(url)}" alt="${escapeHtml(reference)}" data-portable-media="1" loading="lazy" decoding="async"></span>`;
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
                    : `<audio class="portable-audio" src="${escapeHtml(url)}" data-portable-media="1" data-portable-autoplay="1" autoplay loop preload="auto" aria-label="${escapeHtml(kind === 'bgm' ? t('chat.portable.audio.background') : t('chat.portable.audio.clip'))}"></audio>`;
            },
        );

        const template = document.createElement('template');
        template.innerHTML = `<div class="portable-message">${html}</div>`;
        const root = template.content.firstElementChild;
        if (!(root instanceof HTMLElement)) {
            const fallback = document.createElement('div');
            fallback.className = 'portable-message';
            fallback.textContent = displaySource;
            return fallback;
        }
        sanitizePortableTree(root, new SvelteSet([...resolved.values()].filter(isString)));
        return document.importNode(root, true);
    }

    function collectAssetReferences(source: string): Set<string> {
        const references = new SvelteSet<string>();
        for (const match of source.matchAll(
            /<img\s*=\s*(?:"([^"]+)"|'([^']+)'|([^>\s]+))\s*\/?\s*>/gi,
        )) {
            const reference = (match[1] ?? match[2] ?? match[3] ?? '').trim();
            if (reference !== '') references.add(reference);
        }
        for (const match of source.matchAll(/\{\{(?:raw|audio|bgm)::([^{}]+)}}/gi)) {
            const reference = (match[1] ?? '').trim();
            if (reference !== '') references.add(reference);
        }
        return references;
    }

    function selectAsset(
        assets: CharacterRenderAssetDto[],
        reference: string,
        source: string,
    ): CharacterRenderAssetDto | null {
        const wanted = normalizedAlias(reference);
        if (wanted === '') return null;
        const aliases = assets.flatMap((asset) =>
            asset.aliases.map((alias) => ({ asset, alias: normalizedAlias(alias) })),
        );
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

    function sanitizePortableCss(value: string): string {
        const withoutComments = value.replace(/\/\*[\s\S]*?\*\//g, '');
        const withoutImports = withoutComments.replace(/@import\s+[^;]+;?/gi, '');
        if (
            withoutImports.includes('\\') ||
            withoutImports.includes('@') ||
            /:host(?:-context)?\b|::picker\b|appearance\s*:\s*base-select\b/i.test(withoutImports)
        ) {
            return '';
        }
        return withoutImports
            .replace(/url\s*\([^)]*\)/gi, 'none')
            .replace(/position\s*:\s*(?:fixed|sticky)\s*;?/gi, 'position: static;')
            .replace(/(?:^|[;{])\s*(?:inset|z-index)\s*:[^;}]*;?/gi, ';')
            .slice(0, 262_144);
    }

    function sanitizePortableTree(root: HTMLElement, mediaUrls: ReadonlySet<string>): void {
        const forbidden = new Set([
            'SCRIPT',
            'IFRAME',
            'OBJECT',
            'EMBED',
            'LINK',
            'META',
            'BASE',
            'FORM',
            'SVG',
            'MATH',
            'FOREIGNOBJECT',
            'PICTURE',
            'SOURCE',
            'TRACK',
            'DIALOG',
            'SELECT',
            'OPTION',
            'OPTGROUP',
        ]);
        const elements = [root, ...root.querySelectorAll('*')];
        for (const element of elements) {
            if (forbidden.has(element.tagName.toUpperCase())) {
                element.remove();
                continue;
            }
            if (element instanceof HTMLStyleElement) {
                element.textContent = sanitizePortableCss(element.textContent);
            }
            for (const attribute of [...element.attributes]) {
                const name = attribute.name.toLowerCase();
                if (
                    name.startsWith('on') ||
                    name === 'srcdoc' ||
                    name === 'srcset' ||
                    name === 'poster' ||
                    name === 'href' ||
                    name === 'xlink:href' ||
                    name === 'formaction' ||
                    name === 'action' ||
                    name === 'background' ||
                    name === 'cite' ||
                    name === 'data' ||
                    name === 'longdesc' ||
                    name === 'ping' ||
                    name === 'usemap' ||
                    name === 'popover' ||
                    name === 'popovertarget' ||
                    name === 'popovertargetaction' ||
                    name === 'command' ||
                    name === 'commandfor' ||
                    name === 'data-portable-action'
                ) {
                    element.removeAttribute(attribute.name);
                } else if (name.endsWith('-btn')) {
                    element.setAttribute('data-portable-action', attribute.value.slice(0, 512));
                    element.removeAttribute(attribute.name);
                } else if (name === 'style') {
                    const style = sanitizePortableCss(attribute.value);
                    if (style.trim() === '') element.removeAttribute(attribute.name);
                    else element.setAttribute('style', style);
                } else if (
                    name === 'src' &&
                    !(element instanceof HTMLImageElement) &&
                    !(element instanceof HTMLMediaElement)
                ) {
                    element.removeAttribute(attribute.name);
                }
            }
            if (element instanceof HTMLImageElement || element instanceof HTMLMediaElement) {
                const source = element.getAttribute('src');
                if (source === null || !mediaUrls.has(source)) element.removeAttribute('src');
            }
            if (element instanceof HTMLAnchorElement) {
                element.rel = 'noreferrer noopener';
                element.removeAttribute('target');
            }
            if (element instanceof HTMLInputElement) {
                const type = element.type.toLowerCase();
                if (!['button', 'checkbox', 'radio'].includes(type)) element.type = 'button';
                element.removeAttribute('form');
                element.removeAttribute('name');
            }
        }
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
        <div class="portable-host" bind:this={host}></div>
    </div>
{:else}
    <MarkdownText text={normalizedText} />
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

    .portable-host {
        display: block;
        min-width: 0;
        max-width: 100%;
        overflow: hidden;
    }
</style>
