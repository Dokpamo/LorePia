import {
    INITIAL_CONTENT_PACKAGE_STATE,
    type ContentPackageState,
} from '../content-package-controller';

export function contentPackageState(): ContentPackageState {
    return {
        ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
        phase: 'ready',
        status: 'inspected',
        revision: 3,
        inspection: {
            import_id: 'import-1',
            revision: 3,
            package_plan_hash: 'a'.repeat(64),
            review_sha256: 'b'.repeat(64),
            capability_review_sha256: 'c'.repeat(64),
            source_size_bytes: 2048,
            total_uncompressed_size_bytes: 4096,
            asset_count: 0,
            local_import_allowed: true,
            redistribution_status: 'denied_by_manifest',
            manifest: {
                package_id: 'package.synthetic',
                name: '<img src=x onerror=alert(1)>',
                version: '1.0.0',
                author: 'project-owned synthetic',
                license: 'LicenseRef-Private',
                redistribution_allowed: false,
                required_app_version: null,
                required_capabilities: ['prompt_fragments'],
            },
            components: [
                {
                    id: 'component-safe',
                    kind: 'prompt_preset',
                    disposition: 'importable',
                    required_capabilities: [],
                    dependency_ids: [],
                    conflict_ids: [],
                    asset_count: 0,
                },
                {
                    id: 'component-quarantined',
                    kind: 'transform_set',
                    disposition: 'quarantined',
                    required_capabilities: [],
                    dependency_ids: [],
                    conflict_ids: [],
                    asset_count: 0,
                },
            ],
            issues: [
                {
                    severity: 'warning',
                    code: 'quarantined-transform',
                    message: '실행 가능한 변환은 비활성 격리됨',
                },
                {
                    severity: 'warning',
                    code: 'unsupported-html',
                    message: '임의 HTML',
                },
            ],
            capability_decisions: [],
        },
    };
}

export function contentPackageSelectionState(): ContentPackageState {
    return {
        ...contentPackageState(),
        phase: 'selection_ready',
        status: 'awaiting_review',
        revision: 4,
        selected_component_ids: ['component-safe'],
        required_capabilities: [],
        selection: {
            content_selection_plan_hash: 'd'.repeat(64),
            import_plan_sha256: 'e'.repeat(64),
            normalization_evidence_sha256: 'f'.repeat(64),
            normalization_evidence: [],
            target_review: {
                target_review_sha256: '1'.repeat(64),
                documents: [
                    {
                        source_component_id: 'component-safe',
                        component_document_ordinal: 0,
                        document_index: 0,
                        document_kind: 'prompt_preset',
                        target_object_id: 'prompt-existing',
                        disposition: 'update',
                        expected_target_revision_id: 'prompt-revision-7',
                        expected_target_state_revision: 8,
                        source_component_sha256: '2'.repeat(64),
                        document_sha256: '3'.repeat(64),
                    },
                    {
                        source_component_id: 'component-safe',
                        component_document_ordinal: 1,
                        document_index: 1,
                        document_kind: 'prompt_preset',
                        target_object_id: 'prompt-new',
                        disposition: 'create',
                        expected_target_revision_id: null,
                        expected_target_state_revision: null,
                        source_component_sha256: '2'.repeat(64),
                        document_sha256: '4'.repeat(64),
                    },
                ],
            },
        },
    };
}

export function completedContentPackageState(): ContentPackageState {
    return {
        ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
        result: {
            import_id: 'import-1',
            package_id: 'package.synthetic',
            status: 'completed',
            revision: 5,
            committed_document_ids: ['prompt-1'],
            asset_ids: [],
        },
    };
}

export function restartedCompletedExportState(): ContentPackageState {
    return {
        ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
        completed_package_exports: [
            {
                kind: 'lorepia_package',
                source_id: 'import-2',
                sha256: '8'.repeat(64),
                size_bytes: 8192,
                suggested_file_name: 'newer.lorepia.zip',
            },
            {
                kind: 'lorepia_package',
                source_id: 'import-1',
                sha256: '9'.repeat(64),
                size_bytes: 4096,
                suggested_file_name: 'older.lorepia.zip',
            },
        ],
    };
}
