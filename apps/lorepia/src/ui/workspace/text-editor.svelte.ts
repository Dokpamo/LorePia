import { getContext, setContext, tick } from 'svelte';
import type { EditorOrigin } from './editor-morph';

export interface TextEditRequest {
    label: string;
    value: string;
    placeholder?: string;
    maxlength?: number;
    hint?: string;
    requiredMessage?: string;
    applyOnDone?: boolean;
    search?: {
        items: { id: string; title: string; date: string }[];
        onselect: (id: string) => void;
    };
    message?: boolean;
    origin?: () => EditorOrigin;
    onchange: ((value: string) => void) | ((value: string) => Promise<boolean>);
    onsend?: () => void;
}

const key = Symbol('ui-preview-text-editor');

export class TextEditorState {
    request = $state<TextEditRequest | null>(null);
    present = $state(false);
    #opener: HTMLElement | null = null;
    #afterClose: (() => void) | undefined;

    open(request: TextEditRequest, opener?: HTMLElement) {
        this.#opener = opener ?? (document.activeElement as HTMLElement | null);
        this.#afterClose = undefined;
        this.request = request;
        this.present = true;
    }

    close(afterClose?: () => void) {
        this.#afterClose = afterClose;
        this.request = null;
    }

    finish() {
        if (this.request) return;
        this.present = false;
        const afterClose = this.#afterClose;
        this.#afterClose = undefined;
        void tick().then(() => {
            if (afterClose) afterClose();
            else this.restoreFocus();
        });
    }

    restoreFocus() {
        if (this.present) return;
        const opener = this.#opener;
        if (opener?.isConnected && !opener.closest('[inert]'))
            opener.focus({ preventScroll: true });
    }
}

export function provideTextEditor() {
    return setContext(key, new TextEditorState());
}

export function useTextEditor(): TextEditorState {
    return getContext(key);
}
