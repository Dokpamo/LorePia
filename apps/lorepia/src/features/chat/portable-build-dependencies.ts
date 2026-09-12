import type { CharacterRenderProfileDto } from '../../lib/ipc/contracts';

/** Unknown syntax and any transform conservatively keep every context dependency. */
export function portableBuildUsesContext(
    source: string,
    background: string,
    profile: CharacterRenderProfileDto | null,
): boolean {
    return (
        source.includes('{{') ||
        source.includes('}}') ||
        background.includes('{{') ||
        background.includes('}}') ||
        (profile?.output_transforms.length ?? 0) > 0 ||
        (profile?.display_transforms.length ?? 0) > 0
    );
}

/** One input tuple, not a document/asset approval cache. Equal props keep rune identity. */
export function createPortableInputGate() {
    let previous: readonly unknown[] | undefined;
    return <T extends readonly unknown[]>(inputs: T): T => {
        if (
            previous?.length === inputs.length &&
            previous.every((value, index) => Object.is(value, inputs[index]))
        )
            return previous as T;
        previous = inputs;
        return inputs;
    };
}
