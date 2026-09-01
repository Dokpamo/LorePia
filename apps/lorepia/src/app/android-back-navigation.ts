export interface MobileBackRoute {
    view: 'home' | 'chat' | 'create' | 'settings';
    pushed: boolean;
}

export type MobileBackAction = 'pop' | 'home' | 'exit';

export function resolveMobileBackAction(route: MobileBackRoute): MobileBackAction {
    if (route.pushed) return 'pop';
    return route.view === 'home' ? 'exit' : 'home';
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
