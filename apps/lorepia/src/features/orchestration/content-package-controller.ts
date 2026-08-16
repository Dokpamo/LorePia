import { get, writable, type Readable } from 'svelte/store';

import type {
    ApprovableContentPackageCapabilityDto,
    ConfirmedContentPackageUpdateTargetDto,
    ContentPackageApprovalReviewDto,
    ContentPackageCapabilityDto,
    ContentPackageClientApi,
    ContentPackageImportReviewDto,
    ContentPackageImportStatusDto,
    ContentPackageInspectionReviewDto,
    ContentPackageSelectionReviewDto,
    ContentPackageTargetDocumentKindDto,
    ContentPackageTargetReviewDocumentDto,
    ContentPackageTargetReviewDto,
    ContentSourceExportDescriptorDto,
    ContentSourceExportReceiptDto,
    CommitContentPackageImportReceiptDto,
    LorepiaClient,
} from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { normalizeClientError } from '../../lib/ipc/errors';

export type ContentPackagePhase =
    | 'idle'
    | 'listing'
    | 'picking'
    | 'resuming'
    | 'ready'
    | 'selecting'
    | 'selection_ready'
    | 'approving'
    | 'approved'
    | 'committing'
    | 'unavailable'
    | 'error';

export type ContentPackageCapableClient = LorepiaClient & Partial<ContentPackageClientApi>;

export const MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS = 200;
export const MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS = 100;

export interface ContentPackageState {
    phase: ContentPackagePhase;
    inspection: ContentPackageInspectionReviewDto | null;
    status: ContentPackageImportStatusDto | null;
    revision: number | null;
    selection: ContentPackageSelectionReviewDto | null;
    approval: ContentPackageApprovalReviewDto | null;
    result: CommitContentPackageImportReceiptDto | null;
    pending_imports: ContentPackageImportReviewDto[];
    selected_component_ids: string[];
    enabled_component_ids: string[];
    required_capabilities: ContentPackageCapabilityDto[];
    approved_capabilities: ApprovableContentPackageCapabilityDto[];
    confirmed_update_targets: ConfirmedContentPackageUpdateTargetDto[];
    completed_package_exports: ContentSourceExportDescriptorDto[];
    completed_exports_loading: boolean;
    completed_exports_error: string | null;
    exporting_import_id: string | null;
    export_receipt: ContentSourceExportReceiptDto | null;
    export_error: string | null;
    error: string | null;
    announcement: string;
}

export const INITIAL_CONTENT_PACKAGE_STATE: ContentPackageState = {
    phase: 'idle',
    inspection: null,
    status: null,
    revision: null,
    selection: null,
    approval: null,
    result: null,
    pending_imports: [],
    selected_component_ids: [],
    enabled_component_ids: [],
    required_capabilities: [],
    approved_capabilities: [],
    confirmed_update_targets: [],
    completed_package_exports: [],
    completed_exports_loading: false,
    completed_exports_error: null,
    exporting_import_id: null,
    export_receipt: null,
    export_error: null,
    error: null,
    announcement: '',
};

function retainedCompletedExportCatalog(
    state: ContentPackageState,
): Pick<
    ContentPackageState,
    'completed_package_exports' | 'completed_exports_loading' | 'completed_exports_error'
> {
    return {
        completed_package_exports: state.completed_package_exports,
        completed_exports_loading: state.completed_exports_loading,
        completed_exports_error: state.completed_exports_error,
    };
}

const APPROVABLE_CAPABILITIES = new Set<ContentPackageCapabilityDto>([
    'transforms',
    'declarative_interactions',
]);

const TARGET_DOCUMENT_KINDS = new Set<ContentPackageTargetDocumentKindDto>([
    'prompt_preset',
    'knowledge_book',
    'memory_profile',
    'transform_set',
    'interaction_rule_set',
    'content_module',
    'character_content',
]);
const MAX_U32 = 0xffff_ffff;

function errorLabel(error: unknown): string {
    const normalized = normalizeClientError(error);
    return normalized.messageKey === 'error.unexpected'
        ? t('content_package.error.generic')
        : normalized.messageKey;
}

function sortedUnique<Value extends string>(values: readonly Value[]): Value[] {
    return [...new Set(values)].sort();
}

function exactArray(left: readonly string[], right: readonly string[]): boolean {
    return JSON.stringify(left) === JSON.stringify(right);
}

function exactEvidence(
    left: ContentPackageSelectionReviewDto['normalization_evidence'],
    right: ContentPackageSelectionReviewDto['normalization_evidence'],
): boolean {
    return JSON.stringify(left) === JSON.stringify(right);
}

