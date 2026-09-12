import { describe, expect, it } from 'vitest';

import {
    hasPortableDisplayTransform,
    mergePortableDisplayVariables,
    renderPortableDisplay,
    renderPortableMacros,
} from './portable-display';

describe('portable display transforms', () => {
    it('renders bounded card actions and responsive width conditions', () => {
        const source = '{{#if {{? {{screen_width}} <= 600}}}}small{{:else}}wide{{/}}';
        expect(renderPortableMacros(source, { variables: {}, screenWidth: 393 }, source)).toBe(
            'small',
        );
        expect(renderPortableMacros(source, { variables: {}, screenWidth: 900 }, source)).toBe(
            'wide',
        );
        expect(
            renderPortableMacros('{{button::<script>::ToggleSettings}}', { variables: {} }, ''),
        ).toBe('<button type="button" card-btn="ToggleSettings">&lt;script&gt;</button>');
        expect(
            renderPortableMacros('{{button::Label::invalid action}}', { variables: {} }, ''),
        ).toBe('Label');
    });
    const context = {
        variables: { mode: '0', enabled: '1' },
        chatIndex: 4,
        lastMessageId: 5,
    };

    it('expands CBS patterns before regex compilation for empty and existing chats', async () => {
        const transform = {
            pattern:
                '{{#if {{not_equal::{{lastmessageid}}::-1}} }}${{/}}{{#if {{equal::{{lastmessageid}}::-1}} }}※※{{/}}',
            replacement:
                '{{#if {{equal::{{chat_index}}::{{lastmessageid}}}}}}<button>Settings</button>{{/}}',
            flags: 'gu<cbs>',
        };
        await expect(
            renderPortableDisplay('hello', [transform], {
                variables: {},
                chatIndex: 0,
                lastMessageId: 0,
            }),
        ).resolves.toBe('hello<button>Settings</button>');
        await expect(
            renderPortableDisplay('※※hello', [transform], {
                variables: {},
                chatIndex: -1,
                lastMessageId: -1,
            }),
        ).resolves.toBe('<button>Settings</button>hello');
        await expect(
            renderPortableDisplay('older', [transform], {
                variables: {},
                chatIndex: 0,
                lastMessageId: 1,
            }),
        ).resolves.toBe('older');
    });

    it('evaluates single-equals CBS settings and skips a disabled pattern entirely', async () => {
        const transforms = [
            {
                pattern: '{{#if {{? {{getglobalvar::toggle_stopbracket}}=1}}}}x{{/if}}',
                replacement: 'y',
                flags: 'g<cbs>',
            },
        ];
        await expect(
            renderPortableDisplay('x', transforms, { variables: { toggle_stopbracket: '0' } }),
        ).resolves.toBe('x');
        await expect(
            renderPortableDisplay('x', transforms, { variables: { toggle_stopbracket: '1' } }),
        ).resolves.toBe('y');
    });

    it.each(['g<cbs>', 'g<<cbs>>', 'g<cbs', '<sc<script>ript>g'])(
        'does not enable dotAll from modifier text in %s',
        async (flags) => {
            await expect(
                renderPortableDisplay(
                    'a\nb',
                    [{ pattern: 'a.b', replacement: 'bad', flags }],
                    context,
                ),
            ).resolves.toBe('a\nb');
        },
    );

    it('renders tagged blocks and evaluates nested portable conditions', async () => {
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
        await expect(renderPortableDisplay(source, transforms, context)).resolves.toContain(
            '<b>default</b>',
        );
        await expect(renderPortableDisplay(source, transforms, context)).resolves.not.toContain(
            '{{',
        );
    });

    it('leaves image-command transforms to the verified asset resolver', async () => {
        const source = '<img="Guide_smile">';
        await expect(
            renderPortableDisplay(
                source,
                [{ pattern: '<img="([^"]+)">', replacement: '<img src="$1">', flags: 'g' }],
                context,
            ),
        ).resolves.toBe(source);
    });

    it('ignores malformed expressions while applying later compatible rules', async () => {
        await expect(
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
        ).resolves.toBe('<pre>hello</pre>');
    });

    it('evaluates compound conditions and else branches', async () => {
        await expect(
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
        ).resolves.toBe('yes / good');
    });

    it('renders external pure conditionals with line-based legacy variables', async () => {
        const variables = mergePortableDisplayVariables({
            source: JSON.stringify('lang=1\nstatus_type=0'),
        });

        await expect(
            renderPortableDisplay(
                [
                    '{{#if_pure {{equal::{{getvar::lang}}::0}}}}English{{/if}}',
                    '{{#if_pure {{equal::{{getvar::lang}}::1}}}}Korean {{user}}{{/if}}',
                ].join(''),
                [],
                { variables, userName: 'Tester' },
            ),
        ).resolves.toBe('Korean Tester');
    });
});
