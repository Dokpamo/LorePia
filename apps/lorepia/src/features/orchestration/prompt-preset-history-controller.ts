import { get, writable, type Readable } from 'svelte/store';

import { t } from '../../lib/i18n';
import { normalizeClientError } from '../../lib/ipc/errors';
import type {
    LorepiaClient,
    PromptPresetHistoryClientApi,
    PromptPresetRevisionDiffDto,
    PromptPresetRevisionListDto,
    PromptPresetRevisionSummaryDto,
    PromptPresetRollbackReceiptDto,
    PromptPresetRollbackReviewDto,
} from '../../lib/ipc/contracts';

export const MAX_VISIBLE_PROMPT_PRESET_REVISIONS = 100;

export type PromptPresetHistoryPhase =
    'idle' | 'loading' | 'ready' | 'reviewing' | 'applying' | 'error' | 'unavailable';

export interface PromptPresetHistoryState {
    phase: PromptPresetHistoryPhase;
    preset_id: string | null;
    current_revision: number | null;
    revisions: PromptPresetRevisionSummaryDto[];
    truncated: boolean;
    selected_target_revision: number | null;
    diff: PromptPresetRevisionDiffDto | null;
    review: PromptPresetRollbackReviewDto | null;
    receipt: PromptPresetRollbackReceiptDto | null;
    approval_id: string | null;
    error: string | null;
    announcement: string;
}

export const INITIAL_PROMPT_PRESET_HISTORY_STATE: PromptPresetHistoryState = {
    phase: 'idle',
    preset_id: null,
    current_revision: null,
    revisions: [],
    truncated: false,
    selected_target_revision: null,
    diff: null,
    review: null,
    receipt: null,
    approval_id: null,
    error: null,
    announcement: '',
};

export type PromptPresetHistoryCapableClient = LorepiaClient &
    Partial<PromptPresetHistoryClientApi>;

class PromptPresetHistoryContractError extends Error {}

function errorLabel(error: unknown): string {
    if (error instanceof PromptPresetHistoryContractError) return error.message;
    const normalized = normalizeClientError(error);
    return normalized.messageKey === 'error.unexpected'
        ? t('preset_history.error.generic')
        : normalized.messageKey;
}

function isPositiveSafeInteger(value: number): boolean {
    return Number.isSafeInteger(value) && value > 0;
}

function isSha256(value: string): boolean {
    return /^[0-9a-f]{64}$/.test(value);
}

function assertHistoryList(value: PromptPresetRevisionListDto): void {
    if (value.revisions.length > MAX_VISIBLE_PROMPT_PRESET_REVISIONS) {
        throw new PromptPresetHistoryContractError(t('preset_history.error.limit'));
    }
    let previousRevision = 0;
    for (const revision of value.revisions) {
        if (
            !isPositiveSafeInteger(revision.revision) ||
            revision.revision <= previousRevision ||
            revision.revision_id.trim() === '' ||
            revision.name.trim() === '' ||
            !isSha256(revision.sha256)
        ) {
            throw new PromptPresetHistoryContractError(t('preset_history.error.inconsistent'));
        }
        previousRevision = revision.revision;
    }
}

function assertHistoryContainsCurrent(
    value: PromptPresetRevisionListDto,
    currentRevision: number,
): void {
    if (!value.revisions.some((revision) => revision.revision === currentRevision)) {
        throw new PromptPresetHistoryContractError(t('preset_history.error.missing_current'));
    }
}

function assertReviewedDiff(
    presetId: string,
    currentRevision: number,
    targetRevision: number,
    diff: PromptPresetRevisionDiffDto,
    review: PromptPresetRollbackReviewDto,
): void {
    const reviewDiff = review.diff;
    const digests = [
        diff.from_sha256,
        diff.to_sha256,
        diff.diff_sha256,
        review.review_sha256,
        review.expected_current_sha256,
        review.target_sha256,
        review.target_document_sha256,
        review.target_dependency_sha256,
        review.binding_snapshot_sha256,
        reviewDiff.from_sha256,
        reviewDiff.to_sha256,
        reviewDiff.diff_sha256,
    ];
    if (
        diff.preset_id !== presetId ||
        diff.from_revision !== currentRevision ||
        diff.to_revision !== targetRevision ||
        review.preset_id !== presetId ||
        review.expected_current_state_revision !== currentRevision ||
        review.target_revision !== targetRevision ||
        reviewDiff.preset_id !== presetId ||
        reviewDiff.from_revision !== currentRevision ||
        reviewDiff.to_revision !== targetRevision ||
        reviewDiff.from_revision_id !== diff.from_revision_id ||
        reviewDiff.to_revision_id !== diff.to_revision_id ||
        reviewDiff.from_sha256 !== diff.from_sha256 ||
        reviewDiff.to_sha256 !== diff.to_sha256 ||
        reviewDiff.diff_sha256 !== diff.diff_sha256 ||
        review.expected_current_revision_id !== diff.from_revision_id ||
        review.target_revision_id !== diff.to_revision_id ||
        review.expected_current_sha256 !== diff.from_sha256 ||
        review.target_sha256 !== diff.to_sha256 ||
        digests.some((digest) => !isSha256(digest))
    ) {
        throw new PromptPresetHistoryContractError(t('preset_history.error.review_mismatch'));
    }
}

