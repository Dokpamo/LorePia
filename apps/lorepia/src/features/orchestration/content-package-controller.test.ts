import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    ApproveContentPackageImportInput,
    ApproveContentPackageImportReceiptDto,
    ContentPackageClientApi,
    ContentPackageImportReviewDto,
    ContentPackageInspectionReviewDto,
    ContentPackageSelectionReviewDto,
    ContentPackageTargetReviewDocumentDto,
    ContentPackageTargetReviewDto,
    ContentSourceExportReceiptDto,
    LorepiaClient,
    SelectContentPackageImportInput,
} from '../../lib/ipc/contracts';
import { LiveLorepiaClient, type LorepiaTransport } from '../../lib/ipc/client';
import {
    ContentPackageController,
    type ContentPackageCapableClient,
} from './content-package-controller';

const REVIEW_SHA = 'a'.repeat(64);
const PACKAGE_PLAN_SHA = 'b'.repeat(64);
const SELECTION_PLAN_SHA = 'c'.repeat(64);
const IMPORT_PLAN_SHA = 'd'.repeat(64);
const EVIDENCE_SHA = 'e'.repeat(64);
const CAPABILITY_REVIEW_SHA = 'f'.repeat(64);
const APPROVAL_SHA = '1'.repeat(64);
const TARGET_REVIEW_SHA = '2'.repeat(64);
const CREATE_DOCUMENT_SHA = '3'.repeat(64);
const UPDATE_DOCUMENT_SHA = '4'.repeat(64);
const COMPONENT_A_SHA = '5'.repeat(64);
const COMPONENT_Z_SHA = '6'.repeat(64);
const EXPORT_SHA = '7'.repeat(64);
const APPROVAL_ID = '00000000-0000-4000-8000-000000000001';

function inspection(): ContentPackageInspectionReviewDto {
    return {
        import_id: 'import-1',
        revision: 7,
        manifest: {
            package_id: 'package.synthetic',
            name: '합성 테스트 패키지',
            version: '1.0.0',
            author: 'LorePia',
            license: 'project-owned synthetic',
            redistribution_allowed: false,
            required_app_version: null,
            required_capabilities: ['prompt_fragments', 'transforms'],
        },
        source_size_bytes: 1024,
        total_uncompressed_size_bytes: 2048,
        components: [
            {
                id: 'component-z',
                kind: 'transform_set',
                disposition: 'importable',
                dependency_ids: [],
                conflict_ids: [],
                required_capabilities: ['transforms'],
                asset_count: 0,
            },
            {
                id: 'component-a',
                kind: 'prompt_preset',
                disposition: 'importable',
                dependency_ids: [],
                conflict_ids: [],
                required_capabilities: ['prompt_fragments'],
                asset_count: 0,
            },
            {
                id: 'component-quarantined',
                kind: 'raw_extension',
                disposition: 'quarantined',
                dependency_ids: [],
                conflict_ids: [],
                required_capabilities: ['script'],
                asset_count: 0,
            },
        ],
        asset_count: 0,
        issues: [],
        local_import_allowed: true,
        redistribution_status: 'denied_by_manifest',
        package_plan_hash: PACKAGE_PLAN_SHA,
        review_sha256: REVIEW_SHA,
        capability_review_sha256: CAPABILITY_REVIEW_SHA,
        capability_decisions: [
            {
                capability: 'transforms',
                support: 'approval_required',
                approved: false,
                reason: '명시적 승인 필요',
            },
        ],
    };
}

function targetReview(
    selectedComponentIds: readonly string[] = ['component-a', 'component-z'],
): ContentPackageTargetReviewDto {
    const selected = new Set(selectedComponentIds);
    const documents: ContentPackageTargetReviewDocumentDto[] = [
        {
            source_component_id: 'component-a',
            component_document_ordinal: 0,
            document_index: 0,
            document_kind: 'prompt_preset',
            target_object_id: 'prompt-1',
            disposition: 'create',
            expected_target_revision_id: null,
            expected_target_state_revision: null,
            source_component_sha256: COMPONENT_A_SHA,
            document_sha256: CREATE_DOCUMENT_SHA,
        },
        {
            source_component_id: 'component-z',
            component_document_ordinal: 0,
            document_index: 1,
            document_kind: 'transform_set',
            target_object_id: 'transform-set-1',
            disposition: 'update',
            expected_target_revision_id: 'transform-revision-3',
            expected_target_state_revision: 12,
            source_component_sha256: COMPONENT_Z_SHA,
            document_sha256: UPDATE_DOCUMENT_SHA,
        },
    ];
    return {
        target_review_sha256: TARGET_REVIEW_SHA,
        documents: documents
            .filter((document) => selected.has(document.source_component_id))
            .map((document, documentIndex) => ({
                ...document,
                document_index: documentIndex,
            })),
    };
}