function targetDocumentKey(
    document: Pick<
        ContentPackageTargetReviewDocumentDto,
        'source_component_id' | 'component_document_ordinal'
    >,
): string {
    return `${document.source_component_id}:${String(document.component_document_ordinal)}`;
}

function isSha256(value: string): boolean {
    return /^[0-9a-f]{64}$/.test(value);
}

function isBoundedIdentifier(value: string): boolean {
    if (
        value.length === 0 ||
        value.trim() !== value ||
        new TextEncoder().encode(value).length > 256
    ) {
        return false;
    }
    for (let index = 0; index < value.length; index += 1) {
        const codeUnit = value.charCodeAt(index);
        if (codeUnit <= 0x1f || (codeUnit >= 0x7f && codeUnit <= 0x9f)) return false;
    }
    return true;
}

function isSafeExportFileName(value: string): boolean {
    if (
        value.trim().length === 0 ||
        value === '.' ||
        value === '..' ||
        value.length > 1020 ||
        Array.from(value).length > 255 ||
        new TextEncoder().encode(value).length > 1020 ||
        value.includes('/') ||
        value.includes('\\')
    ) {
        return false;
    }
    for (let index = 0; index < value.length; index += 1) {
        const codeUnit = value.charCodeAt(index);
        if (codeUnit <= 0x1f || (codeUnit >= 0x7f && codeUnit <= 0x9f)) return false;
    }
    return true;
}

function isSafeSuggestedExportFileName(value: string): boolean {
    const encoded = new TextEncoder().encode(value);
    const stem = value.split('.')[0]?.toUpperCase() ?? '';
    const reservedWindowsStems = new Set([
        'CON',
        'PRN',
        'AUX',
        'NUL',
        'COM1',
        'COM2',
        'COM3',
        'COM4',
        'COM5',
        'COM6',
        'COM7',
        'COM8',
        'COM9',
        'LPT1',
        'LPT2',
        'LPT3',
        'LPT4',
        'LPT5',
        'LPT6',
        'LPT7',
        'LPT8',
        'LPT9',
    ]);
    return (
        encoded.length > 0 &&
        encoded.length <= 128 &&
        /^[A-Za-z0-9._-]+$/.test(value) &&
        !value.startsWith('.') &&
        !value.endsWith('.') &&
        !value.includes('..') &&
        !reservedWindowsStems.has(stem)
    );
}

function projectPackageExportReceipt(
    receipt: ContentSourceExportReceiptDto,
    importId: string,
): ContentSourceExportReceiptDto | null {
    if (
        receipt.kind !== 'lorepia_package' ||
        receipt.source_id !== importId ||
        !isSha256(receipt.sha256) ||
        !Number.isSafeInteger(receipt.size_bytes) ||
        receipt.size_bytes <= 0 ||
        !isSafeExportFileName(receipt.file_name)
    ) {
        return null;
    }
    return {
        kind: receipt.kind,
        source_id: receipt.source_id,
        sha256: receipt.sha256,
        size_bytes: receipt.size_bytes,
        file_name: receipt.file_name,
    };
}

function projectCompletedPackageExports(
    descriptors: ContentSourceExportDescriptorDto[],
): ContentSourceExportDescriptorDto[] | null {
    if (descriptors.length > MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS) return null;
    const sourceIds = new Set<string>();
    const projected: ContentSourceExportDescriptorDto[] = [];
    for (const descriptor of descriptors) {
        if (
            descriptor.kind !== 'lorepia_package' ||
            !isBoundedIdentifier(descriptor.source_id) ||
            !isSha256(descriptor.sha256) ||
            !Number.isSafeInteger(descriptor.size_bytes) ||
            descriptor.size_bytes <= 0 ||
            !isSafeSuggestedExportFileName(descriptor.suggested_file_name) ||
            sourceIds.has(descriptor.source_id)
        ) {
            return null;
        }
        sourceIds.add(descriptor.source_id);
        projected.push({
            kind: descriptor.kind,
            source_id: descriptor.source_id,
            sha256: descriptor.sha256,
            size_bytes: descriptor.size_bytes,
            suggested_file_name: descriptor.suggested_file_name,
        });
    }
    return projected;
}

