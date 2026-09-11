import type { PortableSurface } from './portable-renderer-policy';

/** Move viewport overlays to the host's isolated card screen, never the chat chrome. */
export function selectPortableRoomMarkup(
    root: HTMLElement,
    background: string,
    surface: PortableSurface,
): void {
    const template = document.createElement('template');
    template.innerHTML = background;
    const styles = [
        ...template.content.querySelectorAll('style'),
        ...root.querySelectorAll('style'),
    ];
    const css = styles.map((style) => style.textContent).join('\n');
    if (css.length > 262_144 || typeof CSSStyleSheet.prototype.replaceSync !== 'function') return;
    const sheet = new CSSStyleSheet();
    try {
        sheet.replaceSync(css);
    } catch {
        return;
    }
    const selectors: string[] = [];
    let count = 0;
    const visit = (rules: CSSRuleList): void => {
        for (const rule of rules) {
            if (++count > 512) return;
            if ('style' in rule && 'selectorText' in rule) {
                const style = rule.style as CSSStyleDeclaration;
                if (style.getPropertyValue('position').trim() === 'fixed')
                    selectors.push(String(rule.selectorText));
            } else if ('cssRules' in rule) visit(rule.cssRules as CSSRuleList);
        }
    };
    visit(sheet.cssRules);
    const overlays = new Set<Element>();
    for (const selector of selectors) {
        try {
            for (const element of root.querySelectorAll(selector)) overlays.add(element);
        } catch {
            /* Unsupported imported selectors do not escape the contained surface. */
        }
    }
    const top = [...overlays].filter(
        (element) => ![...overlays].some((other) => other !== element && other.contains(element)),
    );
    if (surface === 'room' && top.length > 0) {
        const inlineStyles = [...root.querySelectorAll('style')].map((style) =>
            style.cloneNode(true),
        );
        root.replaceChildren(...inlineStyles, ...top);
    } else if (surface === 'message') {
        for (const element of top) element.remove();
    }
}
