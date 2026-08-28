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
    const normalizedText = $derived(
        profile === null || !enabled
            ? text
            : applyPortableTransforms(text, profile.output_transforms),
    );
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
        ).then((rendered) => {
            if (cancelled) return;
            const shadow = target.shadowRoot ?? target.attachShadow({ mode: 'open' });
            const baseStyle = document.createElement('style');
            baseStyle.textContent = BASE_STYLE;
            const importedStyle = document.createElement('style');
            importedStyle.textContent = extractStyleText(
                renderPortableDisplay(activeBackgroundMarkup, [], {
                    variables: activeVariables,
                    chatIndex: activeMessageIndex,
                    lastMessageId: activeLastMessageId,
                    lastCharacterMessage,
                }),
            );
            shadow.replaceChildren(baseStyle, importedStyle, rendered);
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
        :host { display: block; min-width: 0; color: inherit; }
        .portable-message { white-space: pre-wrap; overflow-wrap: anywhere; }
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
        const displaySource = renderPortableDisplay(source, activeProfile.display_transforms, {
            variables: activeVariables,
            chatIndex: activeMessageIndex,
            lastMessageId: activeLastMessageId,
            lastCharacterMessage,
        });
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

        const parsed = new DOMParser().parseFromString(
            `<div class="portable-message">${html}</div>`,
            'text/html',
        );
        const root = parsed.body.firstElementChild;
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
        const parsed = new DOMParser().parseFromString(markup, 'text/html');
        return [...parsed.querySelectorAll('style')]
            .map((style) => style.textContent)
            .join('\n')
            .replace(/@import[^;]+;/gi, '')
            .replace(/url\(\s*(['"]?)https?:[^)]+\)/gi, 'none');
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
        ]);
        const elements = [root, ...root.querySelectorAll('*')];
        for (const element of elements) {
            if (forbidden.has(element.tagName)) {
                element.remove();
                continue;
            }
            for (const attribute of [...element.attributes]) {
                const name = attribute.name.toLowerCase();
                if (name.startsWith('on') || name === 'srcdoc') {
                    element.removeAttribute(attribute.name);
                } else if (name.endsWith('-btn')) {
                    element.setAttribute('data-portable-action', attribute.value.slice(0, 512));
                    element.removeAttribute(attribute.name);
                } else if (name === 'style' && /url\s*\(/i.test(attribute.value)) {
                    element.removeAttribute(attribute.name);
                }
            }
            if (element instanceof HTMLImageElement || element instanceof HTMLMediaElement) {
                const source = element.getAttribute('src');
                if (source === null || !mediaUrls.has(source)) element.removeAttribute('src');
            }
            if (element instanceof HTMLAnchorElement) {
                const href = element.getAttribute('href') ?? '';
                if (!/^https?:\/\//i.test(href)) element.removeAttribute('href');
                element.rel = 'noreferrer noopener';
                element.target = '_blank';
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
    <div class="portable-host" bind:this={host}></div>
{:else}
    <MarkdownText text={normalizedText} />
{/if}

<style>
    .portable-host {
        display: block;
        min-width: 0;
    }
</style>
