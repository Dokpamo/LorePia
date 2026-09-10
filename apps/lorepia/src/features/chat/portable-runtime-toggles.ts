import { MAX_RUNTIME_RECORD_KEYS } from './portable-runtime-state';

export interface PortableRuntimeToggle {
    key: string;
    label: string;
    kind: 'select' | 'toggle' | 'text';
    choices: string[];
}

export function parsePortableRuntimeToggles(schema: string): PortableRuntimeToggle[] {
    const toggles: PortableRuntimeToggle[] = [];
    for (const sourceLine of schema.split(/\r?\n/)) {
        const line = sourceLine.trim();
        if (line === '' || line.startsWith('=')) continue;
        const [key = '', label = '', rawKind = '', rawChoices = ''] = line.split('=');
        const kind = rawKind.trim().toLowerCase();
        if (
            key.trim() === '' ||
            label.trim() === '' ||
            !['', 'select', 'toggle', 'checkbox', 'text', 'textarea'].includes(kind)
        ) {
            continue;
        }
        toggles.push({
            key: key.trim(),
            label: label.trim(),
            kind:
                kind === 'select'
                    ? 'select'
                    : kind === '' || kind === 'toggle' || kind === 'checkbox'
                      ? 'toggle'
                      : 'text',
            choices:
                kind === 'select'
                    ? rawChoices
                          .split(',')
                          .map((choice) => choice.trim())
                          .filter(Boolean)
                          .slice(0, 128)
                    : [],
        });
        if (toggles.length >= MAX_RUNTIME_RECORD_KEYS) break;
    }
    return toggles;
}
