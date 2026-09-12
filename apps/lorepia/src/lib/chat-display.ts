import { writable } from 'svelte/store';
import type { ConversationMode } from './ipc/contracts';

export const CHAT_DISPLAY_MODES = [
    { value: 'default', label: 'uiPreview.defaultMode', hint: 'uiPreview.defaultModeHint' },
    { value: 'story', label: 'uiPreview.storyMode', hint: 'uiPreview.storyModeHint' },
    { value: 'chat', label: 'uiPreview.chatMode', hint: 'uiPreview.chatModeHint' },
] as const;
export type ChatDisplayMode = (typeof CHAT_DISPLAY_MODES)[number]['value'];
type Preferences = Readonly<Record<string, ChatDisplayMode>>;
const prefix = 'lorepia.chatDisplay.';
export const chatDisplayPreferences = writable<Preferences>({});

export function isChatDisplayMode(value: unknown): value is ChatDisplayMode {
    return CHAT_DISPLAY_MODES.some((mode) => mode.value === value);
}

/** Presentation adds a full-width layout without changing the native generation contract. */
export function generationMode(mode: ChatDisplayMode): ConversationMode {
    return mode === 'story' ? 'story' : 'chat';
}

export function conversationDisplayMode(
    id: string | undefined,
    nativeMode: ConversationMode,
    preferences: Preferences,
): ChatDisplayMode {
    if (!id) return nativeMode;
    let saved: unknown = preferences[id];
    if (!saved) {
        try {
            saved = localStorage.getItem(prefix + id);
        } catch {
            // The in-memory preference remains available when device storage is disabled.
        }
    }
    return isChatDisplayMode(saved) && generationMode(saved) === nativeMode ? saved : nativeMode;
}

export function setConversationDisplayMode(id: string, mode: ChatDisplayMode): void {
    try {
        localStorage.setItem(prefix + id, mode);
    } catch {
        // Keep the selected layout usable for this session.
    }
    chatDisplayPreferences.update((current) => ({ ...current, [id]: mode }));
}

/** One mounted workspace retains at most one resolved preference, including absence. */
export function createConversationDisplayModeResolver(): typeof conversationDisplayMode {
    let previous:
        | {
              id: string | undefined;
              nativeMode: ConversationMode;
              preferences: Preferences;
              mode: ChatDisplayMode;
          }
        | undefined;
    return (id, nativeMode, preferences) => {
        if (
            previous &&
            previous.id === id &&
            previous.nativeMode === nativeMode &&
            previous.preferences === preferences
        )
            return previous.mode;
        const mode = conversationDisplayMode(id, nativeMode, preferences);
        previous = { id, nativeMode, preferences, mode };
        return mode;
    };
}
