import { describe, expect, it } from 'vitest';

import type { CharacterRenderProfileDto } from '../../lib/ipc/contracts';
import {
    defaultPortableRuntimeCapabilities,
    requiredPortableRuntimeCapabilities,
} from './portable-runtime';

describe('portable runtime declarative capabilities', () => {
    const toggleOnlyProfile: CharacterRenderProfileDto = {
        character_id: 'character',
        character_content_revision_id: 'revision',
        assets: [],
        background_markup: '',
        toggle_schema: 'music=Background music=toggle',
        initial_variables: { music: '0' },
        output_transforms: [],
        display_transforms: [],
        runtime_scripts: [],
        required_runtime_capabilities: [],
        runtime_capabilities_declared: false,
        runtime_knowledge: [],
        runtime_script_count: 0,
    };

    it('requires UI write for a toggle-only legacy profile', () => {
        expect(requiredPortableRuntimeCapabilities(toggleOnlyProfile)).toEqual(['ui:write']);
    });

    it('drops callback grants when imported scripts were quarantined', () => {
        const quarantinedCallbackProfile: CharacterRenderProfileDto = {
            ...toggleOnlyProfile,
            required_runtime_capabilities: ['runtime:callbacks', 'ui:write'],
            runtime_capabilities_declared: true,
        };

        expect(requiredPortableRuntimeCapabilities(quarantinedCallbackProfile)).toEqual([
            'ui:write',
        ]);
        expect(defaultPortableRuntimeCapabilities(quarantinedCallbackProfile)).toEqual([
            'ui:write',
        ]);
    });
});
