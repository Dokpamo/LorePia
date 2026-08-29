const MAX_PORTABLE_CSS_CHARS = 262_144;
const MAX_PORTABLE_CSS_RULES = 512;
const MAX_PORTABLE_CSS_VALUE_CHARS = 4_096;
const MAX_PORTABLE_ATTRIBUTE_CHARS = 4_096;

const PORTABLE_STYLE_RULE = 1;
const PORTABLE_MEDIA_RULE = 4;
const PORTABLE_KEYFRAMES_RULE = 7;
const PORTABLE_KEYFRAME_RULE = 8;

const ALLOWED_TAGS = new Set([
    'A',
    'ARTICLE',
    'ASIDE',
    'AUDIO',
    'B',
    'BLOCKQUOTE',
    'BR',
    'BUTTON',
    'CAPTION',
    'CODE',
    'DD',
    'DETAILS',
    'DIV',
    'DL',
    'DT',
    'EM',
    'FIGCAPTION',
    'FIGURE',
    'FOOTER',
    'H1',
    'H2',
    'H3',
    'H4',
    'H5',
    'H6',
    'HEADER',
    'HR',
    'I',
    'IMG',
    'INPUT',
    'LABEL',
    'LI',
    'MAIN',
    'MARK',
    'OL',
    'P',
    'PRE',
    'S',
    'SECTION',
    'SMALL',
    'SPAN',
    'STRONG',
    'STYLE',
    'SUB',
    'SUMMARY',
    'SUP',
    'TABLE',
    'TBODY',
    'TD',
    'TFOOT',
    'TH',
    'THEAD',
    'TR',
    'U',
    'UL',
]);

const ALLOWED_COMMON_ATTRIBUTES = new Set([
    'class',
    'dir',
    'hidden',
    'id',
    'lang',
    'role',
    'style',
    'title',
]);

