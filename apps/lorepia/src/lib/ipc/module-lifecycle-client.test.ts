import { describe, expect, it } from 'vitest';

import type {
    ActivateContentModuleInput,
    ApplyContentModuleRollbackInput,
    DeactivateContentModuleInput,
    ResolveContentModuleActivationInput,
    ResolveContentModuleRollbackInput,
} from '../../features/orchestration/module-lifecycle-contracts';
import { LiveLorepiaClient, type LorepiaTransport } from './client';
import type { ChatStreamItemDto } from './contracts';

class RecordingTransport implements LorepiaTransport {
    readonly calls: {
        commandName: string;
        args: Record<string, unknown> | undefined;
    }[] = [];

    invoke(commandName: string, args?: Record<string, unknown>): Promise<unknown> {
        this.calls.push({ commandName, args });
        return Promise.resolve(undefined);
    }

    createChatChannel(onMessage: (message: ChatStreamItemDto) => void): unknown {
        void onMessage;
        return {};
    }

    listen(eventName: string, onPayload: (payload: unknown) => void): Promise<() => void> {
        void eventName;
        void onPayload;
        return Promise.resolve(() => undefined);
    }
}

const activation = {
    runtime_target: {
        conversation_id: 'conversation-1',
        branch_id: 'branch-1',
    },
    expected_binding_revision: 7,
    binding: {
        id: 'binding-1',
        module_id: 'module-1',
        scope: 'branch' as const,
        target_id: 'branch-1',
        conversation_id: 'conversation-1',
        priority: 1,
        resolution_mode: 'pinned' as const,
        pinned_revision_id: 'revision-2',
        package_import_approval_id: 'package-approval-1',
        variable_overrides: { values: [] },
    },
};

const resolutions = {
    expected_review_sha256: 'a'.repeat(64),
    resolutions: [
        {
            component: { kind: 'transform_set' as const, id: 'transform-1' },
            expected_candidates: [
                {
                    module_id: 'module-1',
                    revision_id: 'revision-2',
                    component_hash: 'b'.repeat(64),
                },
            ],
            selected: null,
        },
    ],
};

describe('content-module lifecycle client boundary', () => {
    it('maps every safe lifecycle call to one high-level Tauri command without paths or bytes', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);
        const resolveActivation: ResolveContentModuleActivationInput = {
            activation,
            resolutions,
        };
        const activate: ActivateContentModuleInput = {
            ...resolveActivation,
            approval: {
                approval_id: 'activation-approval-1',
                expected_review_sha256: 'a'.repeat(64),
                expected_plan_sha256: 'c'.repeat(64),
            },
        };
        const resolveRollback: ResolveContentModuleRollbackInput = {
            runtime_target: activation.runtime_target,
            binding_id: 'binding-1',
            target_revision_id: 'revision-1',
            target_package_import_approval_id: 'package-approval-target-1',
            expected_state_revision: 13,
            expected_rollback_review_sha256: 'd'.repeat(64),
            resolutions,
        };
        const applyRollback: ApplyContentModuleRollbackInput = {
            resolution: resolveRollback,
            expected_rollback_plan_sha256: 'e'.repeat(64),
            activation_approval: activate.approval,
        };
        const deactivation = {
            runtime_target: activation.runtime_target,
            binding_id: 'binding-1',
        };
        const deactivate: DeactivateContentModuleInput = {
            deactivation,
            expected_review_sha256: 'f'.repeat(64),
        };

        await client.listContentModuleLifecycleCandidates({
            runtime_target: activation.runtime_target,
            limit: 100,
        });
        await client.listContentModuleLifecycleBindings({
            runtime_target: activation.runtime_target,
            limit: 100,
        });
        await client.reviewContentModuleActivation({ activation });
        await client.resolveContentModuleActivation(resolveActivation);
        await client.activateContentModule(activate);
        await client.reviewContentModuleDeactivation({ deactivation });
        await client.deactivateContentModule(deactivate);
        await client.reviewContentModuleRollback({
            runtime_target: activation.runtime_target,
            binding_id: 'binding-1',
            target_revision_id: 'revision-1',
            target_package_import_approval_id: 'package-approval-target-1',
        });
        await client.resolveContentModuleRollback(resolveRollback);
        await client.applyContentModuleRollback(applyRollback);

        expect(transport.calls).toEqual([
            {
                commandName: 'list_content_module_lifecycle_candidates',
                args: {
                    request: {
                        runtime_target: activation.runtime_target,
                        limit: 100,
                    },
                },
            },
            {
                commandName: 'list_content_module_lifecycle_bindings',
                args: {
                    request: {
                        runtime_target: activation.runtime_target,
                        limit: 100,
                    },
                },
            },
            {
                commandName: 'review_content_module_activation',
                args: { request: { activation } },
            },
            {
                commandName: 'resolve_content_module_activation',
                args: { request: resolveActivation },
            },
            {
                commandName: 'activate_content_module',
                args: { request: activate },
            },
            {
                commandName: 'review_content_module_deactivation',
                args: { request: { deactivation } },
            },
            {
                commandName: 'deactivate_content_module',
                args: { request: deactivate },
            },
            {
                commandName: 'review_content_module_rollback',
                args: {
                    request: {
                        runtime_target: activation.runtime_target,
                        binding_id: 'binding-1',
                        target_revision_id: 'revision-1',
                        target_package_import_approval_id: 'package-approval-target-1',
                    },
                },
            },
            {
                commandName: 'resolve_content_module_rollback',
                args: { request: resolveRollback },
            },
            {
                commandName: 'apply_content_module_rollback',
                args: { request: applyRollback },
            },
        ]);
        const serialized = JSON.stringify(transport.calls);
        expect(serialized).not.toContain('/Users/');
        expect(serialized).not.toContain('"path"');
        expect(serialized).not.toContain('"bytes"');
        expect(serialized).not.toContain('"content"');
    });
});