function assertReceipt(
    presetId: string,
    currentRevision: number,
    targetRevision: number,
    reviewSha256: string,
    approvalId: string,
    receipt: PromptPresetRollbackReceiptDto,
): void {
    if (
        receipt.preset_id !== presetId ||
        receipt.target_revision !== targetRevision ||
        receipt.review_sha256 !== reviewSha256 ||
        receipt.approval_id !== approvalId ||
        !isPositiveSafeInteger(receipt.applied_revision) ||
        receipt.applied_revision <= currentRevision ||
        receipt.applied_revision_id.trim() === '' ||
        !isSha256(receipt.applied_sha256) ||
        !isSha256(receipt.approval_sha256)
    ) {
        throw new PromptPresetHistoryContractError(t('preset_history.error.receipt_inconsistent'));
    }
}

export class PromptPresetHistoryController {
    readonly state: Readable<PromptPresetHistoryState>;

    private readonly mutable = writable<PromptPresetHistoryState>(
        structuredClone(INITIAL_PROMPT_PRESET_HISTORY_STATE),
    );
    private contextEpoch = 0;
    private operationEpoch = 0;
    private destroyed = false;

    constructor(
        private readonly client: PromptPresetHistoryCapableClient,
        private readonly approvalIdFactory: () => string = () => crypto.randomUUID(),
    ) {
        this.state = { subscribe: this.mutable.subscribe };
    }

    destroy(): void {
        this.destroyed = true;
        this.contextEpoch += 1;
        this.operationEpoch += 1;
    }

    private operationIsStale(contextEpoch: number, operationEpoch: number): boolean {
        return (
            this.destroyed ||
            contextEpoch !== this.contextEpoch ||
            operationEpoch !== this.operationEpoch
        );
    }

    async load(presetId: string | null, currentRevision: number | null): Promise<void> {
        const epoch = ++this.contextEpoch;
        ++this.operationEpoch;
        const previous = get(this.mutable);
        if (presetId === null || currentRevision === null) {
            this.mutable.set(structuredClone(INITIAL_PROMPT_PRESET_HISTORY_STATE));
            return;
        }
        if (!isPositiveSafeInteger(currentRevision) || presetId.trim() === '') {
            this.mutable.set({
                ...structuredClone(INITIAL_PROMPT_PRESET_HISTORY_STATE),
                phase: 'error',
                preset_id: presetId,
                current_revision: currentRevision,
                error: t('preset_history.error.bad_revision'),
            });
            return;
        }
        const list = this.client.listPromptPresetRevisions;
        if (list === undefined) {
            this.mutable.set({
                ...structuredClone(INITIAL_PROMPT_PRESET_HISTORY_STATE),
                phase: 'unavailable',
                preset_id: presetId,
                current_revision: currentRevision,
                error: t('preset_history.error.unavailable'),
            });
            return;
        }

        const preservedReceipt =
            previous.preset_id === presetId &&
            previous.receipt?.applied_revision === currentRevision
                ? previous.receipt
                : null;
        this.mutable.set({
            ...structuredClone(INITIAL_PROMPT_PRESET_HISTORY_STATE),
            phase: 'loading',
            preset_id: presetId,
            current_revision: currentRevision,
            receipt: preservedReceipt,
            announcement: preservedReceipt ? previous.announcement : '',
        });
        try {
            const result = await list.call(this.client, {
                prompt_preset_id: presetId,
                limit: MAX_VISIBLE_PROMPT_PRESET_REVISIONS,
            });
            if (this.destroyed || epoch !== this.contextEpoch) return;
            assertHistoryList(result);
            assertHistoryContainsCurrent(result, currentRevision);
            this.mutable.update((state) => ({
                ...state,
                phase: 'ready',
                revisions: result.revisions,
                truncated: result.truncated,
                error: null,
            }));
        } catch (error: unknown) {
            if (this.destroyed || epoch !== this.contextEpoch) return;
            this.mutable.update((state) => ({
                ...state,
                phase: 'error',
                error: errorLabel(error),
            }));
        }
    }

