import { get, writable } from 'svelte/store';
import { t } from '../../lib/i18n';
import { LorepiaClientError, normalizeClientError } from '../../lib/ipc/errors';
import type { ContentModuleActivationState } from './module-lifecycle-controller';
import {
    contentModuleComponentKey,
    contentModuleCandidateKey,
} from './module-lifecycle-controller';
import type {
    ContentModuleConflictDto,
    ContentModuleResolutionSetInput,
} from './module-lifecycle-contracts';
import type {
    ContentModulePagedClientApi,
    ModuleReviewPage,
    ModulePlanSummary,
    ModuleReceiptSummary,
} from './module-activation-pages';

interface State {
    busy: boolean;
    page: ModuleReviewPage | null;
    plan: ModulePlanSummary | null;
    receipt: ModuleReceiptSummary | null;
    choices: Record<string, string>;
    error: string | null;
}

/** Owns one immutable activation draft and ignores all results after disposal. */
export class ModuleActivationController {
    private readonly mutable = writable<State>({
        busy: false,
        page: null,
        plan: null,
        receipt: null,
        choices: {},
        error: null,
    });
    readonly state = { subscribe: this.mutable.subscribe };
    private epoch = 0;
    private disposed = false;
    private approvalId: string | null = null;
    private readonly draft: ContentModuleActivationState;

    constructor(
        private readonly client: ContentModulePagedClientApi,
        draft: ContentModuleActivationState,
    ) {
        this.draft = structuredClone(draft);
    }

    destroy() {
        this.disposed = true;
        this.epoch++;
    }

    private async run(work: (current: () => boolean) => Promise<void>): Promise<boolean> {
        if (this.disposed || get(this.mutable).busy) return false;
        const epoch = ++this.epoch;
        const current = () => !this.disposed && epoch === this.epoch;
        this.mutable.update((s) => ({ ...s, busy: true, error: null }));
        try {
            await work(current);
            return current();
        } catch (error) {
            if (current()) {
                const normalized = normalizeClientError(error);
                this.mutable.update((s) => ({
                    ...s,
                    error:
                        normalized.messageKey === 'error.module_plan_too_large'
                            ? t('error.module_plan_too_large')
                            : normalized.code === 'invalid_input'
                              ? t('importSetup.moduleReviewChanged')
                              : t('module_lifecycle.error.generic'),
                }));
            }
            return false;
        } finally {
            if (current()) this.mutable.update((s) => ({ ...s, busy: false }));
        }
    }

    private invalid(): never {
        throw new LorepiaClientError({
            code: 'invalid_input',
            message_key: 'error.invalid_input',
            recoverable: false,
            operation_id: null,
            field_errors: [],
        });
    }

    review(offset = 0) {
        return this.run(async (current) => {
            const previous = get(this.mutable).page;
            const page = await this.client.reviewContentModuleActivationPage({
                activation: this.draft.request,
                offset,
                expected_review_sha256: offset === 0 ? null : (previous?.review_sha256 ?? null),
            });
            if (!current()) return;
            const { candidate, request } = this.draft;
            const metadata = page.proposed_revision;
            if (
                metadata.module_id !== candidate.module_id ||
                metadata.revision_id !== candidate.revision_id ||
                metadata.revision_source_sha256 !== candidate.revision_source_sha256 ||
                !metadata.local_use_allowed ||
                page.activation_binding_id !== request.binding.id ||
                !/^[a-f0-9]{64}$/.test(page.review_sha256) ||
                !Number.isSafeInteger(page.state_revision) ||
                page.offset !== offset ||
                page.components.length > 64 ||
                page.conflicts.length > 512 ||
                !Number.isSafeInteger(page.component_count) ||
                page.component_count < offset + page.components.length ||
                (page.next_offset !== null &&
                    (page.next_offset !== offset + page.components.length ||
                        page.next_offset >= page.component_count)) ||
                (page.next_offset === null &&
                    offset + page.components.length !== page.component_count) ||
                (offset !== 0 && page.review_sha256 !== previous?.review_sha256)
            )
                this.invalid();
            if (candidate.source_kind === 'imported_package') {
                const selected = candidate.completed_package_approvals.find(
                    (a) => a.approval_id === request.binding.package_import_approval_id,
                );
                const returned = page.package_approval;
                if (
                    !selected ||
                    !returned ||
                    Object.entries(selected).some(
                        ([key, value]) => returned[key as keyof typeof returned] !== value,
                    )
                )
                    this.invalid();
            } else if (page.package_approval !== null) this.invalid();
            if (offset === 0) this.approvalId = null;
            this.mutable.update((s) => ({
                ...s,
                page,
                ...(offset === 0 ? { plan: null, receipt: null, choices: {} } : {}),
            }));
        });
    }

