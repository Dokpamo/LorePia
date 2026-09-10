import { parseCss, type AST } from 'svelte/compiler';

export interface StyleRule {
    selector: string;
    declarations: Record<string, string>;
    media: string[];
}

/** Parse the supported CSS syntax instead of relying on declaration order/formatting. */
export function styleRules(source: string): StyleRule[] {
    const result: StyleRule[] = [];
    function walk(nodes: AST.CSS.Block['children'], media: string[]): void {
        for (const node of nodes) {
            if (node.type === 'Atrule' && node.block) {
                walk(node.block.children, node.name === 'media' ? [...media, node.prelude] : media);
            } else if (node.type === 'Rule') {
                result.push({
                    selector: source.slice(node.prelude.start, node.prelude.end),
                    declarations: Object.fromEntries(
                        node.block.children
                            .filter((child) => child.type === 'Declaration')
                            .map((child) => [child.property, child.value]),
                    ),
                    media,
                });
                walk(node.block.children, media);
            }
        }
    }
    walk(parseCss(source).children, []);
    return result;
}
