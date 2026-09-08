/**
 * Message catalog access.
 *
 * Every user-visible string lives in a locale catalog keyed by a stable
 * identifier. Components read messages through `tr` so a locale change
 * re-renders them; plain modules (controllers, validators) call `t` directly.
 *
 * Keys are checked at compile time: `MessageKey` is derived from the Korean
 * catalog, so a typo or a message removed from one locale is a type error
 * rather than a string that silently renders as its own key.
 */

import { derived, get, writable, type Readable } from 'svelte/store';

import { ko as koBase } from './ko';
import { koUiPreview } from './ko-ui-preview';
import { en } from './en';
import { koWorkspace } from './ko-workspace';
import { koWorkspaceAi } from './ko-workspace-ai';
import { koWorkspaceRuntime } from './ko-workspace-runtime';
import { koWorkspaceData } from './ko-workspace-data';

const ko = {
    ...koUiPreview,
    ...koBase,
    ...koWorkspace,
    ...koWorkspaceAi,
    ...koWorkspaceRuntime,
    ...koWorkspaceData,
};

export type MessageKey = keyof typeof ko;
export type MessageParams = Readonly<Record<string, string | number>>;

export const LOCALES = ['ko', 'en'] as const;
export type Locale = (typeof LOCALES)[number];

const CATALOGS: Record<Locale, Record<MessageKey, string>> = { ko, en: { ...ko, ...en } };

function storedLocale(): Locale {
    try {
        return localStorage.getItem('lorepia.locale') === 'en' ? 'en' : 'ko';
    } catch {
        return 'ko';
    }
}
const initialLocale = storedLocale();
export const locale = writable<Locale>(initialLocale);
if (typeof document !== 'undefined') document.documentElement.lang = initialLocale;
export function setLocale(value: Locale): void {
    locale.set(value);
    if (typeof document !== 'undefined') document.documentElement.lang = value;
    try {
        localStorage.setItem('lorepia.locale', value);
    } catch {
        /* The current selection remains usable. */
    }
}

/** Placeholders are named (`{count}`), never positional. */
const PLACEHOLDER = /\{([a-z0-9_]+)\}/gi;

function format(template: string, params: MessageParams | undefined): string {
    if (params === undefined) return template;
    return template.replace(PLACEHOLDER, (match, name: string) => {
        const value = params[name];
        return value === undefined ? match : String(value);
    });
}

function translate(current: Locale, key: MessageKey, params?: MessageParams): string {
    // `Record<MessageKey, string>` makes every catalog total, so a locale that
    // omits a message is a compile error rather than a blank string at runtime.
    return format(CATALOGS[current][key], params);
}

/** Reads one message outside a component. */
export function t(key: MessageKey, params?: MessageParams): string {
    return translate(get(locale), key, params);
}

/** Reactive reader for components: `{$tr('chat.send')}`. */
export const tr: Readable<(key: MessageKey, params?: MessageParams) => string> = derived(
    locale,
    (current) =>
        (key: MessageKey, params?: MessageParams): string =>
            translate(current, key, params),
);