    async reviewTarget(targetRevision: number): Promise<boolean> {
        const state = get(this.mutable);
        const presetId = state.preset_id;
        const currentRevision = state.current_revision;
        const target = state.revisions.find((revision) => revision.revision === targetRevision);
        if (
            presetId === null ||
            currentRevision === null ||
            target === undefined ||
            targetRevision >= currentRevision
        ) {
            return false;
        }
        if (!target.rollback_allowed) {
            this.mutable.update((current) => ({
                ...current,
                phase: 'error',
                selected_target_revision: targetRevision,
                diff: null,
                review: null,
                approval_id: null,
                receipt: null,
                error: t('preset_history.error.builtin'),
                announcement: '',
            }));
            return false;
        }
        const diffCommand = this.client.diffPromptPresetRevisions;
        const reviewCommand = this.client.reviewPromptPresetRollback;
        if (diffCommand === undefined || reviewCommand === undefined) {
            this.mutable.update((current) => ({
                ...current,
                phase: 'unavailable',
                error: t('preset_history.error.review_unavailable'),
            }));
            return false;
        }

        const contextEpoch = this.contextEpoch;
        const operationEpoch = ++this.operationEpoch;
        this.mutable.update((current) => ({
            ...current,
            phase: 'reviewing',
            selected_target_revision: targetRevision,
            diff: null,
            review: null,
            receipt: null,
            approval_id: null,
            error: null,
            announcement: '',
        }));
        try {
            const [diff, review] = await Promise.all([
                diffCommand.call(this.client, {
                    prompt_preset_id: presetId,
                    from_revision: currentRevision,
                    to_revision: targetRevision,
                }),
                reviewCommand.call(this.client, {
                    prompt_preset_id: presetId,
                    expected_current_revision: currentRevision,
                    target_revision: targetRevision,
                }),
            ]);
            if (
                this.destroyed ||
                contextEpoch !== this.contextEpoch ||
                operationEpoch !== this.operationEpoch
            ) {
                return false;
            }
            assertReviewedDiff(presetId, currentRevision, targetRevision, diff, review);
            this.mutable.update((current) => ({
                ...current,
                phase: 'ready',
                diff,
                review,
                error: null,
                announcement: t('preset_history.notice.review_ready', { revision: targetRevision }),
            }));
            return true;
        } catch (error: unknown) {
            if (
                this.destroyed ||
                contextEpoch !== this.contextEpoch ||
                operationEpoch !== this.operationEpoch
            ) {
                return false;
            }
            this.mutable.update((current) => ({
                ...current,
                phase: 'error',
                diff: null,
                review: null,
                approval_id: null,
                error: errorLabel(error),
            }));
            return false;
        }
    }

    async applyReviewedRollback(): Promise<PromptPresetRollbackReceiptDto | null> {
        const state = get(this.mutable);
        const presetId = state.preset_id;
        const currentRevision = state.current_revision;
        const targetRevision = state.selected_target_revision;
        const review = state.review;
        const apply = this.client.applyPromptPresetRollback;
        if (
            presetId === null ||
            currentRevision === null ||
            targetRevision === null ||
            review === null ||
            apply === undefined
        ) {
            return null;
        }

        const contextEpoch = this.contextEpoch;
        const operationEpoch = ++this.operationEpoch;
        const approvalId = state.approval_id ?? this.approvalIdFactory();
        this.mutable.update((current) => ({
            ...current,
            phase: 'applying',
            approval_id: approvalId,
            receipt: null,
            error: null,
            announcement: '',
        }));

        let receipt: PromptPresetRollbackReceiptDto;
        try {
            receipt = await apply.call(this.client, {
                prompt_preset_id: presetId,
                expected_current_revision: currentRevision,
                target_revision: targetRevision,
                approval_id: approvalId,
                expected_review_sha256: review.review_sha256,
            });
            if (
                this.destroyed ||
                contextEpoch !== this.contextEpoch ||
                operationEpoch !== this.operationEpoch
            ) {
                return null;
            }
            assertReceipt(
                presetId,
                currentRevision,
                targetRevision,
                review.review_sha256,
                approvalId,
                receipt,
            );
            this.mutable.update((current) => ({
                ...current,
                phase: 'ready',
                current_revision: receipt.applied_revision,
                diff: null,
                review: null,
                receipt,
                error: null,
                announcement: t('preset_history.notice.applied', {
                    source: targetRevision,
                    target: receipt.applied_revision,
                }),
            }));
        } catch (error: unknown) {
            if (
                this.destroyed ||
                contextEpoch !== this.contextEpoch ||
                operationEpoch !== this.operationEpoch
            ) {
                return null;
            }
            this.mutable.update((current) => ({
                ...current,
                phase: 'error',
                receipt: null,
                error: errorLabel(error),
                announcement: '',
            }));
            return null;
        }

        const list = this.client.listPromptPresetRevisions;
        if (list !== undefined) {
            try {
                const refreshed = await list.call(this.client, {
                    prompt_preset_id: presetId,
                    limit: MAX_VISIBLE_PROMPT_PRESET_REVISIONS,
                });
                if (this.operationIsStale(contextEpoch, operationEpoch)) {
                    return receipt;
                }
                assertHistoryList(refreshed);
                assertHistoryContainsCurrent(refreshed, receipt.applied_revision);
                this.mutable.update((current) => ({
                    ...current,
                    revisions: refreshed.revisions,
                    truncated: refreshed.truncated,
                }));
            } catch {
                if (!this.operationIsStale(contextEpoch, operationEpoch)) {
                    this.mutable.update((current) => ({
                        ...current,
                        error: t('preset_history.error.reload_failed'),
                    }));
                }
            }
        }
        return receipt;
    }
}
