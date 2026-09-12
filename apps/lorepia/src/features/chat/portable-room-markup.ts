import type { PortableSurface } from './portable-renderer-policy';

/** Keep message prose in the transcript and viewport controls in the isolated room. */
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
    if (css.length <= 262_144 && typeof CSSStyleSheet.prototype.replaceSync === 'function') {
        try {
            const sheet = new CSSStyleSheet();
            sheet.replaceSync(css);
            visit(sheet.cssRules);
        } catch {
            // Unsupported CSS does not turn ordinary message prose into a room overlay.
        }
    }
    const overlays = new Set<Element>(
        [...root.querySelectorAll<HTMLElement>('[style]')].filter(
            (element) => element.style.position === 'fixed',
        ),
    );
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
    if (surface === 'room') {
        const inlineStyles = [...root.querySelectorAll('style')].map((style) =>
            style.cloneNode(true),
        );
        root.replaceChildren(...inlineStyles, ...top);
    } else {
        for (const element of top) element.remove();
    }
    if (surface === 'room') root.append(template.content);
}