const ALLOWED_CSS_PROPERTIES = new Set([
    'align-content',
    'align-items',
    'align-self',
    'animation',
    'animation-delay',
    'animation-direction',
    'animation-duration',
    'animation-fill-mode',
    'animation-iteration-count',
    'animation-name',
    'animation-play-state',
    'animation-timing-function',
    'aspect-ratio',
    'background',
    'background-attachment',
    'background-blend-mode',
    'background-clip',
    'background-color',
    'background-image',
    'background-origin',
    'background-position',
    'background-repeat',
    'background-size',
    'border',
    'border-block',
    'border-block-color',
    'border-block-end',
    'border-block-start',
    'border-block-style',
    'border-block-width',
    'border-bottom',
    'border-bottom-color',
    'border-bottom-left-radius',
    'border-bottom-right-radius',
    'border-bottom-style',
    'border-bottom-width',
    'border-collapse',
    'border-color',
    'border-inline',
    'border-inline-color',
    'border-inline-end',
    'border-inline-start',
    'border-inline-style',
    'border-inline-width',
    'border-left',
    'border-left-color',
    'border-left-style',
    'border-left-width',
    'border-radius',
    'border-right',
    'border-right-color',
    'border-right-style',
    'border-right-width',
    'border-spacing',
    'border-style',
    'border-top',
    'border-top-color',
    'border-top-left-radius',
    'border-top-right-radius',
    'border-top-style',
    'border-top-width',
    'border-width',
    'box-decoration-break',
    'box-shadow',
    'box-sizing',
    'break-after',
    'break-before',
    'break-inside',
    'caption-side',
    'caret-color',
    'clear',
    'color',
    'color-scheme',
    'column-count',
    'column-fill',
    'column-gap',
    'column-rule',
    'column-rule-color',
    'column-rule-style',
    'column-rule-width',
    'column-span',
    'column-width',
    'columns',
    'display',
    'empty-cells',
    'flex',
    'flex-basis',
    'flex-direction',
    'flex-flow',
    'flex-grow',
    'flex-shrink',
    'flex-wrap',
    'float',
    'font',
    'font-family',
    'font-feature-settings',
    'font-kerning',
    'font-optical-sizing',
    'font-size',
    'font-stretch',
    'font-style',
    'font-variant',
    'font-variant-caps',
    'font-variation-settings',
    'font-weight',
    'gap',
    'grid',
    'grid-area',
    'grid-auto-columns',
    'grid-auto-flow',
    'grid-auto-rows',
    'grid-column',
    'grid-column-end',
    'grid-column-gap',
    'grid-column-start',
    'grid-gap',
    'grid-row',
    'grid-row-end',
    'grid-row-gap',
    'grid-row-start',
    'grid-template',
    'grid-template-areas',
    'grid-template-columns',
    'grid-template-rows',
    'height',
    'hyphens',
    'inline-size',
    'justify-content',
    'justify-items',
    'justify-self',
    'letter-spacing',
    'line-break',
    'line-height',
    'list-style',
    'list-style-position',
    'list-style-type',
    'margin',
    'margin-block',
    'margin-block-end',
    'margin-block-start',
    'margin-bottom',
    'margin-inline',
    'margin-inline-end',
    'margin-inline-start',
    'margin-left',
    'margin-right',
    'margin-top',
    'max-block-size',
    'max-height',
    'max-inline-size',
    'max-width',
    'min-block-size',
    'min-height',
    'min-inline-size',
    'min-width',
    'mix-blend-mode',
    'object-fit',
    'object-position',
    'opacity',
    'order',
    'outline',
    'outline-color',
    'outline-offset',
    'outline-style',
    'outline-width',
    'overflow',
    'overflow-wrap',
    'overflow-x',
    'overflow-y',
    'overscroll-behavior',
    'padding',
    'padding-block',
    'padding-block-end',
    'padding-block-start',
    'padding-bottom',
    'padding-inline',
    'padding-inline-end',
    'padding-inline-start',
    'padding-left',
    'padding-right',
    'padding-top',
    'position',
    'rotate',
    'row-gap',
    'scale',
    'scroll-behavior',
    'tab-size',
    'table-layout',
    'text-align',
    'text-align-last',
    'text-decoration',
    'text-decoration-color',
    'text-decoration-line',
    'text-decoration-style',
    'text-decoration-thickness',
    'text-emphasis',
    'text-indent',
    'text-justify',
    'text-orientation',
    'text-overflow',
    'text-shadow',
    'text-transform',
    'text-underline-offset',
    'text-wrap',
    'transform',
    'transform-origin',
    'transition',
    'transition-delay',
    'transition-duration',
    'transition-property',
    'transition-timing-function',
    'translate',
    'unicode-bidi',
    'vertical-align',
    'visibility',
    'white-space',
    'width',
    'word-break',
    'word-spacing',
    'writing-mode',
    '-webkit-box-decoration-break',
    '-webkit-text-fill-color',
    '-webkit-text-stroke',
    '-webkit-text-stroke-color',
    '-webkit-text-stroke-width',
]);

const UNSAFE_CSS_VALUE =
    /(?:url|image-set|-webkit-image-set|cross-fade|element|paint|var|env|attr|expression)\s*\(|(?:javascript|data|file|lorepia-asset|https?):/i;
const VIEWPORT_UNIT = /(?:^|[^a-z0-9_-])[-+]?(?:\d*\.)?\d+(?:d?v[wh]|s?v[wh]|l?v[wh]|vmin|vmax)\b/i;
const SAFE_ACTION = /^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,511}$/;

export function isPortableAction(value: unknown): value is string {
    return typeof value === 'string' && SAFE_ACTION.test(value);
}

