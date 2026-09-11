import { afterEach, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import {
    chatDisplayPreferences,
    conversationDisplayMode,
    setConversationDisplayMode,
} from './chat-display';

afterEach(() => {
    chatDisplayPreferences.set({});
    localStorage.clear();
    vi.restoreAllMocks();
});

it('remembers the full-width layout per room across a renderer restart', () => {
    setConversationDisplayMode('room-a', 'default');
    expect(conversationDisplayMode('room-a', 'chat', {})).toBe('default');
    expect(conversationDisplayMode('room-b', 'chat', {})).toBe('chat');
    setConversationDisplayMode('room-a', 'chat');
    expect(conversationDisplayMode('room-a', 'chat', {})).toBe('chat');
});

it('preserves native modes for existing rooms and ignores stale or malformed preferences', () => {
    expect(conversationDisplayMode('old-room', 'story', {})).toBe('story');
    setConversationDisplayMode('room-a', 'default');
    expect(conversationDisplayMode('room-a', 'story', get(chatDisplayPreferences))).toBe('story');
    localStorage.setItem('lorepia.chatDisplay.bad-room', 'invalid');
    expect(conversationDisplayMode('bad-room', 'chat', {})).toBe('chat');
});

it('keeps switching usable if device preference storage is unavailable', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
        throw new Error('unavailable');
    });
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
        throw new Error('unavailable');
    });
    setConversationDisplayMode('room-a', 'default');
    expect(conversationDisplayMode('room-a', 'chat', get(chatDisplayPreferences))).toBe('default');
    expect(conversationDisplayMode('room-b', 'chat', get(chatDisplayPreferences))).toBe('chat');
});
