import { expect, it } from 'vitest';
import type { CharacterRenderProfileDto } from '../../lib/ipc/contracts';
import { createPortableInputGate, portableBuildUsesContext } from './portable-build-dependencies';

it('treats malformed or unknown macros and any transform as dynamic', () => {
    for (const text of ['{{unknown::x}}', '{{unfinished', 'unfinished}}', '{{lastmessageid}}']) {
        expect(portableBuildUsesContext(text, '', null)).toBe(true);
        expect(portableBuildUsesContext('', text, null)).toBe(true);
    }
    for (const field of ['output_transforms', 'display_transforms']) {
        const profile = {
            output_transforms: [],
            display_transforms: [],
            [field]: [{}],
        } as unknown as CharacterRenderProfileDto;
        expect(portableBuildUsesContext('static', '', profile)).toBe(true);
    }
    expect(
        portableBuildUsesContext('<div><img=image></div>', '<style>x{color:red}</style>', null),
    ).toBe(false);
});

it('keeps one equal input tuple and invalidates changed object authority', () => {
    const select = createPortableInputGate();
    const profile = {};
    const first = select([profile, 'text', 1] as const);
    expect(select([profile, 'text', 1] as const)).toBe(first);
    expect(select([{}, 'text', 1] as const)).not.toBe(first);
    expect(select([profile, 'text', 1] as const)).not.toBe(first);
});