function requiredTargetDocument(
    review: ContentPackageTargetReviewDto,
    targetObjectId: string,
): ContentPackageTargetReviewDocumentDto {
    const document = review.documents.find(
        (candidate) => candidate.target_object_id === targetObjectId,
    );
    if (document === undefined) throw new Error('synthetic target-review document is missing');
    return document;
}

function selection(
    selectedComponentIds: readonly string[] = ['component-a', 'component-z'],
): ContentPackageSelectionReviewDto {
    return {
        content_selection_plan_hash: SELECTION_PLAN_SHA,
        import_plan_sha256: IMPORT_PLAN_SHA,
        normalization_evidence_sha256: EVIDENCE_SHA,
        normalization_evidence: [
            {
                component_id: 'component-z',
                object_id: 'transform-set-1',
                field: 'enabled',
                before: true,
                after: false,
                reason: '가져오기 전 안전 비활성화',
            },
        ],
        target_review: targetReview(selectedComponentIds),
    };
}

function pendingReview(): ContentPackageImportReviewDto {
    return {
        import_id: 'import-1',
        package_id: 'package.synthetic',
        status: 'awaiting_review',
        revision: 8,
        package_plan_hash: PACKAGE_PLAN_SHA,
        review_sha256: REVIEW_SHA,
        capability_review_sha256: CAPABILITY_REVIEW_SHA,
        selected_component_ids: ['component-a', 'component-z'],
        selection: selection(),
        approval: null,
    };
}

function approvalReceipt(
    approvalId = APPROVAL_ID,
    enabledComponentIds = ['component-z'],
): ApproveContentPackageImportReceiptDto {
    return {
        import_id: 'import-1',
        status: 'approved',
        revision: 9,
        package_plan_hash: PACKAGE_PLAN_SHA,
        content_selection_plan_hash: SELECTION_PLAN_SHA,
        review_sha256: REVIEW_SHA,
        import_plan_sha256: IMPORT_PLAN_SHA,
        capability_review_sha256: CAPABILITY_REVIEW_SHA,
        normalization_evidence_sha256: EVIDENCE_SHA,
        normalization_evidence: selection().normalization_evidence,
        target_review: targetReview(),
        approval_sha256: APPROVAL_SHA,
        approval_id: approvalId,
        enabled_component_ids: enabledComponentIds,
        approved_capabilities: ['transforms'],
    };
}

function capableClient(
    overrides: Partial<ContentPackageClientApi> = {},
): ContentPackageCapableClient {
    return {
        listPendingContentPackageImports: vi.fn().mockResolvedValue([]),
        listCompletedContentPackageExports: vi.fn().mockResolvedValue([]),
        pickContentPackageImport: vi.fn().mockResolvedValue(inspection()),
        reopenContentPackageImport: vi.fn().mockResolvedValue({
            inspection: { ...inspection(), revision: 8 },
            lifecycle: pendingReview(),
        }),
        selectContentPackageImport: vi.fn((input: SelectContentPackageImportInput) =>
            Promise.resolve({
                import_id: 'import-1',
                status: 'awaiting_review' as const,
                revision: 8,
                package_plan_hash: PACKAGE_PLAN_SHA,
                review_sha256: REVIEW_SHA,
                capability_review_sha256: CAPABILITY_REVIEW_SHA,
                selected_component_ids: input.selected_component_ids,
                selection: selection(input.selected_component_ids),
                required_capabilities: [
                    ...(input.selected_component_ids.includes('component-a')
                        ? (['prompt_fragments'] as const)
                        : []),
                    ...(input.selected_component_ids.includes('component-z')
                        ? (['transforms'] as const)
                        : []),
                ],
            }),
        ),
        approveContentPackageImport: vi.fn((input: ApproveContentPackageImportInput) =>
            Promise.resolve(approvalReceipt(input.approval_id, input.enable_component_ids)),
        ),
        commitContentPackageImport: vi.fn().mockResolvedValue({
            import_id: 'import-1',
            package_id: 'package.synthetic',
            status: 'completed',
            revision: 10,
            committed_document_ids: ['prompt-1', 'transform-1'],
            asset_ids: [],
        }),
        discardContentPackageImport: vi.fn().mockResolvedValue({
            import_id: 'import-1',
            package_id: 'package.synthetic',
            status: 'discarded',
            revision: 9,
            selected_component_ids: ['component-a', 'component-z'],
            failure_code: null,
            created_at: '2026-08-03T00:00:00Z',
            updated_at: '2026-08-03T00:01:00Z',
        }),
        exportContentSource: vi.fn().mockResolvedValue({
            kind: 'lorepia_package',
            source_id: 'import-1',
            sha256: EXPORT_SHA,
            size_bytes: 4096,
            file_name: 'package.synthetic.lorepia.zip',
        }),
        ...overrides,
    } as unknown as ContentPackageCapableClient;
}

