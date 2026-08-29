import { describe, expect, it, vi } from 'vitest';

import {
    isPortableAction,
    sanitizePortableCss,
    sanitizePortableInlineStyle,
    sanitizePortableTree,
} from './portable-renderer-policy';
import { isPortableRendererMessage, PORTABLE_RENDERER_CHANNEL } from './portable-renderer-protocol';

describe('portable renderer policy', () => {
    it('uses parsed declarations to remove network, overlay, and variable indirection', () => {
        const mainHeadAppend = vi.spyOn(document.head, 'append');
        const result = sanitizePortableCss(`
            @import "https://evil.test/import.css";
            .hostile {
                --payload: url(https://evil.test/pixel);
                background: var(--payload);
                background-image: image-set(url(https://evil.test/a) 1x);
                position: fixed;
                inset: 0;
                z-index: 2147483647;
                pointer-events: all;
                width: 100vw;
                color: rgb(1, 2, 3);
            }
        `);

        expect(result).toBe('.hostile{color:rgb(1, 2, 3);}');
        expect(mainHeadAppend).not.toHaveBeenCalled();
    });

    it('keeps bounded animation and layout rules but rejects unknown at-rules', () => {
        const result = sanitizePortableCss(`
            @font-face { font-family: hostile; src: url(https://evil.test/font); }
            @keyframes pulse {
                from { opacity: .5; transform: scale(.98); }
                to { opacity: 1; transform: scale(1); }
            }
            @media (max-width: 500px) {
                .panel { display: grid; gap: 8px; }
            }
        `);

        expect(result).toContain('@keyframes pulse{');
        expect(result).toContain('opacity:0.5;');
        expect(result).toContain('transform:scale(.98);');
        expect(result).toContain('@media (max-width: 500px){.panel{display:grid;gap:8px;}}');
        expect(result).not.toContain('@font-face');
        expect(result).not.toContain('evil.test');
    });

    it('fails closed on CSS escapes and sanitizes inline declarations', () => {
        expect(sanitizePortableCss(String.raw`.x { position: f\69xed; color: red; }`)).toBe('');
        const inline = sanitizePortableInlineStyle(
            'color: red; position: absolute; background: url(https://evil.test); width: 100vh',
        );
        expect(inline).toContain('color:red;');
        expect(inline).not.toMatch(/(?:^|;)position:|url\s*\(|evil\.test|100vh/i);
    });

    it('enforces tag, attribute, asset, and exact action allowlists', () => {
        const template = document.createElement('template');
        template.innerHTML = `<div>
            <math><mtext>math</mtext></math>
            <svg><a xlink:href="https://evil.test">svg</a></svg>
            <form action="https://evil.test"><input autofocus></form>
            <img src="https://evil.test/a.png" srcset="https://evil.test/b.png">
            <button card-btn="safe_action" onclick="evil()">safe</button>
            <button other-btn="forged">forged</button>
            <div card-btn="not_interactive">no</div>
        </div>`;
        const root = template.content.firstElementChild;
        if (!(root instanceof HTMLElement)) throw new Error('fixture root missing');

        sanitizePortableTree(root, new Set());

        expect(root.querySelector('math, svg, form')).toBeNull();
        expect(
            root.querySelector('[src], [srcset], [autofocus], [onclick], [other-btn]'),
        ).toBeNull();
        expect(root.querySelector('button[data-portable-action="safe_action"]')).not.toBeNull();
        expect(root.querySelector('div[data-portable-action]')).toBeNull();
    });

    it('validates the narrow renderer message schema', () => {
        const runtimeId = '00000000-0000-4000-8000-000000000001';
        expect(isPortableAction('generate__radio__1')).toBe(true);
        expect(isPortableAction('contains spaces')).toBe(false);
        expect(
            isPortableRendererMessage(
                {
                    channel: PORTABLE_RENDERER_CHANNEL,
                    type: 'portable_action',
                    runtimeId,
                    action: 'generate__radio__1',
                },
                runtimeId,
            ),
        ).toBe(true);
        expect(
            isPortableRendererMessage(
                {
                    channel: PORTABLE_RENDERER_CHANNEL,
                    type: 'portable_resize',
                    runtimeId,
                    height: 721,
                },
                runtimeId,
            ),
        ).toBe(false);
    });
});