function validTargetReview(
    targetReview: ContentPackageTargetReviewDto,
    selectedComponentIds: readonly string[],
): boolean {
    if (
        !isSha256(targetReview.target_review_sha256) ||
        targetReview.documents.length > MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS
    ) {
        return false;
    }
    const selected = new Set(selectedComponentIds);
    const documentKeys = new Set<string>();
    const targetObjectIds = new Set<string>();
    const ordinalsByComponent = new Map<string, number[]>();
    for (const [expectedDocumentIndex, document] of targetReview.documents.entries()) {
        const key = targetDocumentKey(document);
        if (
            !selected.has(document.source_component_id) ||
            !isBoundedIdentifier(document.source_component_id) ||
            !Number.isSafeInteger(document.component_document_ordinal) ||
            document.component_document_ordinal < 0 ||
            document.component_document_ordinal > MAX_U32 ||
            !Number.isSafeInteger(document.document_index) ||
            document.document_index < 0 ||
            document.document_index > MAX_U32 ||
            document.document_index !== expectedDocumentIndex ||
            !TARGET_DOCUMENT_KINDS.has(document.document_kind) ||
            !isBoundedIdentifier(document.target_object_id) ||
            !isSha256(document.source_component_sha256) ||
            !isSha256(document.document_sha256) ||
            documentKeys.has(key) ||
            targetObjectIds.has(document.target_object_id)
        ) {
            return false;
        }
        documentKeys.add(key);
        targetObjectIds.add(document.target_object_id);
        const ordinals = ordinalsByComponent.get(document.source_component_id) ?? [];
        ordinals.push(document.component_document_ordinal);
        ordinalsByComponent.set(document.source_component_id, ordinals);
        const disposition = document.disposition as string;
        if (disposition === 'create') {
            if (
                document.expected_target_revision_id !== null ||
                document.expected_target_state_revision !== null
            ) {
                return false;
            }
        } else if (disposition === 'update') {
            if (
                document.expected_target_revision_id === null ||
                !isBoundedIdentifier(document.expected_target_revision_id) ||
                document.expected_target_state_revision === null ||
                !Number.isSafeInteger(document.expected_target_state_revision) ||
                document.expected_target_state_revision <= 0
            ) {
                return false;
            }
        } else {
            return false;
        }
    }
    for (const ordinals of ordinalsByComponent.values()) {
        ordinals.sort((left, right) => left - right);
        if (ordinals.some((ordinal, expectedOrdinal) => ordinal !== expectedOrdinal)) return false;
    }
    return true;
}

function exactTargetReview(
    left: ContentPackageTargetReviewDto,
    right: ContentPackageTargetReviewDto,
): boolean {
    return JSON.stringify(left) === JSON.stringify(right);
}

function updateTargetConfirmation(
    document: ContentPackageTargetReviewDocumentDto,
): ConfirmedContentPackageUpdateTargetDto | null {
    if (
        document.disposition !== 'update' ||
        document.expected_target_revision_id === null ||
        document.expected_target_state_revision === null
    ) {
        return null;
    }
    return {
        source_component_id: document.source_component_id,
        component_document_ordinal: document.component_document_ordinal,
        target_object_id: document.target_object_id,
        expected_target_revision_id: document.expected_target_revision_id,
        expected_target_state_revision: document.expected_target_state_revision,
    };
}

function requiredUpdateTargetConfirmations(
    targetReview: ContentPackageTargetReviewDto,
): ConfirmedContentPackageUpdateTargetDto[] {
    return targetReview.documents.flatMap((document) => {
        const confirmation = updateTargetConfirmation(document);
        return confirmation === null ? [] : [confirmation];
    });
}

function requiredCapabilities(
    inspection: ContentPackageInspectionReviewDto,
    selectedComponentIds: readonly string[],
): ContentPackageCapabilityDto[] {
    const selected = new Set(selectedComponentIds);
    return sortedUnique(
        inspection.components
            .filter((component) => selected.has(component.id))
            .flatMap((component) => component.required_capabilities),
    );
}

function requiredApprovals(
    capabilities: readonly ContentPackageCapabilityDto[],
): ApprovableContentPackageCapabilityDto[] {
    return sortedUnique(
        capabilities.filter((capability) =>
            APPROVABLE_CAPABILITIES.has(capability),
        ) as ApprovableContentPackageCapabilityDto[],
    );
}

function phaseForStatus(
    status: ContentPackageImportStatusDto,
    hasSelection: boolean,
    hasApproval: boolean,
): ContentPackagePhase | null {
    switch (status) {
        case 'inspected':
            return 'ready';
        case 'awaiting_review':
            return hasSelection ? 'selection_ready' : null;
        case 'approved':
        case 'committing':
            return hasSelection && hasApproval ? 'approved' : null;
        default:
            return null;
    }
}