export function sanitizePortableCss(value: string, ownerDocument: Document = document): string {
    if (
        value === '' ||
        value.length > MAX_PORTABLE_CSS_CHARS ||
        value.includes('\0') ||
        value.includes('\\') ||
        /[<>]/.test(value)
    ) {
        return '';
    }
    try {
        const StyleSheet = ownerDocument.defaultView?.CSSStyleSheet ?? globalThis.CSSStyleSheet;
        if (typeof StyleSheet !== 'function') return '';
        const sheet = new StyleSheet();
        if (typeof sheet.replaceSync !== 'function') return '';
        sheet.replaceSync(value);
        const rules = sheet.cssRules;
        if (rules.length > MAX_PORTABLE_CSS_RULES) return '';
        return sanitizeRuleList(rules);
    } catch {
        return '';
    }
}

export function sanitizePortableInlineStyle(
    value: string,
    ownerDocument: Document = document,
): string {
    if (
        value === '' ||
        value.length > MAX_PORTABLE_CSS_VALUE_CHARS ||
        value.includes('\0') ||
        value.includes('\\') ||
        /[<>]/.test(value)
    ) {
        return '';
    }
    const element = ownerDocument.createElement('span');
    element.style.cssText = value;
    return sanitizeDeclaration(element.style);
}

export function sanitizePortableTree(root: HTMLElement, mediaUrls: ReadonlySet<string>): void {
    const elements = [root, ...root.querySelectorAll('*')];
    for (const element of elements) {
        if (element !== root && !root.contains(element)) continue;
        if (!ALLOWED_TAGS.has(element.tagName.toUpperCase())) {
            element.remove();
            continue;
        }
        if (element instanceof HTMLStyleElement) {
            element.textContent = sanitizePortableCss(element.textContent, root.ownerDocument);
        }
        for (const attribute of [...element.attributes]) {
            const name = attribute.name.toLowerCase();
            const value = attribute.value;
            if (name === 'card-btn') {
                const interactive =
                    element instanceof HTMLButtonElement || element instanceof HTMLInputElement;
                if (interactive && isPortableAction(value.trim())) {
                    element.setAttribute('data-portable-action', value.trim());
                }
                element.removeAttribute(attribute.name);
                continue;
            }
            if (!attributeAllowed(element, name) || value.length > MAX_PORTABLE_ATTRIBUTE_CHARS) {
                element.removeAttribute(attribute.name);
                continue;
            }
            if (name === 'style') {
                const sanitized = sanitizePortableInlineStyle(value, root.ownerDocument);
                if (sanitized === '') element.removeAttribute(attribute.name);
                else element.setAttribute(name, sanitized);
            }
        }
        if (element instanceof HTMLImageElement || element instanceof HTMLMediaElement) {
            const source = element.getAttribute('src');
            if (source === null || !mediaUrls.has(source)) element.removeAttribute('src');
        }
        if (element instanceof HTMLAnchorElement) {
            element.removeAttribute('href');
            element.removeAttribute('target');
        }
        if (element instanceof HTMLButtonElement) element.type = 'button';
        if (element instanceof HTMLInputElement) {
            const type = element.type.toLowerCase();
            if (!['button', 'checkbox', 'radio'].includes(type)) element.type = 'button';
        }
    }
}

function sanitizeRuleList(rules: CSSRuleList): string {
    const output: string[] = [];
    for (const rule of rules) {
        const sanitized = sanitizeRule(rule);
        if (sanitized !== '') output.push(sanitized);
    }
    return output.join('\n').slice(0, MAX_PORTABLE_CSS_CHARS);
}

function legacyCssRuleType(rule: CSSRule): unknown {
    // CSSRule.type remains the interoperable discriminator in WebKit and jsdom even though
    // the typed DOM API marks direct property access as deprecated.
    return Reflect.get(rule, 'type');
}

