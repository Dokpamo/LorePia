import type { CreatorControlDto } from '../../lib/ipc/contracts';

export function creatorControlOptions(
    control: CreatorControlDto,
): { value: string; label: string }[] {
    return control.choices.slice(0, 100).map((choice, index) => ({
        value: choice,
        label: control.choice_labels?.[index] ?? choice,
    }));
}
