import { describe, expect, it } from 'vitest';

import { hasPortableDisplayTransform, renderPortableDisplay } from './portable-display';

describe('portable display transforms', () => {
    const context = {
        variables: { mode: '0', enabled: '1' },
        chatIndex: 4,
        lastMessageId: 5,
    };

    it('renders tagged blocks and evaluates nested portable conditions', () => {
        const transforms = [
            {
                pattern: '\\[Status\\]([\\s\\S]*?)\\[/Status\\]',
                replacement:
                    '<details><summary>Status</summary><pre>$1</pre>{{#if {{equal::{{getvar::mode}}::0}}}}<b>default</b>{{/}}</details>',
                flags: '',
            },
        ];
        const source = '[Status]\nHealth: 10/10\n[/Status]';

        expect(hasPortableDisplayTransform(source, transforms)).toBe(true);
        expect(renderPortableDisplay(source, transforms, context)).toContain('<b>default</b>');
        expect(renderPortableDisplay(source, transforms, context)).not.toContain('{{');
    });

    it('leaves image-command transforms to the verified asset resolver', () => {
        const source = '<img="Guide_smile">';
        expect(
            renderPortableDisplay(
                source,
                [{ pattern: '<img="([^"]+)">', replacement: '<img src="$1">', flags: 'g' }],
                context,
            ),
        ).toBe(source);
    });

    it('ignores malformed expressions while applying later compatible rules', () => {
        expect(
            renderPortableDisplay(
                '[Radio]hello[/Radio]',
                [
                    { pattern: '(', replacement: 'bad', flags: '' },
                    {
                        pattern: '\\[Radio\\]([\\s\\S]*)\\[/Radio\\]',
                        replacement: '<pre>$1</pre>',
                        flags: '',
                    },
                ],
                context,
            ),
        ).toBe('<pre>hello</pre>');
    });

    it('evaluates compound conditions and else branches', () => {
        expect(
            renderPortableDisplay(
                'value',
                [
                    {
                        pattern: 'value',
                        replacement:
                            '{{#when::1::and::1}}yes{{:else}}no{{/when}} / {{#when::0::or::not::true}}bad{{:else}}good{{/when}}',
                        flags: '',
                    },
                ],
                context,
            ),
        ).toBe('yes / good');
    });
});