function sanitizeRule(rule: CSSRule): string {
    if (
        legacyCssRuleType(rule) === PORTABLE_STYLE_RULE &&
        'selectorText' in rule &&
        'style' in rule
    ) {
        const selector = String(rule.selectorText);
        if (
            selector.length === 0 ||
            selector.length > 4_096 ||
            /:host(?:-context)?\b/i.test(selector)
        ) {
            return '';
        }
        const declaration = sanitizeDeclaration(rule.style as CSSStyleDeclaration);
        return declaration === '' ? '' : `${selector}{${declaration}}`;
    }
    if (
        legacyCssRuleType(rule) === PORTABLE_MEDIA_RULE &&
        'conditionText' in rule &&
        'cssRules' in rule
    ) {
        const condition = String(rule.conditionText);
        if (
            condition.length === 0 ||
            condition.length > 1_024 ||
            !/^[a-z0-9\s():.,%<>=/_-]+$/i.test(condition)
        ) {
            return '';
        }
        const children = sanitizeRuleList(rule.cssRules as CSSRuleList);
        return children === '' ? '' : `@media ${condition}{${children}}`;
    }
    if (
        legacyCssRuleType(rule) === PORTABLE_KEYFRAMES_RULE &&
        'name' in rule &&
        'cssRules' in rule
    ) {
        const name = String(rule.name);
        if (!/^[-_A-Za-z][-_A-Za-z0-9]{0,63}$/.test(name)) return '';
        const frames: string[] = [];
        for (const frame of rule.cssRules as CSSRuleList) {
            if (
                legacyCssRuleType(frame) !== PORTABLE_KEYFRAME_RULE ||
                !('keyText' in frame) ||
                !('style' in frame)
            ) {
                continue;
            }
            const key = String(frame.keyText);
            if (
                !/^(?:from|to|\d{1,3}(?:\.\d+)?%)(?:\s*,\s*(?:from|to|\d{1,3}(?:\.\d+)?%))*$/i.test(
                    key,
                )
            ) {
                continue;
            }
            const declaration = sanitizeDeclaration(frame.style as CSSStyleDeclaration);
            if (declaration !== '') frames.push(`${key}{${declaration}}`);
        }
        return frames.length === 0 ? '' : `@keyframes ${name}{${frames.join('')}}`;
    }
    return '';
}

function sanitizeDeclaration(style: CSSStyleDeclaration): string {
    const declarations: string[] = [];
    for (let index = 0; index < style.length; index += 1) {
        const property = style.item(index).toLowerCase();
        const value = style.getPropertyValue(property).trim();
        if (
            !ALLOWED_CSS_PROPERTIES.has(property) ||
            value === '' ||
            value.length > MAX_PORTABLE_CSS_VALUE_CHARS ||
            value.includes('\\') ||
            UNSAFE_CSS_VALUE.test(value) ||
            VIEWPORT_UNIT.test(value)
        ) {
            continue;
        }
        if (property === 'position' && !/^(?:static|relative)$/.test(value)) continue;
        declarations.push(`${property}:${value};`);
    }
    return declarations.join('');
}

function attributeAllowed(element: Element, name: string): boolean {
    if (name === 'data-portable-action') return false;
    if (name.startsWith('aria-')) return true;
    if (name.startsWith('data-')) {
        return name !== 'data-portable-media' && name !== 'data-portable-autoplay';
    }
    if (ALLOWED_COMMON_ATTRIBUTES.has(name)) return true;
    if (element instanceof HTMLDetailsElement) return name === 'open';
    if (element instanceof HTMLLabelElement) return name === 'for';
    if (element instanceof HTMLButtonElement) {
        return name === 'disabled' || name === 'type' || name === 'value';
    }
    if (element instanceof HTMLInputElement) {
        return ['checked', 'disabled', 'type', 'value'].includes(name);
    }
    if (element instanceof HTMLImageElement) {
        return ['alt', 'decoding', 'height', 'loading', 'src', 'width'].includes(name);
    }
    if (element instanceof HTMLAudioElement) {
        return ['autoplay', 'controls', 'loop', 'muted', 'preload', 'src'].includes(name);
    }
    if (element instanceof HTMLTableCellElement) {
        return ['colspan', 'headers', 'rowspan', 'scope'].includes(name);
    }
    return false;
}