function deferred<Value>(): {
    promise: Promise<Value>;
    resolve: (value: Value) => void;
} {
    let resolvePromise!: (value: Value) => void;
    const promise = new Promise<Value>((resolve) => {
        resolvePromise = resolve;
    });
    return { promise, resolve: resolvePromise };
}

async function completeSyntheticPackage(controller: ContentPackageController): Promise<void> {
    await controller.pickAndInspect();
    controller.toggleComponent('component-z');
    controller.toggleComponent('component-a');
    await controller.reviewSelection();
    controller.toggleApprovedCapability('transforms');
    controller.toggleUpdateTargetConfirmation('component-z', 0);
    await controller.approve();
    await controller.commit();
}

afterEach(() => {
    vi.restoreAllMocks();
});

describe('ContentPackageController', () => {
    it('reports the unavailable Core boundary without manufacturing a review or result', async () => {
        const controller = new ContentPackageController({} as LorepiaClient);

        await expect(controller.pickAndInspect()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'unavailable',
            inspection: null,
            selection: null,
            approval: null,
            result: null,
            selected_component_ids: [],
            error: '현재 Core가 안전한 콘텐츠 패키지 선택 및 검토 API를 제공하지 않습니다.',
        });
    });

    it('shows normalization evidence before exact explicit approval and commit hash echoes', async () => {
        vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(APPROVAL_ID);
        const client = capableClient();
        const controller = new ContentPackageController(client);

        await expect(controller.pickAndInspect()).resolves.toBe(true);
        expect(controller.toggleComponent('component-z')).toBe(true);
        expect(controller.toggleComponent('component-a')).toBe(true);

        await expect(controller.reviewSelection()).resolves.toBe(true);
        expect(client.selectContentPackageImport).toHaveBeenCalledWith({
            import_id: 'import-1',
            expected_revision: 7,
            expected_package_plan_hash: PACKAGE_PLAN_SHA,
            expected_review_sha256: REVIEW_SHA,
            expected_capability_review_sha256: CAPABILITY_REVIEW_SHA,
            selected_component_ids: ['component-a', 'component-z'],
        });
        expect(get(controller.state)).toMatchObject({
            phase: 'selection_ready',
            selection: {
                normalization_evidence_sha256: EVIDENCE_SHA,
                normalization_evidence: [
                    {
                        component_id: 'component-z',
                        before: true,
                        after: false,
                    },
                ],
                target_review: {
                    target_review_sha256: TARGET_REVIEW_SHA,
                    documents: [
                        { target_object_id: 'prompt-1', disposition: 'create' },
                        {
                            target_object_id: 'transform-set-1',
                            disposition: 'update',
                            expected_target_revision_id: 'transform-revision-3',
                            expected_target_state_revision: 12,
                        },
                    ],
                },
            },
            approval: null,
        });

        expect(controller.toggleEnabledComponent('component-z')).toBe(true);
        expect(controller.toggleApprovedCapability('transforms')).toBe(true);
        await expect(controller.approve()).resolves.toBe(false);
        expect(client.approveContentPackageImport).not.toHaveBeenCalled();
        expect(controller.toggleUpdateTargetConfirmation('component-z', 0)).toBe(true);
        await expect(controller.approve()).resolves.toBe(true);
        expect(client.approveContentPackageImport).toHaveBeenCalledWith({
            import_id: 'import-1',
            expected_revision: 8,
            expected_package_plan_hash: PACKAGE_PLAN_SHA,
            expected_content_selection_plan_hash: SELECTION_PLAN_SHA,
            expected_review_sha256: REVIEW_SHA,
            expected_import_plan_sha256: IMPORT_PLAN_SHA,
            expected_capability_review_sha256: CAPABILITY_REVIEW_SHA,
            expected_normalization_evidence_sha256: EVIDENCE_SHA,
            expected_target_review_sha256: TARGET_REVIEW_SHA,
            approval_id: APPROVAL_ID,
            enable_component_ids: ['component-z'],
            approved_capabilities: ['transforms'],
            confirmed_update_targets: [
                {
                    source_component_id: 'component-z',
                    component_document_ordinal: 0,
                    target_object_id: 'transform-set-1',
                    expected_target_revision_id: 'transform-revision-3',
                    expected_target_state_revision: 12,
                },
            ],
        });

        await expect(controller.commit()).resolves.toBe(true);
        expect(client.commitContentPackageImport).toHaveBeenCalledWith({
            import_id: 'import-1',
            expected_revision: 9,
            expected_package_plan_hash: PACKAGE_PLAN_SHA,
            expected_content_selection_plan_hash: SELECTION_PLAN_SHA,
            expected_review_sha256: REVIEW_SHA,
            expected_import_plan_sha256: IMPORT_PLAN_SHA,
            expected_approval_sha256: APPROVAL_SHA,
            expected_capability_review_sha256: CAPABILITY_REVIEW_SHA,
            expected_normalization_evidence_sha256: EVIDENCE_SHA,
        });
        expect(get(controller.state)).toMatchObject({
            phase: 'idle',
            inspection: null,
            selection: null,
            approval: null,
            result: {
                import_id: 'import-1',
                status: 'completed',
                committed_document_ids: ['prompt-1', 'transform-1'],
            },
        });

        await expect(controller.exportCompletedPackage()).resolves.toBe(true);
        expect(client.exportContentSource).toHaveBeenCalledWith({
            kind: 'content_package',
            import_id: 'import-1',
        });
        expect(get(controller.state)).toMatchObject({
            exporting_import_id: null,
            export_receipt: {
                kind: 'lorepia_package',
                source_id: 'import-1',
                sha256: EXPORT_SHA,
                size_bytes: 4096,
                file_name: 'package.synthetic.lorepia.zip',
            },
            export_error: null,
            announcement: 'package.synthetic.lorepia.zip 파일로 콘텐츠 패키지를 내보냈습니다.',
        });

        const exportContentSource = client.exportContentSource;
        if (exportContentSource === undefined) throw new Error('synthetic export API is missing');
        vi.mocked(exportContentSource).mockResolvedValueOnce(null);
        await expect(controller.exportCompletedPackage()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            exporting_import_id: null,
            export_receipt: {
                source_id: 'import-1',
                sha256: EXPORT_SHA,
            },
            export_error: null,
            announcement: 'package.synthetic.lorepia.zip 파일로 콘텐츠 패키지를 내보냈습니다.',
        });
    });

    it('blocks duplicate package exports and treats native picker cancellation as neutral', async () => {
        const pendingExport = deferred<ContentSourceExportReceiptDto | null>();
        const client = capableClient({
            exportContentSource: vi.fn(() => pendingExport.promise),
        });
        const controller = new ContentPackageController(client);
        await completeSyntheticPackage(controller);

        const firstExport = controller.exportCompletedPackage();
        await expect(controller.exportCompletedPackage()).resolves.toBe(false);
        expect(client.exportContentSource).toHaveBeenCalledTimes(1);
        expect(get(controller.state)).toMatchObject({
            exporting_import_id: 'import-1',
            export_receipt: null,
            export_error: null,
        });

        pendingExport.resolve(null);
        await expect(firstExport).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            exporting_import_id: null,
            export_receipt: null,
            export_error: null,
            announcement: '콘텐츠 패키지를 안전하게 가져왔습니다.',
        });
    });

    it('fails closed when package export evidence names a different durable source', async () => {
        const client = capableClient({
            exportContentSource: vi.fn().mockResolvedValue({
                kind: 'lorepia_package',
                source_id: 'import-other',
                sha256: EXPORT_SHA,
                size_bytes: 4096,
                file_name: 'package.synthetic.lorepia.zip',
                path: '/private/should-not-cross',
            }),
        });
        const controller = new ContentPackageController(client);
        await completeSyntheticPackage(controller);

        await expect(controller.exportCompletedPackage()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            exporting_import_id: null,
            export_receipt: null,
            export_error: 'Core 내보내기 영수증이 완료된 패키지와 일치하지 않습니다.',
        });
        expect(JSON.stringify(get(controller.state))).not.toContain('/private/');
    });

    it('rejects zero-byte delivery evidence and non-portable completed-export suggestions', async () => {
        const client = capableClient({
            exportContentSource: vi.fn().mockResolvedValue({
                kind: 'lorepia_package',
                source_id: 'import-1',
                sha256: EXPORT_SHA,
                size_bytes: 0,
                file_name: 'package.synthetic.lorepia.zip',
            }),
            listCompletedContentPackageExports: vi.fn().mockResolvedValue([
                {
                    kind: 'lorepia_package',
                    source_id: 'import-1',
                    sha256: EXPORT_SHA,
                    size_bytes: 4096,
                    suggested_file_name: 'package name.zip',
                },
            ]),
        });
        const controller = new ContentPackageController(client);
        await completeSyntheticPackage(controller);

        await expect(controller.exportCompletedPackage()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            export_receipt: null,
            export_error: 'Core 내보내기 영수증이 완료된 패키지와 일치하지 않습니다.',
        });
        await expect(controller.loadCompletedPackageExports()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            completed_package_exports: [],
            completed_exports_error: 'Core 완료 패키지 내보내기 목록이 안전한 스냅샷이 아닙니다.',
        });
    });

    it('does not select quarantined or missing components', async () => {
        const controller = new ContentPackageController(capableClient());
        await controller.pickAndInspect();

        expect(controller.toggleComponent('component-quarantined')).toBe(false);
        expect(controller.toggleComponent('component-missing')).toBe(false);
        expect(get(controller.state).selected_component_ids).toEqual([]);
    });

    it('keeps a hostile package quarantined across the live command adapter without dispatching runtime work', async () => {
        const hostileInspection: ContentPackageInspectionReviewDto = {
            import_id: 'import-hostile',
            revision: 1,
            manifest: {
                package_id: 'package.hostile.synthetic',
                name: '격리 합성 패키지',
                version: '1.0.0',
                author: 'LorePia tests',
                license: 'project-owned synthetic',
                redistribution_allowed: false,
                required_app_version: null,
                required_capabilities: ['html', 'script', 'high_risk_assets'],
            },
            source_size_bytes: 2_048,
            total_uncompressed_size_bytes: 4_096,
            components: [
                {
                    id: 'component-hostile-code',
                    kind: 'content_module',
                    disposition: 'quarantined',
                    dependency_ids: [],
                    conflict_ids: [],
                    required_capabilities: ['html', 'script'],
                    asset_count: 2,
                },
            ],
            asset_count: 2,
            issues: [
                {
                    severity: 'blocker',
                    code: 'executable-content-quarantined',
                    message: '스크립트와 HTML을 포함한 구성 요소를 실행 불가 상태로 격리했습니다.',
                },
            ],
            local_import_allowed: false,
            redistribution_status: 'validation_blocked',
            package_plan_hash: PACKAGE_PLAN_SHA,
            review_sha256: REVIEW_SHA,
            capability_review_sha256: CAPABILITY_REVIEW_SHA,
            capability_decisions: [
                {
                    capability: 'script',
                    support: 'unsupported',
                    approved: false,
                    reason: '가져온 코드는 실행할 수 없습니다.',
                },
                {
                    capability: 'html',
                    support: 'unsupported',
                    approved: false,
                    reason: '가져온 HTML은 렌더링하거나 실행할 수 없습니다.',
                },
                {
                    capability: 'high_risk_assets',
                    support: 'unsupported',
                    approved: false,
                    reason: '격리된 자산은 전달할 수 없습니다.',
                },
            ],
        };
        const calls: {
            commandName: string;
            args: Record<string, unknown> | undefined;
        }[] = [];
        let inspectionReturned = false;
        const transport: LorepiaTransport = {
            invoke(commandName, args) {
                calls.push({ commandName, args });
                if (commandName === 'pick_content_package_import' && !inspectionReturned) {
                    inspectionReturned = true;
                    return Promise.resolve(structuredClone(hostileInspection));
                }
                if (commandName === 'discard_content_package_import' && inspectionReturned) {
                    return Promise.reject(
                        Object.assign(new Error('quarantined review retained'), {
                            code: 'unsafe_archive',
                            message_key: 'error.content_package_quarantined_review_retained',
                            recoverable: false,
                            operation_id: 'operation-discard-hostile',
                            field_errors: [],
                        }),
                    );
                }
                return Promise.reject(new Error(`unexpected command: ${commandName}`));
            },
            createChatChannel: () => ({}),
            listen: () => Promise.resolve(() => undefined),
        };
        const controller = new ContentPackageController(new LiveLorepiaClient(transport));

        await expect(controller.pickAndInspect()).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            status: 'inspected',
            inspection: {
                import_id: 'import-hostile',
                local_import_allowed: false,
                components: [
                    {
                        id: 'component-hostile-code',
                        kind: 'content_module',
                        disposition: 'quarantined',
                    },
                ],
                issues: [
                    {
                        severity: 'blocker',
                        code: 'executable-content-quarantined',
                        message:
                            '스크립트와 HTML을 포함한 구성 요소를 실행 불가 상태로 격리했습니다.',
                    },
                ],
            },
            selected_component_ids: [],
            enabled_component_ids: [],
            required_capabilities: [],
            selection: null,
            approval: null,
            result: null,
        });

        expect(controller.toggleComponent('component-hostile-code')).toBe(false);
        await expect(controller.reviewSelection()).resolves.toBe(false);
        await expect(controller.approve()).resolves.toBe(false);
        await expect(controller.commit()).resolves.toBe(false);
        await expect(controller.discard()).resolves.toBe(false);

        expect(calls).toEqual([
            { commandName: 'pick_content_package_import', args: undefined },
            {
                commandName: 'discard_content_package_import',
                args: {
                    request: {
                        import_id: 'import-hostile',
                        expected_revision: 1,
                        expected_review_sha256: REVIEW_SHA,
                        expected_import_plan_sha256: null,
                        expected_capability_review_sha256: CAPABILITY_REVIEW_SHA,
                    },
                },
            },
        ]);
        const forbiddenCommands = [
            'select_content_package_import',
            'approve_content_package_import',
            'commit_content_package_import',
            'list_content_module_lifecycle_candidates',
            'review_content_module_activation',
            'resolve_content_module_activation',
            'activate_content_module',
            'resolve_asset_delivery',
        ];
        expect(
            calls
                .map(({ commandName }) => commandName)
                .filter((commandName) => forbiddenCommands.includes(commandName)),
        ).toEqual([]);

        const failedState = get(controller.state);
        expect(failedState).toMatchObject({
            phase: 'error',
            status: 'inspected',
            inspection: { import_id: 'import-hostile' },
            selected_component_ids: [],
            enabled_component_ids: [],
            selection: null,
            approval: null,
            result: null,
            error: 'error.content_package_quarantined_review_retained',
            announcement: 'error.content_package_quarantined_review_retained',
        });
        const visibleState = JSON.stringify(failedState);
        expect(visibleState).not.toContain('lorepia-asset://');
        expect(visibleState).not.toContain('candidate_id');
        expect(visibleState).not.toContain('module_id');
        expect(visibleState).not.toContain('asset_id');
        expect(visibleState).not.toContain('콘텐츠 패키지를 안전하게 가져왔습니다.');
    });

    it('requires every approval-required capability before approval', async () => {
        const client = capableClient();
        const controller = new ContentPackageController(client);
        await controller.pickAndInspect();
        controller.toggleComponent('component-z');
        await controller.reviewSelection();

        await expect(controller.approve()).resolves.toBe(false);
        expect(client.approveContentPackageImport).not.toHaveBeenCalled();
    });

    it('rejects selection receipts whose selected IDs are not the canonical request', async () => {
        const client = capableClient({
            selectContentPackageImport: vi.fn().mockResolvedValue({
                import_id: 'import-1',
                status: 'awaiting_review',
                revision: 8,
                package_plan_hash: PACKAGE_PLAN_SHA,
                review_sha256: REVIEW_SHA,
                capability_review_sha256: CAPABILITY_REVIEW_SHA,
                selected_component_ids: ['component-a', 'component-a', 'component-z'],
                selection: selection(),
                required_capabilities: ['transforms'],
            }),
        });
        const controller = new ContentPackageController(client);
        await controller.pickAndInspect();
        controller.toggleComponent('component-z');
        controller.toggleComponent('component-a');

        await expect(controller.reviewSelection()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            selection: null,
            error: 'Core 검토 결과가 현재 패키지 스냅샷과 일치하지 않습니다.',
        });
    });

    it.each<[string, (review: ContentPackageTargetReviewDto) => void]>([
        [
            'a duplicate component document',
            (review) => {
                review.documents.push(structuredClone(requiredTargetDocument(review, 'prompt-1')));
            },
        ],
        [
            'a non-contiguous document index',
            (review) => {
                requiredTargetDocument(review, 'transform-set-1').document_index = 4;
            },
        ],
        [
            'an invalid source component digest',
            (review) => {
                requiredTargetDocument(review, 'transform-set-1').source_component_sha256 =
                    'G'.repeat(64);
            },
        ],
        [
            'an unselected source component',
            (review) => {
                requiredTargetDocument(review, 'transform-set-1').source_component_id =
                    'component-missing';
            },
        ],
        [
            'a zero update state revision',
            (review) => {
                requiredTargetDocument(review, 'transform-set-1').expected_target_state_revision =
                    0;
            },
        ],
        [
            'an unknown target disposition',
            (review) => {
                (
                    requiredTargetDocument(review, 'transform-set-1') as unknown as {
                        disposition: string;
                    }
                ).disposition = 'replace';
            },
        ],
        [
            'an out-of-range component document ordinal',
            (review) => {
                requiredTargetDocument(review, 'transform-set-1').component_document_ordinal =
                    0x1_0000_0000;
            },
        ],
        [
            'an unbounded UTF-8 target identifier',
            (review) => {
                requiredTargetDocument(review, 'transform-set-1').target_object_id = '한'.repeat(
                    86,
                );
            },
        ],
        [
            'a whitespace-padded target revision identifier',
            (review) => {
                requiredTargetDocument(review, 'transform-set-1').expected_target_revision_id =
                    ' transform-revision-3';
            },
        ],
        [
            'more than 200 target documents',
            (review) => {
                review.documents = Array.from({ length: 201 }, (_, index) => ({
                    source_component_id: 'component-z',
                    component_document_ordinal: index,
                    document_index: index,
                    document_kind: 'transform_set' as const,
                    target_object_id: `transform-set-${String(index)}`,
                    disposition: 'update' as const,
                    expected_target_revision_id: `transform-revision-${String(index)}`,
                    expected_target_state_revision: index + 1,
                    source_component_sha256: COMPONENT_Z_SHA,
                    document_sha256: UPDATE_DOCUMENT_SHA,
                }));
            },
        ],
    ])('rejects a selection target review containing %s', async (_label, mutateReview) => {
        const returnedSelection = selection();
        mutateReview(returnedSelection.target_review);
        const client = capableClient({
            selectContentPackageImport: vi.fn().mockResolvedValue({
                import_id: 'import-1',
                status: 'awaiting_review',
                revision: 8,
                package_plan_hash: PACKAGE_PLAN_SHA,
                review_sha256: REVIEW_SHA,
                capability_review_sha256: CAPABILITY_REVIEW_SHA,
                selected_component_ids: ['component-a', 'component-z'],
                selection: returnedSelection,
                required_capabilities: ['prompt_fragments', 'transforms'],
            }),
        });
        const controller = new ContentPackageController(client);
        await controller.pickAndInspect();
        controller.toggleComponent('component-z');
        controller.toggleComponent('component-a');

        await expect(controller.reviewSelection()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            selection: null,
            confirmed_update_targets: [],
        });
    });

    it('rejects a reopened review with a malformed target snapshot', async () => {
        const lifecycle = pendingReview();
        const selectionReview = lifecycle.selection;
        if (selectionReview === null) throw new Error('synthetic selection is missing');
        requiredTargetDocument(
            selectionReview.target_review,
            'transform-set-1',
        ).component_document_ordinal = 2;
        const client = capableClient({
            reopenContentPackageImport: vi.fn().mockResolvedValue({
                inspection: { ...inspection(), revision: 8 },
                lifecycle,
            }),
        });
        const controller = new ContentPackageController(client);

        await expect(controller.resume('import-1')).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({ phase: 'error', selection: null });
    });

    it('rejects an approval receipt whose exact target review changed', async () => {
        vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(APPROVAL_ID);
        const client = capableClient({
            approveContentPackageImport: vi.fn((input: ApproveContentPackageImportInput) => {
                const receipt = approvalReceipt(input.approval_id, input.enable_component_ids);
                requiredTargetDocument(receipt.target_review, 'transform-set-1').document_sha256 =
                    '7'.repeat(64);
                return Promise.resolve(receipt);
            }),
        });
        const controller = new ContentPackageController(client);
        await controller.pickAndInspect();
        controller.toggleComponent('component-z');
        controller.toggleComponent('component-a');
        await controller.reviewSelection();
        controller.toggleApprovedCapability('transforms');
        controller.toggleUpdateTargetConfirmation('component-z', 0);

        await expect(controller.approve()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            approval: null,
        });
    });

    it('loads and resumes bounded durable pending reviews', async () => {
        const client = capableClient({
            listPendingContentPackageImports: vi.fn().mockResolvedValue([pendingReview()]),
        });
        const controller = new ContentPackageController(client);

        await expect(controller.loadPendingImports()).resolves.toBe(true);
        expect(client.listPendingContentPackageImports).toHaveBeenCalledWith({ limit: 100 });
        await expect(controller.resume('import-1')).resolves.toBe(true);

        expect(client.reopenContentPackageImport).toHaveBeenCalledWith({
            import_id: 'import-1',
        });
        expect(get(controller.state)).toMatchObject({
            phase: 'selection_ready',
            revision: 8,
            selection: {
                normalization_evidence_sha256: EVIDENCE_SHA,
                target_review: { target_review_sha256: TARGET_REVIEW_SHA },
            },
            pending_imports: [{ import_id: 'import-1' }],
        });
    });

    it('loads restart-safe completed exports in backend order and exports an exact catalog row', async () => {
        const descriptors = [
            {
                kind: 'lorepia_package' as const,
                source_id: 'import-2',
                sha256: '8'.repeat(64),
                size_bytes: 8192,
                suggested_file_name: 'newer.lorepia.zip',
            },
            {
                kind: 'lorepia_package' as const,
                source_id: 'import-1',
                sha256: EXPORT_SHA,
                size_bytes: 4096,
                suggested_file_name: 'package.synthetic.lorepia.zip',
            },
        ];
        const client = capableClient({
            listCompletedContentPackageExports: vi.fn().mockResolvedValue(descriptors),
        });
        const controller = new ContentPackageController(client);

        await expect(controller.loadPendingImports()).resolves.toBe(true);
        expect(client.listCompletedContentPackageExports).toHaveBeenCalledWith({ limit: 100 });
        expect(get(controller.state)).toMatchObject({
            result: null,
            completed_exports_loading: false,
            completed_exports_error: null,
            completed_package_exports: descriptors,
        });

        await expect(controller.exportCompletedPackageFromCatalog('import-1')).resolves.toBe(true);
        expect(client.exportContentSource).toHaveBeenCalledWith({
            kind: 'content_package',
            import_id: 'import-1',
        });
        expect(get(controller.state)).toMatchObject({
            export_receipt: {
                source_id: 'import-1',
                sha256: EXPORT_SHA,
                size_bytes: 4096,
            },
        });
    });

    it('retains the prior verified completed-export catalog when a reload snapshot is malformed', async () => {
        const verified = {
            kind: 'lorepia_package' as const,
            source_id: 'import-1',
            sha256: EXPORT_SHA,
            size_bytes: 4096,
            suggested_file_name: 'package.synthetic.lorepia.zip',
        };
        const listCompletedContentPackageExports = vi
            .fn()
            .mockResolvedValueOnce([verified])
            .mockResolvedValueOnce([verified, verified]);
        const controller = new ContentPackageController(
            capableClient({ listCompletedContentPackageExports }),
        );

        await expect(controller.loadCompletedPackageExports()).resolves.toBe(true);
        await expect(controller.loadCompletedPackageExports()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            completed_package_exports: [verified],
            completed_exports_loading: false,
            completed_exports_error: 'Core 완료 패키지 내보내기 목록이 안전한 스냅샷이 아닙니다.',
        });
    });

    it('rejects catalog delivery evidence whose immutable hash changed after restart', async () => {
        const descriptor = {
            kind: 'lorepia_package' as const,
            source_id: 'import-1',
            sha256: EXPORT_SHA,
            size_bytes: 4096,
            suggested_file_name: 'package.synthetic.lorepia.zip',
        };
        const client = capableClient({
            listCompletedContentPackageExports: vi.fn().mockResolvedValue([descriptor]),
            exportContentSource: vi.fn().mockResolvedValue({
                kind: 'lorepia_package',
                source_id: 'import-1',
                sha256: '0'.repeat(64),
                size_bytes: 4096,
                file_name: 'package.synthetic.lorepia.zip',
            }),
        });
        const controller = new ContentPackageController(client);
        await controller.loadCompletedPackageExports();

        await expect(controller.exportCompletedPackageFromCatalog('import-1')).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            export_receipt: null,
            export_error: 'Core 내보내기 영수증이 완료된 패키지와 일치하지 않습니다.',
        });
    });

    it('keeps a discarded review cleared when an older approval resolves later', async () => {
        vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(APPROVAL_ID);
        const pendingApproval = deferred<ApproveContentPackageImportReceiptDto>();
        const client = capableClient({
            approveContentPackageImport: vi.fn(() => pendingApproval.promise),
        });
        const controller = new ContentPackageController(client);
        await controller.pickAndInspect();
        controller.toggleComponent('component-z');
        await controller.reviewSelection();
        controller.toggleApprovedCapability('transforms');
        controller.toggleUpdateTargetConfirmation('component-z', 0);

        const approve = controller.approve();
        await expect(controller.discard()).resolves.toBe(true);
        expect(client.discardContentPackageImport).toHaveBeenCalledWith({
            import_id: 'import-1',
            expected_revision: 8,
            expected_review_sha256: REVIEW_SHA,
            expected_import_plan_sha256: IMPORT_PLAN_SHA,
            expected_capability_review_sha256: CAPABILITY_REVIEW_SHA,
        });

        pendingApproval.resolve(approvalReceipt());
        await expect(approve).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'idle',
            inspection: null,
            selection: null,
            approval: null,
            selected_component_ids: [],
        });
    });

    it('retains the inspection when Core fails to discard it', async () => {
        const client = capableClient({
            discardContentPackageImport: vi.fn().mockRejectedValue(new Error('discard failed')),
        });
        const controller = new ContentPackageController(client);
        await controller.pickAndInspect();

        await expect(controller.discard()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            inspection: { import_id: 'import-1' },
        });
    });
});
