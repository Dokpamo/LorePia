export interface ComposerKeyInput {
    key: string;
    shiftKey: boolean;
    isComposing: boolean;
}

export function shouldSubmitComposer(
    event: ComposerKeyInput,
    compositionActive: boolean,
    sendOnEnter = true,
): boolean {
    if (!sendOnEnter || event.key !== 'Enter' || event.shiftKey) {
        return false;
    }
    return !compositionActive && !event.isComposing;
}