export class ContentPackageController {
    private readonly mutable = writable<ContentPackageState>(
        structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
    );
    readonly state: Readable<ContentPackageState> = this.mutable;

    private operationEpoch = 0;
    private catalogEpoch = 0;

    constructor(private readonly client: ContentPackageCapableClient) {
        if (
            client.listPendingContentPackageImports === undefined ||
            client.pickContentPackageImport === undefined ||
            client.reopenContentPackageImport === undefined ||
            client.selectContentPackageImport === undefined ||
            client.approveContentPackageImport === undefined ||
            client.commitContentPackageImport === undefined ||
            client.discardContentPackageImport === undefined
        ) {
            const message = t('content_package.error.unsupported_flow');
            this.mutable.set({
                ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
                phase: 'unavailable',
                error: message,
                announcement: message,
            });
        }
    }

    private update(updater: (state: ContentPackageState) => ContentPackageState): void {
        this.mutable.update(updater);
    }

    private isCurrent(epoch: number): boolean {
        return epoch === this.operationEpoch;
    }

    private markUnavailable(message: string): false {
        this.update((state) => ({
            ...state,
            phase: 'unavailable',
            error: message,
            announcement: message,
        }));
        return false;
    }

    private markError(epoch: number, error: unknown): false {
        if (!this.isCurrent(epoch)) return false;
        const message = errorLabel(error);
        this.update((state) => ({
            ...state,
            phase: 'error',
            error: message,
            announcement: message,
        }));
        return false;
    }

    private markSnapshotMismatch(epoch: number): false {
        if (!this.isCurrent(epoch)) return false;
        const message = t('content_package.error.review_mismatch');
        this.update((state) => ({
            ...state,
            phase: 'error',
            error: message,
            announcement: message,
        }));
        return false;
    }

    async loadCompletedPackageExports(): Promise<boolean> {
        const list = this.client.listCompletedContentPackageExports;
        if (list === undefined) {
            const message = t('content_package.error.unsupported_exports');
            this.update((state) => ({
                ...state,
                completed_exports_loading: false,
                completed_exports_error: message,
                announcement: message,
            }));
            return false;
        }
        const epoch = ++this.catalogEpoch;
        this.update((state) => ({
            ...state,
            completed_exports_loading: true,
            completed_exports_error: null,
        }));
        try {
            const descriptors = await list.call(this.client, {
                limit: MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS,
            });
            if (epoch !== this.catalogEpoch) return false;
            const projected = projectCompletedPackageExports(descriptors);
            if (projected === null) {
                const message = t('content_package.error.unsafe_exports');
                this.update((state) => ({
                    ...state,
                    completed_exports_loading: false,
                    completed_exports_error: message,
                    announcement: message,
                }));
                return false;
            }
            this.update((state) => ({
                ...state,
                completed_package_exports: projected,
                completed_exports_loading: false,
                completed_exports_error: null,
            }));
            return true;
        } catch (error: unknown) {
            if (epoch !== this.catalogEpoch) return false;
            const message = errorLabel(error);
            this.update((state) => ({
                ...state,
                completed_exports_loading: false,
                completed_exports_error: message,
                announcement: message,
            }));
            return false;
        }
    }

    async loadPendingImports(): Promise<boolean> {
        const completedExportsLoad = this.loadCompletedPackageExports();
        const list = this.client.listPendingContentPackageImports;
        if (list === undefined) {
            await completedExportsLoad;
            return this.markUnavailable(t('content_package.error.unsupported_pending'));
        }
        const epoch = ++this.operationEpoch;
        const previous = get(this.mutable);
        if (previous.phase === 'idle') {
            this.update((state) => ({ ...state, phase: 'listing', error: null }));
        }
        try {
            const pendingImports = await list.call(this.client, { limit: 100 });
            if (!this.isCurrent(epoch)) return false;
            this.update((state) => ({
                ...state,
                phase: state.phase === 'listing' ? 'idle' : state.phase,
                pending_imports: pendingImports.slice(0, 100),
                error: null,
            }));
            await completedExportsLoad;
            return true;
        } catch (error: unknown) {
            await completedExportsLoad;
            return this.markError(epoch, error);
        }
    }

