import { get } from 'svelte/store';
import { describe, expect, it } from 'vitest';

import { t } from '../../lib/i18n';
import { LorepiaClientError } from '../../lib/ipc/errors';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerFixture } from './app-controller-test-support';

describe('LorepiaAppController imports', () => {
    it('localizes an unsupported character-card source', async () => {
        const { mockClient } = createAppControllerFixture();
        const controller = new LorepiaAppController(
            mockClient({
                selectImportSource: () =>
                    Promise.reject(
                        new LorepiaClientError({
                            code: 'unsupported_content',
                            message_key: 'error.unsupported_content',
                            recoverable: false,
                            operation_id: null,
                            field_errors: [],
                        }),
                    ),
            }),
        );

        await controller.beginImport();

        expect(get(controller.state).import_flow).toEqual({
            phase: 'error',
            error: `${t('import.blocked')}: ${t('error.invalid_input')}`,
            inspection: null,
        });
    });

    it('commits a Risu preset as content without inserting a fake character', async () => {
        const { mockClient } = createAppControllerFixture();
        const inspection = {
            inspection_id: 'inspection-risu',
            kind: 'risu_preset' as const,
            display_name: 'Wave V1.6',
            description: 'Risu prompt preset',
            source_sha256: 'a'.repeat(64),
            source_size: 71_533,
            estimated_stored_size: 72_000,
            asset_count: 0,
            dynamic_content: {
                runtime_script_count: 0,
                elevated_runtime_script_count: 0,
                required_runtime_capabilities: [],
                runtime_capabilities_declared: false,
                regex_rule_count: 0,
                enabled_regex_rule_count: 0,
                model_calls_possible: false,
                custom_markup_present: false,
                regex_rules: [],
            },
            representative_image: null,
            warnings: [],
            blocked_reasons: [],
            unsupported_optional_fields: [],
            allowed: true,
        };
        const controller = new LorepiaAppController(
            mockClient({
                selectImportSource: () =>
                    Promise.resolve({
                        ticket_id: 'ticket-risu',
                        display_name: 'wave.risup',
                        size_bytes: inspection.source_size,
                    }),
                inspectImport: () => Promise.resolve(inspection),
                commitImport: () =>
                    Promise.resolve({
                        kind: 'content',
                        content: {
                            kind: 'risu_preset',
                            import_id: 'package-import-risu',
                            display_name: inspection.display_name,
                            document_count: 2,
                            asset_count: 0,
                        },
                    }),
            }),
        );

        await controller.beginImport();
        await controller.commitImport();

        const state = get(controller.state);
        expect(state.import_flow).toEqual({
            phase: 'idle',
            error: null,
            inspection: null,
        });
        expect(state.library.characters).toEqual([]);
        expect(state.announcement).toBe(
            t('import.notice.content_added', {
                name: inspection.display_name,
                documents: 2,
                assets: 0,
            }),
        );
    });
});
