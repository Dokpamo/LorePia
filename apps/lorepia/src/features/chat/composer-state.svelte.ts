import { tick } from 'svelte';
import { SvelteMap } from 'svelte/reactivity';

import type { ChatScrollLifecycle } from './chat-scroll.svelte';
import { shouldSubmitComposer } from './composer';

interface ChatComposerStateOptions {
    chatScroll: ChatScrollLifecycle;
    currentActiveMessageActionId(): string | null;
    currentDesktop(): boolean;
    onBranchReset(): void;
    onSubmit(): void | Promise<void>;
}

export class ChatComposerState {
    draft = $state('');
    compositionActive = $state(false);
    sending = $state(false);
    expanded = $state(false);
    canFullscreen = $state(false);
    overflows = $state(false);
    fullscreen = $state(false);
    textarea = $state<HTMLTextAreaElement | null>(null);
    field = $state<HTMLDivElement | null>(null);
    leadingAction = $state<HTMLButtonElement | null>(null);
    sendButton = $state<HTMLButtonElement | null>(null);
    fullscreenSurface = $state<HTMLFormElement | null>(null);
    fullscreenCloseButton = $state<HTMLButtonElement | null>(null);
    fullscreenSendButton = $state<HTMLButtonElement | null>(null);
    fullscreenTextRegion = $state<HTMLDivElement | null>(null);
    fullscreenTextarea = $state<HTMLTextAreaElement | null>(null);

    #activeDraftKey = '';
    readonly #drafts = new SvelteMap<string, string>();

    constructor(private readonly options: ChatComposerStateOptions) {}