    async pickAndInspect(): Promise<boolean> {
        const picker = this.client.pickContentPackageImport;
        if (picker === undefined) {
            return this.markUnavailable(t('content_package.error.unsupported_select'));
        }
        const epoch = ++this.operationEpoch;
        const previous = get(this.mutable);
        const pendingImports = previous.pending_imports;
        const completedExportCatalog = retainedCompletedExportCatalog(previous);
        this.mutable.set({
            ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
            ...completedExportCatalog,
            phase: 'picking',
            pending_imports: pendingImports,
        });

        try {
            const inspection = await picker.call(this.client);
            if (!this.isCurrent(epoch)) return false;
            if (inspection === null) {
                this.mutable.set({
                    ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
                    ...completedExportCatalog,
                    pending_imports: pendingImports,
                    announcement: t('content_package.notice.selection_cancelled'),
                });
                return false;
            }
            this.mutable.set({
                ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
                ...completedExportCatalog,
                phase: 'ready',
                inspection,
                status: 'inspected',
                revision: inspection.revision,
                pending_imports: pendingImports,
                announcement: t('content_package.notice.review_ready'),
            });
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    async resume(importId: string): Promise<boolean> {
        const reopen = this.client.reopenContentPackageImport;
        if (reopen === undefined) {
            return this.markUnavailable(t('content_package.error.unsupported_reopen'));
        }
        const epoch = ++this.operationEpoch;
        const previous = get(this.mutable);
        const pendingImports = previous.pending_imports;
        const completedExportCatalog = retainedCompletedExportCatalog(previous);
        this.update((state) => ({
            ...state,
            phase: 'resuming',
            error: null,
            announcement: t('content_package.notice.reopening'),
        }));
        try {
            const workspace = await reopen.call(this.client, { import_id: importId });
            if (!this.isCurrent(epoch)) return false;
            const { inspection, lifecycle } = workspace;
            const phase = phaseForStatus(
                lifecycle.status,
                lifecycle.selection !== null,
                lifecycle.approval !== null,
            );
            const selectedComponentIds = sortedUnique(lifecycle.selected_component_ids);
            if (
                phase === null ||
                inspection.import_id !== importId ||
                lifecycle.import_id !== importId ||
                lifecycle.package_id !== inspection.manifest.package_id ||
                lifecycle.revision !== inspection.revision ||
                lifecycle.package_plan_hash !== inspection.package_plan_hash ||
                lifecycle.review_sha256 !== inspection.review_sha256 ||
                lifecycle.capability_review_sha256 !== inspection.capability_review_sha256 ||
                !exactArray(lifecycle.selected_component_ids, selectedComponentIds) ||
                (lifecycle.selection !== null &&
                    !validTargetReview(lifecycle.selection.target_review, selectedComponentIds))
            ) {
                return this.markSnapshotMismatch(epoch);
            }
            const capabilities = requiredCapabilities(inspection, selectedComponentIds);
            this.mutable.set({
                ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
                ...completedExportCatalog,
                phase,
                inspection,
                status: lifecycle.status,
                revision: lifecycle.revision,
                selection: lifecycle.selection,
                approval: lifecycle.approval,
                pending_imports: pendingImports,
                selected_component_ids: selectedComponentIds,
                enabled_component_ids: lifecycle.approval?.enabled_component_ids ?? [],
                required_capabilities: capabilities,
                approved_capabilities: lifecycle.approval?.approved_capabilities ?? [],
                confirmed_update_targets: [],
                announcement: t('content_package.notice.reopened'),
            });
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    toggleComponent(componentId: string): boolean {
        const state = get(this.mutable);
        if (state.phase !== 'ready' || state.inspection === null) return false;
        const component = state.inspection.components.find(({ id }) => id === componentId);
        if (component?.disposition !== 'importable') return false;

        const isSelected = state.selected_component_ids.includes(componentId);
        this.update((current) => ({
            ...current,
            selected_component_ids: isSelected
                ? current.selected_component_ids.filter((id) => id !== componentId)
                : sortedUnique([...current.selected_component_ids, componentId]),
            error: null,
            announcement: isSelected
                ? t('content_package.notice.excluded')
                : t('content_package.notice.included'),
        }));
        return true;
    }

    async reviewSelection(): Promise<boolean> {
        const select = this.client.selectContentPackageImport;
        if (select === undefined) {
            return this.markUnavailable(t('content_package.error.unsupported_selection_review'));
        }
        const state = get(this.mutable);
        if (
            state.phase !== 'ready' ||
            state.inspection === null ||
            state.revision === null ||
            state.selected_component_ids.length === 0
        ) {
            return false;
        }
        const epoch = ++this.operationEpoch;
        const inspection = state.inspection;
        const selectedComponentIds = sortedUnique(state.selected_component_ids);
        this.update((current) => ({
            ...current,
            phase: 'selecting',
            error: null,
            announcement: t('content_package.notice.reviewing_selection'),
        }));

        try {
            const receipt = await select.call(this.client, {
                import_id: inspection.import_id,
                expected_revision: state.revision,
                expected_package_plan_hash: inspection.package_plan_hash,
                expected_review_sha256: inspection.review_sha256,
                expected_capability_review_sha256: inspection.capability_review_sha256,
                selected_component_ids: selectedComponentIds,
            });
            if (!this.isCurrent(epoch)) return false;
            if (
                receipt.import_id !== inspection.import_id ||
                receipt.package_plan_hash !== inspection.package_plan_hash ||
                receipt.review_sha256 !== inspection.review_sha256 ||
                receipt.capability_review_sha256 !== inspection.capability_review_sha256 ||
                !exactArray(receipt.selected_component_ids, selectedComponentIds) ||
                !validTargetReview(receipt.selection.target_review, selectedComponentIds)
            ) {
                return this.markSnapshotMismatch(epoch);
            }
            this.update((current) => ({
                ...current,
                phase: 'selection_ready',
                status: receipt.status,
                revision: receipt.revision,
                selection: receipt.selection,
                approval: null,
                result: null,
                selected_component_ids: selectedComponentIds,
                enabled_component_ids: [],
                required_capabilities: sortedUnique(receipt.required_capabilities),
                approved_capabilities: [],
                confirmed_update_targets: [],
                error: null,
                announcement: t('content_package.notice.evidence_ready'),
            }));
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    toggleEnabledComponent(componentId: string): boolean {
        const state = get(this.mutable);
        if (
            state.phase !== 'selection_ready' ||
            !state.selected_component_ids.includes(componentId)
        ) {
            return false;
        }
        const isEnabled = state.enabled_component_ids.includes(componentId);
        this.update((current) => ({
            ...current,
            enabled_component_ids: isEnabled
                ? current.enabled_component_ids.filter((id) => id !== componentId)
                : sortedUnique([...current.enabled_component_ids, componentId]),
            announcement: isEnabled
                ? t('content_package.notice.import_inactive')
                : t('content_package.notice.import_active'),
        }));
        return true;
    }

    toggleApprovedCapability(capability: ApprovableContentPackageCapabilityDto): boolean {
        const state = get(this.mutable);
        if (
            state.phase !== 'selection_ready' ||
            !requiredApprovals(state.required_capabilities).includes(capability)
        ) {
            return false;
        }
        const isApproved = state.approved_capabilities.includes(capability);
        this.update((current) => ({
            ...current,
            approved_capabilities: isApproved
                ? current.approved_capabilities.filter((value) => value !== capability)
                : sortedUnique([...current.approved_capabilities, capability]),
            announcement: isApproved
                ? t('content_package.notice.capability_revoked')
                : t('content_package.notice.capability_approved'),
        }));
        return true;
    }

    toggleUpdateTargetConfirmation(
        sourceComponentId: string,
        componentDocumentOrdinal: number,
    ): boolean {
        const state = get(this.mutable);
        if (state.phase !== 'selection_ready' || state.selection === null) return false;
        const matchingDocuments = state.selection.target_review.documents.filter(
            (document) =>
                document.source_component_id === sourceComponentId &&
                document.component_document_ordinal === componentDocumentOrdinal,
        );
        if (matchingDocuments.length !== 1) return false;
        const document = matchingDocuments[0];
        if (document?.disposition !== 'update') return false;
        const key = targetDocumentKey(document);
        const confirmedKeys = new Set(state.confirmed_update_targets.map(targetDocumentKey));
        if (confirmedKeys.has(key)) confirmedKeys.delete(key);
        else confirmedKeys.add(key);
        const confirmedUpdateTargets = requiredUpdateTargetConfirmations(
            state.selection.target_review,
        ).filter((confirmation) => confirmedKeys.has(targetDocumentKey(confirmation)));
        this.update((current) => ({
            ...current,
            confirmed_update_targets: confirmedUpdateTargets,
            error: null,
            announcement: confirmedKeys.has(key)
                ? t('content_package.notice.target_confirmed')
                : t('content_package.notice.target_unconfirmed'),
        }));
        return true;
    }

    async approve(): Promise<boolean> {
        const approve = this.client.approveContentPackageImport;
        if (approve === undefined) {
            return this.markUnavailable(t('content_package.error.unsupported_approval'));
        }
        const state = get(this.mutable);
        if (
            state.phase !== 'selection_ready' ||
            state.inspection === null ||
            state.selection === null ||
            state.revision === null
        ) {
            return false;
        }
        const required = requiredApprovals(state.required_capabilities);
        const approvedCapabilities = sortedUnique(state.approved_capabilities);
        const requiredUpdateTargets = requiredUpdateTargetConfirmations(
            state.selection.target_review,
        );
        if (
            !validTargetReview(state.selection.target_review, state.selected_component_ids) ||
            !exactArray(required, approvedCapabilities) ||
            JSON.stringify(requiredUpdateTargets) !== JSON.stringify(state.confirmed_update_targets)
        ) {
            return false;
        }

        const epoch = ++this.operationEpoch;
        const { inspection, selection } = state;
        const enabledComponentIds = sortedUnique(state.enabled_component_ids);
        const approvalId = globalThis.crypto.randomUUID();
        this.update((current) => ({
            ...current,
            phase: 'approving',
            error: null,
            announcement: t('content_package.notice.pinning'),
        }));
        try {
            const receipt = await approve.call(this.client, {
                import_id: inspection.import_id,
                expected_revision: state.revision,
                expected_package_plan_hash: inspection.package_plan_hash,
                expected_content_selection_plan_hash: selection.content_selection_plan_hash,
                expected_review_sha256: inspection.review_sha256,
                expected_import_plan_sha256: selection.import_plan_sha256,
                expected_capability_review_sha256: inspection.capability_review_sha256,
                expected_normalization_evidence_sha256: selection.normalization_evidence_sha256,
                expected_target_review_sha256: selection.target_review.target_review_sha256,
                approval_id: approvalId,
                enable_component_ids: enabledComponentIds,
                approved_capabilities: approvedCapabilities,
                confirmed_update_targets: structuredClone(requiredUpdateTargets),
            });
            if (!this.isCurrent(epoch)) return false;
            if (
                receipt.import_id !== inspection.import_id ||
                receipt.package_plan_hash !== inspection.package_plan_hash ||
                receipt.content_selection_plan_hash !== selection.content_selection_plan_hash ||
                receipt.review_sha256 !== inspection.review_sha256 ||
                receipt.import_plan_sha256 !== selection.import_plan_sha256 ||
                receipt.capability_review_sha256 !== inspection.capability_review_sha256 ||
                receipt.normalization_evidence_sha256 !== selection.normalization_evidence_sha256 ||
                !exactEvidence(receipt.normalization_evidence, selection.normalization_evidence) ||
                !exactTargetReview(receipt.target_review, selection.target_review) ||
                receipt.approval_id !== approvalId ||
                !exactArray(receipt.enabled_component_ids, enabledComponentIds) ||
                !exactArray(receipt.approved_capabilities, approvedCapabilities)
            ) {
                return this.markSnapshotMismatch(epoch);
            }
            this.update((current) => ({
                ...current,
                phase: 'approved',
                status: receipt.status,
                revision: receipt.revision,
                approval: {
                    approval_sha256: receipt.approval_sha256,
                    approval_id: receipt.approval_id,
                    enabled_component_ids: receipt.enabled_component_ids,
                    approved_capabilities: receipt.approved_capabilities,
                },
                error: null,
                announcement: t('content_package.notice.pinned'),
            }));
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    async commit(): Promise<boolean> {
        const commit = this.client.commitContentPackageImport;
        if (commit === undefined) {
            return this.markUnavailable(t('content_package.error.unsupported_commit'));
        }
        const state = get(this.mutable);
        if (
            state.phase !== 'approved' ||
            state.inspection === null ||
            state.selection === null ||
            state.approval === null ||
            state.revision === null
        ) {
            return false;
        }
        const epoch = ++this.operationEpoch;
        const { inspection, selection, approval } = state;
        this.update((current) => ({
            ...current,
            phase: 'committing',
            error: null,
            announcement: t('content_package.notice.importing'),
        }));
        try {
            const result = await commit.call(this.client, {
                import_id: inspection.import_id,
                expected_revision: state.revision,
                expected_package_plan_hash: inspection.package_plan_hash,
                expected_content_selection_plan_hash: selection.content_selection_plan_hash,
                expected_review_sha256: inspection.review_sha256,
                expected_import_plan_sha256: selection.import_plan_sha256,
                expected_approval_sha256: approval.approval_sha256,
                expected_capability_review_sha256: inspection.capability_review_sha256,
                expected_normalization_evidence_sha256: selection.normalization_evidence_sha256,
            });
            if (!this.isCurrent(epoch)) return false;
            if (
                result.import_id !== inspection.import_id ||
                result.package_id !== inspection.manifest.package_id
            ) {
                return this.markSnapshotMismatch(epoch);
            }
            const pendingImports = state.pending_imports.filter(
                ({ import_id }) => import_id !== inspection.import_id,
            );
            this.mutable.set({
                ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
                ...retainedCompletedExportCatalog(state),
                result,
                pending_imports: pendingImports,
                announcement: t('content_package.notice.imported'),
            });
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    async exportCompletedPackage(): Promise<boolean> {
        const state = get(this.mutable);
        if (state.result?.status !== 'completed') return false;
        return this.exportPackageSource(state.result.import_id, null);
    }

    async exportCompletedPackageFromCatalog(importId: string): Promise<boolean> {
        const state = get(this.mutable);
        const matches = state.completed_package_exports.filter(
            (descriptor) => descriptor.source_id === importId,
        );
        if (matches.length !== 1) return false;
        return this.exportPackageSource(importId, matches[0] ?? null);
    }

    private async exportPackageSource(
        importId: string,
        expectedDescriptor: ContentSourceExportDescriptorDto | null,
    ): Promise<boolean> {
        const exportContentSource = this.client.exportContentSource;
        const state = get(this.mutable);
        if (state.exporting_import_id !== null) return false;
        if (exportContentSource === undefined) {
            const message = t('content_package.error.unsupported_export');
            this.update((current) => ({
                ...current,
                export_error: message,
                announcement: message,
            }));
            return false;
        }
        const epoch = ++this.operationEpoch;
        const previousAnnouncement = state.announcement;
        this.update((current) => ({
            ...current,
            exporting_import_id: importId,
            export_error: null,
            announcement: t('content_package.notice.export_picking'),
        }));
        try {
            const receipt = await exportContentSource.call(this.client, {
                kind: 'content_package',
                import_id: importId,
            });
            if (!this.isCurrent(epoch)) return false;
            if (receipt === null) {
                this.update((current) => ({
                    ...current,
                    exporting_import_id: null,
                    export_error: null,
                    announcement: previousAnnouncement,
                }));
                return false;
            }
            const projected = projectPackageExportReceipt(receipt, importId);
            if (
                projected === null ||
                (expectedDescriptor !== null &&
                    (projected.sha256 !== expectedDescriptor.sha256 ||
                        projected.size_bytes !== expectedDescriptor.size_bytes))
            ) {
                const message = t('content_package.error.export_mismatch');
                this.update((current) => ({
                    ...current,
                    exporting_import_id: null,
                    export_error: message,
                    announcement: message,
                }));
                return false;
            }
            this.update((current) => ({
                ...current,
                exporting_import_id: null,
                export_receipt: projected,
                export_error: null,
                announcement: t('content_package.notice.exported', { name: projected.file_name }),
            }));
            return true;
        } catch (error: unknown) {
            if (!this.isCurrent(epoch)) return false;
            const message = errorLabel(error);
            this.update((current) => ({
                ...current,
                exporting_import_id: null,
                export_error: message,
                announcement: message,
            }));
            return false;
        }
    }

    async discard(): Promise<boolean> {
        const discard = this.client.discardContentPackageImport;
        if (discard === undefined) {
            return this.markUnavailable(t('content_package.error.unsupported_discard'));
        }
        const state = get(this.mutable);
        if (state.inspection === null || state.revision === null) return false;
        const epoch = ++this.operationEpoch;
        const { inspection, selection } = state;
        this.update((current) => ({
            ...current,
            error: null,
            announcement: t('content_package.notice.discarding'),
        }));
        try {
            await discard.call(this.client, {
                import_id: inspection.import_id,
                expected_revision: state.revision,
                expected_review_sha256: inspection.review_sha256,
                expected_import_plan_sha256: selection?.import_plan_sha256 ?? null,
                expected_capability_review_sha256: inspection.capability_review_sha256,
            });
            if (!this.isCurrent(epoch)) return false;
            const pendingImports = state.pending_imports.filter(
                ({ import_id }) => import_id !== inspection.import_id,
            );
            this.mutable.set({
                ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
                ...retainedCompletedExportCatalog(state),
                pending_imports: pendingImports,
                announcement: t('content_package.notice.discarded'),
            });
            return true;
        } catch (error: unknown) {
            return this.markError(epoch, error);
        }
    }

    destroy(): void {
        ++this.operationEpoch;
        ++this.catalogEpoch;
    }
}
