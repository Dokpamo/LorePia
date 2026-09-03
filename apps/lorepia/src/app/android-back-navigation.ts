export interface MobileBackRoute {
    view: 'home' | 'chat' | 'create' | 'settings';
    pushed: boolean;
}

export type MobileBackAction = 'pop' | 'home' | 'exit';

export function resolveMobileBackAction(route: MobileBackRoute): MobileBackAction {
    if (route.pushed) return 'pop';
    return route.view === 'home' ? 'exit' : 'home';
}

/** Ask the focused top-layer control to consume Back as Escape before routing. */
export function dismissMobileBackLayer(): boolean {
    if (typeof window === 'undefined' || typeof document === 'undefined') return false;

    const escapeEvent = new KeyboardEvent('keydown', {
        key: 'Escape',
        code: 'Escape',
        bubbles: true,
        cancelable: true,
        composed: true,
    });
    const target = document.activeElement instanceof HTMLElement ? document.activeElement : window;
    target.dispatchEvent(escapeEvent);
    if (escapeEvent.defaultPrevented) return true;

    const dialog = document.querySelector<HTMLDialogElement>('dialog[open][aria-modal="true"]');
    if (dialog === null) return false;
    const cancelEvent = new Event('cancel', { cancelable: true });
    dialog.dispatchEvent(cancelEvent);
    return cancelEvent.defaultPrevented;
}

interface AndroidBackWindow extends Window {
    isTauri?: boolean;
    __LOREPIA_ANDROID_BACK__?: () => MobileBackAction;
}

function androidWindow(): AndroidBackWindow | null {
    if (typeof window === 'undefined' || typeof navigator === 'undefined') return null;
    const target = window as AndroidBackWindow;
    return target.isTauri === true &&
        navigator.userAgent.toLocaleLowerCase('en-US').includes('android')
        ? target
        : null;
}

/** Expose the route decision consumed by MainActivity's Android Back callback. */
export function installAndroidBack(
    readRoute: () => MobileBackRoute,
    popRoute: () => void,
    goHome: () => void,
): () => void {
    const target = androidWindow();
    if (target === null) return () => undefined;
    const handler = (): MobileBackAction => {
        if (dismissMobileBackLayer()) return 'pop';
        const action = resolveMobileBackAction(readRoute());
        if (action === 'pop') popRoute();
        else if (action === 'home') goHome();
        return action;
    };
    target.__LOREPIA_ANDROID_BACK__ = handler;

    return () => {
        if (target.__LOREPIA_ANDROID_BACK__ === handler) {
            delete target.__LOREPIA_ANDROID_BACK__;
        }
    };
}