    syncBranch(nextKey: string): void {
        if (nextKey === this.#activeDraftKey) return;
        if (this.#activeDraftKey !== '') this.#drafts.set(this.#activeDraftKey, this.draft);
        this.draft = this.#drafts.get(nextKey) ?? '';
        this.#activeDraftKey = nextKey;
        this.expanded = this.options.currentDesktop();
        this.options.onBranchReset();
        this.fullscreen = false;
        this.canFullscreen = false;
        this.overflows = false;
    }

    syncDesktop(desktop: boolean): void {
        if (desktop) {
            this.expanded = true;
            return;
        }
        const composerOwnsFocus =
            typeof document !== 'undefined' &&
            this.field?.contains(document.activeElement) === true;
        if (!composerOwnsFocus && this.draft.trim().length === 0) this.expanded = false;
    }

    beginSubmission(): string | null {
        if (this.sending || this.draft.trim().length === 0) return null;
        this.sending = true;
        return this.draft;
    }

    acceptSubmission(): void {
        this.draft = '';
        this.fullscreen = false;
        if (this.#activeDraftKey !== '') this.#drafts.delete(this.#activeDraftKey);
    }

    finishSubmission(): void {
        this.sending = false;
    }

    focusTextarea(): void {
        this.textarea?.focus();
    }

    handleKeydown(event: KeyboardEvent): void {
        if (
            shouldSubmitComposer(
                {
                    key: event.key,
                    shiftKey: event.shiftKey,
                    isComposing: event.isComposing,
                },
                this.compositionActive,
            )
        ) {
            event.preventDefault();
            void this.options.onSubmit();
        }
    }

    handleFullscreenKeydown(event: KeyboardEvent): void {
        if (event.key === 'Escape') {
            event.preventDefault();
            void this.setFullscreen(false);
            return;
        }
        this.handleKeydown(event);
    }

    measureComposer(node: HTMLTextAreaElement, draftValue: string) {
        let observer: ResizeObserver | null = null;
        let observedWidth: number | null = null;
        let scrollAnchorFrame: number | null = null;
        let overlayScrollFrame: number | null = null;
        let updateQueued = false;
        let pendingExpandedMeasurement: boolean | null = null;
        const field = node.closest<HTMLElement>('.composer-field');
        const composer = field?.closest<HTMLElement>('.composer');
        const chatPane = field?.closest<HTMLElement>('.chat-pane');
        const resizeTarget = composer ?? field ?? node;

        if (draftValue.trim().length === 0) {
            this.canFullscreen = false;
            this.overflows = false;
        }

        const update = (measureExpanded = this.expanded): void => {
            const appliesMeasurementLayout =
                measureExpanded && field !== null && !field.classList.contains('expanded');
            if (appliesMeasurementLayout) field.classList.add('measuring');
            const style = getComputedStyle(node);
            const parsedFontSize = Number.parseFloat(style.fontSize);
            const fontSize =
                Number.isFinite(parsedFontSize) && parsedFontSize >= 8 ? parsedFontSize : 16;
            const parsedLineHeight = Number.parseFloat(style.lineHeight);
            const lineHeight =
                Number.isFinite(parsedLineHeight) && parsedLineHeight > fontSize
                    ? parsedLineHeight
                    : fontSize *
                      (Number.isFinite(parsedLineHeight) && parsedLineHeight > 0
                          ? parsedLineHeight
                          : 1.45);
            const padding =
                (Number.parseFloat(style.paddingTop) || 0) +
                (Number.parseFloat(style.paddingBottom) || 0);
            const previousInlineHeight = node.style.height;
            node.style.height = '0px';
            const scrollHeight = Math.max(lineHeight + padding, node.scrollHeight);
            node.style.height = previousInlineHeight;
            const contentHeight = Math.max(lineHeight, scrollHeight - padding);
            const lineCount = Math.max(1, Math.ceil((contentHeight - 1) / lineHeight));
            const textRegion = node.closest<HTMLElement>('.composer-text-region');
            const parsedMaximumHeight = Number.parseFloat(
                textRegion ? getComputedStyle(textRegion).maxHeight : '',
            );
            const maximumHeight =
                Number.isFinite(parsedMaximumHeight) && parsedMaximumHeight >= lineHeight + padding
                    ? parsedMaximumHeight
                    : scrollHeight;
            if (appliesMeasurementLayout) field.classList.remove('measuring');
            const nextTextSize = Math.ceil(Math.min(scrollHeight, maximumHeight));
            field?.style.setProperty('--composer-text-size', `${String(nextTextSize)}px`);
            const overflows = scrollHeight > maximumHeight + 1;
            this.overflows = overflows;
            if (!overflows) {
                if (scrollAnchorFrame !== null) cancelAnimationFrame(scrollAnchorFrame);
                let remainingFrames = 3;
                const anchorVisibleLines = (): void => {
                    node.scrollTop = 0;
                    remainingFrames -= 1;
                    if (remainingFrames > 0) {
                        scrollAnchorFrame = requestAnimationFrame(anchorVisibleLines);
                        return;
                    }
                    scrollAnchorFrame = null;
                };
                node.scrollTop = 0;
                scrollAnchorFrame = requestAnimationFrame(anchorVisibleLines);
            } else if (scrollAnchorFrame !== null) {
                cancelAnimationFrame(scrollAnchorFrame);
                scrollAnchorFrame = null;
            }
            if (node.value.trim().length === 0) {
                this.canFullscreen = false;
                return;
            }
            this.canFullscreen = lineCount >= 2 || overflows;
        };
        const scheduleUpdate = (measureExpanded?: boolean): void => {
            if (measureExpanded !== undefined) {
                pendingExpandedMeasurement = measureExpanded;
            }
            if (updateQueued) return;
            updateQueued = true;
            queueMicrotask(() => {
                updateQueued = false;
                const nextExpandedMeasurement = pendingExpandedMeasurement ?? this.expanded;
                pendingExpandedMeasurement = null;
                update(nextExpandedMeasurement);
            });
        };
        const handleInput = (): void => update(this.expanded);
        const handleFocusIn = (event: FocusEvent): void => {
            if (event.relatedTarget instanceof Node && field?.contains(event.relatedTarget)) return;
            if (this.expanded) return;
            // Resolve the final text height before the single expansion transition starts.
            update(true);
            this.expanded = true;
        };
        const handleFocusOut = (event: FocusEvent): void => {
            if (event.relatedTarget instanceof Node && field?.contains(event.relatedTarget)) return;
            if (!this.expanded) return;
            if (this.options.currentDesktop()) return;
            if (node.value.trim().length > 0) return;
            this.expanded = false;
            scheduleUpdate(false);
        };
        const syncComposerOverlay = (fieldHeight: number): void => {
            if (
                composer === null ||
                composer === undefined ||
                chatPane === null ||
                chatPane === undefined
            ) {
                return;
            }
            const composerStyle = getComputedStyle(composer);
            const bottomInset = Number.parseFloat(composerStyle.paddingBottom) || 0;
            const verticalPadding =
                (Number.parseFloat(composerStyle.paddingTop) || 0) + bottomInset;
            const overlayHeight = Math.ceil(fieldHeight + verticalPadding);
            chatPane.style.setProperty(
                '--composer-field-height',
                `${String(Math.ceil(fieldHeight))}px`,
            );
            chatPane.style.setProperty(
                '--composer-field-bottom-inset',
                `${String(Math.ceil(bottomInset))}px`,
            );
            const previousOverlayHeight = Number.parseFloat(
                chatPane.style.getPropertyValue('--composer-overlay-height'),
            );
            if (
                Number.isFinite(previousOverlayHeight) &&
                Math.abs(previousOverlayHeight - overlayHeight) < 0.5
            ) {
                return;
            }
            chatPane.style.setProperty('--composer-overlay-height', `${String(overlayHeight)}px`);
            const scroller = this.options.chatScroll.scroller;
            const composerOwnsFocus = field?.contains(document.activeElement) ?? false;
            const overlayDelta = Number.isFinite(previousOverlayHeight)
                ? overlayHeight - previousOverlayHeight
                : 0;
            if (
                (!this.options.chatScroll.nearBottom && !(this.expanded && composerOwnsFocus)) ||
                scroller === null
            ) {
                return;
            }
            if (
                this.options.currentActiveMessageActionId() !== null &&
                this.expanded &&
                composerOwnsFocus &&
                Number.isFinite(previousOverlayHeight)
            ) {
                const anchoredScrollTop = scroller.scrollTop + overlayDelta;
                this.options.chatScroll.applyProgrammaticScrollPosition(
                    scroller,
                    anchoredScrollTop,
                );
                if (overlayScrollFrame !== null) cancelAnimationFrame(overlayScrollFrame);
                overlayScrollFrame = requestAnimationFrame(() => {
                    overlayScrollFrame = null;
                    this.options.chatScroll.applyProgrammaticScrollPosition(
                        scroller,
                        anchoredScrollTop,
                    );
                });
                return;
            }
            this.options.chatScroll.applyProgrammaticScrollPosition(
                scroller,
                scroller.scrollHeight,
            );
            if (overlayScrollFrame !== null) cancelAnimationFrame(overlayScrollFrame);
            overlayScrollFrame = requestAnimationFrame(() => {
                overlayScrollFrame = null;
                this.options.chatScroll.applyProgrammaticScrollPosition(
                    scroller,
                    scroller.scrollHeight,
                );
            });
        };

        node.addEventListener('input', handleInput);
        node.addEventListener('focus', handleFocusIn);
        node.addEventListener('blur', handleFocusOut);
        field?.addEventListener('focusin', handleFocusIn);
        field?.addEventListener('focusout', handleFocusOut);
        if (typeof ResizeObserver !== 'undefined') {
            observer = new ResizeObserver((entries) => {
                for (const entry of entries) {
                    if (entry.target === field) {
                        const borderBoxHeight = entry.borderBoxSize[0]?.blockSize;
                        syncComposerOverlay(borderBoxHeight ?? entry.contentRect.height);
                        continue;
                    }
                    const nextWidth = entry.contentRect.width;
                    if (observedWidth !== null && Math.abs(nextWidth - observedWidth) < 0.5)
                        continue;
                    observedWidth = nextWidth;
                    scheduleUpdate();
                }
            });
            observer.observe(resizeTarget);
            if (field !== null && field !== resizeTarget) observer.observe(field);
        }
        scheduleUpdate();

        return {
            update: (nextDraft: string): void => {
                if (nextDraft.trim().length === 0) {
                    this.canFullscreen = false;
                    this.overflows = false;
                }
                scheduleUpdate();
            },
            destroy: (): void => {
                node.removeEventListener('input', handleInput);
                node.removeEventListener('focus', handleFocusIn);
                node.removeEventListener('blur', handleFocusOut);
                field?.removeEventListener('focusin', handleFocusIn);
                field?.removeEventListener('focusout', handleFocusOut);
                observer?.disconnect();
                if (scrollAnchorFrame !== null) cancelAnimationFrame(scrollAnchorFrame);
                if (overlayScrollFrame !== null) cancelAnimationFrame(overlayScrollFrame);
                chatPane?.style.removeProperty('--composer-overlay-height');
            },
        };
    }

    async setFullscreen(nextOpen: boolean): Promise<void> {
        if (nextOpen && !this.canFullscreen) return;
        if (this.fullscreen === nextOpen) return;
        this.#syncFullscreenSurfaceOrigin();
        this.#syncFullscreenControlOrigins();
        this.#syncFullscreenTextOrigin();
        this.fullscreen = nextOpen;
        await tick();
        const target = nextOpen ? this.fullscreenTextarea : this.textarea;
        target?.focus({ preventScroll: true });
        const cursor = target?.value.length ?? 0;
        target?.setSelectionRange(cursor, cursor);
    }

    #setComposerControlOrigin(
        source: HTMLButtonElement | null,
        target: HTMLButtonElement | null,
    ): void {
        if (source === null || target === null) return;

        const sourceRect = source.getBoundingClientRect();
        const previousTransition = target.style.transition;
        const previousTransform = target.style.transform;

        // Measure the target at its layout position rather than at a previous
        // morph offset, then install the new start point without painting it.
        target.style.transition = 'none';
        target.style.transform = 'none';
        const targetRect = target.getBoundingClientRect();
        const originX =
            sourceRect.left + sourceRect.width / 2 - (targetRect.left + targetRect.width / 2);
        const originY =
            sourceRect.top + sourceRect.height / 2 - (targetRect.top + targetRect.height / 2);

        target.style.setProperty('--composer-control-origin-x', `${String(originX)}px`);
        target.style.setProperty('--composer-control-origin-y', `${String(originY)}px`);
        if (previousTransform === '') target.style.removeProperty('transform');
        else target.style.transform = previousTransform;
        target.getBoundingClientRect();
        if (previousTransition === '') target.style.removeProperty('transition');
        else target.style.transition = previousTransition;
    }

    #syncFullscreenControlOrigins(): void {
        this.#setComposerControlOrigin(this.leadingAction, this.fullscreenCloseButton);
        this.#setComposerControlOrigin(this.sendButton, this.fullscreenSendButton);
    }

    #syncFullscreenTextOrigin(): void {
        const source = this.textarea;
        const target = this.fullscreenTextarea;
        const targetRegion = this.fullscreenTextRegion;
        if (source === null || target === null || targetRegion === null) return;

        const sourceRect = source.getBoundingClientRect();
        const sourceStyle = getComputedStyle(source);
        const previousTransition = targetRegion.style.transition;
        const previousTransform = targetRegion.style.transform;

        targetRegion.style.transition = 'none';
        targetRegion.style.transform = 'none';
        const targetRect = target.getBoundingClientRect();
        const targetStyle = getComputedStyle(target);
        const sourceTextLeft = sourceRect.left + (Number.parseFloat(sourceStyle.paddingLeft) || 0);
        const sourceTextTop = sourceRect.top + (Number.parseFloat(sourceStyle.paddingTop) || 0);
        const targetTextLeft = targetRect.left + (Number.parseFloat(targetStyle.paddingLeft) || 0);
        const targetTextTop = targetRect.top + (Number.parseFloat(targetStyle.paddingTop) || 0);

        targetRegion.style.setProperty(
            '--composer-text-origin-x',
            `${String(sourceTextLeft - targetTextLeft)}px`,
        );
        targetRegion.style.setProperty(
            '--composer-text-origin-y',
            `${String(sourceTextTop - targetTextTop)}px`,
        );
        targetRegion.style.setProperty('--composer-text-origin-font-size', sourceStyle.fontSize);
        targetRegion.style.setProperty(
            '--composer-text-origin-line-height',
            sourceStyle.lineHeight,
        );
        if (previousTransform === '') targetRegion.style.removeProperty('transform');
        else targetRegion.style.transform = previousTransform;
        targetRegion.getBoundingClientRect();
        if (previousTransition === '') targetRegion.style.removeProperty('transition');
        else targetRegion.style.transition = previousTransition;
    }

    #syncFullscreenSurfaceOrigin(): void {
        const source = this.field;
        const target = this.fullscreenSurface;
        if (source === null || target === null) return;

        const sourceRect = source.getBoundingClientRect();
        const targetRect = target.getBoundingClientRect();
        const originTop = Math.max(0, sourceRect.top - targetRect.top);
        const originRight = Math.max(0, targetRect.right - sourceRect.right);
        const originBottom = Math.max(0, targetRect.bottom - sourceRect.bottom);
        const originLeft = Math.max(0, sourceRect.left - targetRect.left);
        const originRadius = getComputedStyle(source).borderTopLeftRadius || '0px';

        target.style.setProperty('--composer-origin-top', `${String(originTop)}px`);
        target.style.setProperty('--composer-origin-right', `${String(originRight)}px`);
        target.style.setProperty('--composer-origin-bottom', `${String(originBottom)}px`);
        target.style.setProperty('--composer-origin-left', `${String(originLeft)}px`);
        target.style.setProperty('--composer-origin-radius', originRadius);
    }
}
