import { writable } from 'svelte/store';

export type ChatTextSize = 'normal' | 'large';
function readSize(): ChatTextSize {
    try { return localStorage.getItem('lorepia.chatTextSize') === 'large' ? 'large' : 'normal'; }
    catch { return 'normal'; }
}
export const chatTextSize = writable<ChatTextSize>(readSize());
export function setChatTextSize(value: ChatTextSize): void {
    chatTextSize.set(value);
    document.documentElement.dataset.chatTextSize = value;
    try { localStorage.setItem('lorepia.chatTextSize', value); } catch { /* Keep the visual preference usable. */ }
}
export function initDisplay(): void { setChatTextSize(readSize()); }
