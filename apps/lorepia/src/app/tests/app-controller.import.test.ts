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
            resource_override_active: false,
            resource_override_available: false,
        });
    });

    it('explains temporary free-space needs for a large import', async () => {
        const { mockClient } = createAppControllerFixture();
        const controller = new LorepiaAppController(
            mockClient({
                selectImportSource: () =>
                    Promise.reject(
                        new LorepiaClientError({
                            code: 'storage_unavailable',
                            message_key: 'error.storage_unavailable',
                            recoverable: true,
                            operation_id: null,
                            field_errors: [],
                        }),
                    ),
            }),
        );

        await controller.beginImport();

        expect(get(controller.state).import_flow.error).toBe(t('import.error.storage_unavailable'));
    });

    it('never offers a resource override for an unsafe archive structure', async () => {
        const { mockClient } = createAppControllerFixture();
        const controller = new LorepiaAppController(
            mockClient({
                selectImportSource: () =>
                    Promise.reject(
                        new LorepiaClientError({
                            code: 'unsafe_archive',
                            message_key: 'error.unsafe_archive',
                            recoverable: false,
                            operation_id: null,
                            field_errors: [],
                        }),
                    ),
            }),
        );

        await controller.beginImport();

        expect(get(controller.state).import_flow.resource_override_available).toBe(false);
    });

    it('offers a resource override after a byte-envelope inspection failure', async () => {
        const { mockClient } = createAppControllerFixture();
        const controller = new LorepiaAppController(
            mockClient({
                selectImportSource: () =>
                    Promise.reject(
                        new LorepiaClientError({
                            code: 'resource_limit_exceeded',
                            message_key: 'error.resource_limit_exceeded',
                            recoverable: true,
                            operation_id: null,
                            field_errors: [],
                        }),
                    ),
            }),
        );

        await controller.beginImport();

        expect(get(controller.state).import_flow).toMatchObject({
            error: t('import.error.resource_limit'),
            resource_override_available: true,
        });
    });

    it('commits a external preset as content without inserting a fake character', async () => {
        const { mockClient } = createAppControllerFixture();
        const inspection = {
            inspection_id: 'inspection-imported',
            kind: 'imported_preset' as const,
            display_name: 'Wave V1.6',
            description: 'external prompt preset',
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
                        ticket_id: 'ticket-imported',
                        display_name: 'wave.risup',
                        size_bytes: inspection.source_size,
                    }),
                inspectImport: () => Promise.resolve(inspection),
                commitImport: () =>
                    Promise.resolve({
                        kind: 'content',
                        content: {
                            kind: 'imported_preset',
                            import_id: 'package-imported',
                            display_name: inspection.display_name,
                            document_count: 2,
                            asset_count: 0,
                        },
                    }),
            }),
        );

        await controller.beginImport();
        const result = await controller.commitImport();

        const state = get(controller.state);
        expect(state.import_flow).toEqual({
            phase: 'idle',
            error: null,
            inspection: null,
            resource_override_active: false,
            resource_override_available: false,
        });
        expect(state.library.characters).toEqual([]);
        expect(result).toEqual({
            kind: 'content',
            content: {
                kind: 'imported_preset',
                import_id: 'package-imported',
                display_name: inspection.display_name,
                document_count: 2,
                asset_count: 0,
            },
        });
        expect(state.announcement).toBe(
            t('import.notice.content_added', {
                name: inspection.display_name,
                documents: 2,
                assets: 0,
            }),
        );
    });

    it('requires a separate foreground retry before using the large resource envelope', async () => {
        const { mockClient } = createAppControllerFixture();
        const selectedPolicies: string[] = [];
        const inspection = {
            inspection_id: 'inspection-large',
            kind: 'character_card_v3' as const,
            display_name: 'Large card',
            description: 'Ten GiB fixture',
            source_sha256: 'b'.repeat(64),
            source_size: 10 * 1024 * 1024 * 1024,
            estimated_stored_size: 10 * 1024 * 1024 * 1024,
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
                selectImportSource: (policy = 'standard') => {
                    selectedPolicies.push(policy);
                    if (policy === 'standard') {
                        return Promise.reject(
                            new LorepiaClientError({
                                code: 'selected_file_too_large',
                                message_key: 'error.selected_file_too_large',
                                recoverable: true,
                                operation_id: null,
                                field_errors: [],
                            }),
                        );
                    }
                    return Promise.resolve({
                        ticket_id: 'ticket-large',
                        display_name: 'large.charx',
                        size_bytes: inspection.source_size,
                    });
                },
                inspectImport: () => Promise.resolve(inspection),
            }),
        );

        await controller.beginImport();
        expect(get(controller.state).import_flow.resource_override_available).toBe(true);

        await controller.beginImport();
        expect(selectedPolicies).toEqual(['standard', 'user_approved_large']);
        expect(get(controller.state).import_flow).toMatchObject({
            phase: 'ready',
            resource_override_active: true,
            resource_override_available: false,
            inspection,
        });
    });
});