    choose(conflict: ContentModuleConflictDto, choice: string) {
        const state = get(this.mutable);
        const key = contentModuleComponentKey(conflict.component);
        if (
            state.busy ||
            state.receipt ||
            !state.page?.conflicts.some((c) => contentModuleComponentKey(c.component) === key) ||
            (choice !== 'omit' &&
                !conflict.candidates.some((c) => contentModuleCandidateKey(c) === choice))
        )
            return;
        this.approvalId = null;
        this.mutable.update((s) => ({
            ...s,
            plan: null,
            choices: { ...s.choices, [key]: choice },
        }));
    }

    private resolutions(): ContentModuleResolutionSetInput {
        const state = get(this.mutable);
        if (!state.page) this.invalid();
        return {
            expected_review_sha256: state.page.review_sha256,
            resolutions: state.page.conflicts.map((conflict) => {
                const choice = state.choices[contentModuleComponentKey(conflict.component)];
                const selected = conflict.candidates.find(
                    (c) => contentModuleCandidateKey(c) === choice,
                );
                if (!selected && choice !== 'omit') this.invalid();
                return {
                    component: conflict.component,
                    expected_candidates: conflict.candidates,
                    selected: selected ?? null,
                };
            }),
        };
    }

    resolve() {
        return this.run(async (current) => {
            const page = get(this.mutable).page;
            if (!page) this.invalid();
            const plan = await this.client.resolveContentModuleActivationSummary({
                activation: this.draft.request,
                resolutions: this.resolutions(),
            });
            if (!current()) return;
            if (
                plan.review_sha256 !== page.review_sha256 ||
                plan.expected_state_revision !== page.state_revision ||
                plan.activation_binding_id !== this.draft.request.binding.id ||
                !/^[a-f0-9]{64}$/.test(plan.plan_sha256) ||
                plan.component_count + plan.omitted_component_count !== page.component_count ||
                plan.runtime_enabled_count > plan.component_count ||
                [
                    plan.component_count,
                    plan.runtime_enabled_count,
                    plan.omitted_component_count,
                ].some((count) => !Number.isSafeInteger(count) || count < 0)
            )
                this.invalid();
            this.approvalId = globalThis.crypto.randomUUID();
            this.mutable.update((s) => ({ ...s, plan }));
        });
    }

    activate() {
        return this.run(async (current) => {
            const { plan, receipt: completed } = get(this.mutable);
            if (!plan || !this.approvalId || completed) this.invalid();
            // Keep this id on unknown transport outcomes so retry is idempotent.
            const receipt = await this.client.activateContentModuleSummary({
                activation: this.draft.request,
                resolutions: this.resolutions(),
                approval: {
                    approval_id: this.approvalId,
                    expected_review_sha256: plan.review_sha256,
                    expected_plan_sha256: plan.plan_sha256,
                },
            });
            if (!current()) return;
            const binding = receipt.binding.binding;
            const requested = this.draft.request.binding;
            if (
                !receipt.verified ||
                receipt.approval_id !== this.approvalId ||
                Object.entries(plan).some(
                    ([key, value]) => receipt.plan[key as keyof ModulePlanSummary] !== value,
                ) ||
                !/^[a-f0-9]{64}$/.test(receipt.approval_sha256) ||
                binding.id !== requested.id ||
                binding.module_id !== requested.module_id ||
                binding.revision_id !== this.draft.candidate.revision_id ||
                !binding.enabled ||
                !binding.approved ||
                binding.activation_approval_id !== this.approvalId ||
                binding.activation_review_sha256 !== plan.review_sha256 ||
                binding.activation_plan_sha256 !== plan.plan_sha256 ||
                binding.package_import_approval_id !== requested.package_import_approval_id ||
                binding.scope !== requested.scope ||
                binding.target_id !== requested.target_id ||
                binding.conversation_id !== requested.conversation_id ||
                binding.priority !== requested.priority ||
                binding.resolution_mode !== requested.resolution_mode ||
                binding.pinned_revision_id !== requested.pinned_revision_id ||
                JSON.stringify(binding.variable_overrides) !==
                    JSON.stringify(requested.variable_overrides) ||
                !Number.isSafeInteger(receipt.binding.state_revision) ||
                receipt.binding.state_revision !== plan.expected_state_revision + 1
            )
                this.invalid();
            this.mutable.update((s) => ({ ...s, receipt }));
        });
    }
}
