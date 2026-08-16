import { describe, expect, it } from 'vitest';

import rustInvokeHandler from '../../../src-tauri/src/lib.rs?raw';
import type { ChatStreamItemDto } from './contracts';
import {
    LOREPIA_COMMANDS,
    LOREPIA_EVENTS,
    LiveLorepiaClient,
    type LorepiaTransport,
} from './client';

class RecordingTransport implements LorepiaTransport {
    readonly calls: {
        commandName: string;
        args: Record<string, unknown> | undefined;
    }[] = [];
    channelListener: ((item: ChatStreamItemDto) => void) | null = null;
    readonly eventListeners = new Map<string, (payload: unknown) => void>();
    readonly responses = new Map<string, unknown>();

    invoke(commandName: string, args?: Record<string, unknown>): Promise<unknown> {
        this.calls.push({ commandName, args });
        if (this.responses.has(commandName)) {
            return Promise.resolve(this.responses.get(commandName));
        }
        if (commandName === LOREPIA_COMMANDS.sendMessage) {
            return Promise.resolve({ generation_id: 'generation-1' });
        }
        if (commandName === LOREPIA_COMMANDS.disposeChatStream) {
            return Promise.resolve(true);
        }
        return Promise.resolve(undefined);
    }

    createChatChannel(onMessage: (message: ChatStreamItemDto) => void): unknown {
        this.channelListener = onMessage;
        return { kind: 'test-channel' };
    }

    listen(eventName: string, onPayload: (payload: unknown) => void): Promise<() => void> {
        this.eventListeners.set(eventName, onPayload);
        return Promise.resolve(() => {
            this.eventListeners.delete(eventName);
        });
    }

    emitEvent(eventName: string, payload: unknown): void {
        this.eventListeners.get(eventName)?.(payload);
    }
}

describe('LiveLorepiaClient transport boundary', () => {
    it('sends only greeting identity and exact character-content revision selectors', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);

        await client.getCharacterGreetingCatalog('character-1');
        await client.createConversation('character-1', '새 대화', 'chat', {
            character_content_revision_id: 'character-revision-7',
            greeting_id: 'greeting-default',
        });
        await client.openExistingConversation('conversation-1');

        expect(transport.calls).toEqual([
            {
                commandName: 'get_character_greeting_catalog',
                args: { request: { character_id: 'character-1' } },
            },
            {
                commandName: 'create_conversation',
                args: {
                    input: {
                        character_id: 'character-1',
                        title: '새 대화',
                        mode: 'chat',
                        greeting: {
                            character_content_revision_id: 'character-revision-7',
                            greeting_id: 'greeting-default',
                        },
                    },
                },
            },
            {
                commandName: 'open_existing_conversation',
                args: { request: { conversation_id: 'conversation-1' } },
            },
        ]);
        expect(JSON.stringify(transport.calls)).not.toContain('"greeting_text"');
        expect(JSON.stringify(transport.calls)).not.toContain('"content":');
    });

    it('reads and subscribes to the bounded memory supervisor status surface', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);
        const received: unknown[] = [];

        await client.getMemorySupervisorStatus();
        const unlisten = await client.subscribeMemorySupervisorStatus((status) => {
            received.push(status);
        });
        transport.emitEvent(LOREPIA_EVENTS.memorySupervisorStatus, {
            sequence: 2,
            phase: 'running',
            recovered_interrupted_jobs: 0,
            completed_jobs: 1,
            job_id: 'must-not-cross',
        });
        const status = {
            sequence: 3,
            phase: 'running' as const,
            recovered_interrupted_jobs: 1,
            completed_jobs: 2,
        };
        transport.emitEvent(LOREPIA_EVENTS.memorySupervisorStatus, status);

        expect(transport.calls).toEqual([
            { commandName: 'get_memory_supervisor_status', args: undefined },
        ]);
        expect(received).toEqual([status]);
        expect(JSON.stringify(received)).not.toContain('job_id');
        expect(JSON.stringify(received)).not.toContain('payload');
        expect(JSON.stringify(received)).not.toContain('error');

        unlisten();
        expect(transport.eventListeners.has(LOREPIA_EVENTS.memorySupervisorStatus)).toBe(false);
    });

    it('uses opaque interaction delivery IDs and filters malformed effect events', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);
        const received: unknown[] = [];

        await client.listInteractionEffects();
        await client.acknowledgeInteractionEffect('delivery-1');
        await client.retryInteractionEffect('delivery-2');
        await client.decideInteractionProposal({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            proposal_record_id: 'proposal-1',
            expected_state_revision: 7,
            expected_proposal_revision: 3,
            decision: 'approve',
        });
        await client.listInteractionProposals({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            status: 'pending',
            limit: 100,
        });
        await client.expireInteractionProposals({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            limit: 100,
        });
        await client.listInteractionEffectHistory({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            after: null,
            limit: 100,
        });
        await client.listReopenInteractionEffects({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            limit: 100,
        });
        await client.submitInteractionChoice({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            effect_id: 'effect-choice-1',
            choice_id: 'choice-1',
            expected_state_revision: 7,
        });
        const unlisten = await client.subscribeInteractionEffects((event) => {
            received.push(event);
        });
        transport.emitEvent(LOREPIA_EVENTS.interactionEffect, {
            delivery_id: 'delivery-unsafe',
            effect_id: 'effect-unsafe',
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            resulting_state_revision: 2,
            event_created_at: '2026-08-03T00:00:00Z',
            effect: {
                kind: 'run_script',
                path: '/Users/synthetic/private.js',
            },
        });
        transport.emitEvent(LOREPIA_EVENTS.interactionEffect, {
            delivery_id: 'delivery-unsafe-asset',
            effect_id: 'effect-unsafe-asset',
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            resulting_state_revision: 2,
            event_created_at: '2026-08-03T00:00:00Z',
            effect: {
                kind: 'show_asset',
                region: 'message',
                asset: {
                    asset_id: 'asset-unsafe',
                    sha256: 'ab'.repeat(32),
                    media_type: 'image/png',
                    kind: 'image',
                    size_bytes: 10,
                    width: 1,
                    height: 1,
                    duration_ms: null,
                    url: `lorepia-asset://sha256/${'ab'.repeat(32)}`,
                    path: '/Users/synthetic/private.png',
                },
            },
        });
        const safeEvent = {
            delivery_id: 'delivery-3',
            effect_id: 'effect-3',
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            resulting_state_revision: 3,
            event_created_at: '2026-08-03T00:00:01Z',
            effect: {
                kind: 'present_choices' as const,
                choices: [{ id: 'choice-1', label: '합성 선택지' }],
            },
        };
        transport.emitEvent(LOREPIA_EVENTS.interactionEffect, safeEvent);
        const rejectedProjection = {
            delivery_id: 'delivery-rejected',
            effect_id: 'effect-rejected',
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            resulting_state_revision: 4,
            event_created_at: '2026-08-03T00:00:02Z',
            effect: {
                kind: 'projection_rejected' as const,
                reason: 'unsafe_native_text' as const,
            },
        };
        transport.emitEvent(LOREPIA_EVENTS.interactionEffect, rejectedProjection);

        expect(transport.calls).toEqual([
            { commandName: 'list_interaction_effects', args: undefined },
            {
                commandName: 'acknowledge_interaction_effect',
                args: { request: { delivery_id: 'delivery-1' } },
            },
            {
                commandName: 'retry_interaction_effect',
                args: { request: { delivery_id: 'delivery-2' } },
            },
            {
                commandName: 'decide_interaction_proposal',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        proposal_record_id: 'proposal-1',
                        expected_state_revision: 7,
                        expected_proposal_revision: 3,
                        decision: 'approve',
                    },
                },
            },
            {
                commandName: 'list_interaction_proposals',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        status: 'pending',
                        limit: 100,
                    },
                },
            },
            {
                commandName: 'expire_interaction_proposals',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        limit: 100,
                    },
                },
            },
            {
                commandName: 'list_interaction_effect_history',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        after: null,
                        limit: 100,
                    },
                },
            },
            {
                commandName: 'list_reopen_interaction_effects',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        limit: 100,
                    },
                },
            },
            {
                commandName: 'submit_interaction_choice',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        effect_id: 'effect-choice-1',
                        choice_id: 'choice-1',
                        expected_state_revision: 7,
                    },
                },
            },
        ]);
        expect(received).toEqual([safeEvent, rejectedProjection]);
        expect(JSON.stringify(received)).not.toContain('/Users/');
        expect(JSON.stringify(transport.calls)).not.toContain('action_args');
        expect(JSON.stringify(transport.calls)).not.toContain('rule');

        unlisten();
        expect(transport.eventListeners.has(LOREPIA_EVENTS.interactionEffect)).toBe(false);
    });

    it('echoes attempt-owned proposal authority and u64 CAS tokens without numeric coercion', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);
        const maximumRevision = '18446744073709551615';

        await client.expireGenerationAttemptProposals({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            limit: 100,
        });
        await client.listGenerationAttemptProposals({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            status: 'pending',
            limit: 100,
        });
        await client.listRetryableGenerationAttempts({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            limit: 100,
        });
        await client.decideGenerationAttemptProposal({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            generation_id: 'generation-1',
            proposal_record_id: 'proposal-1',
            expected_aggregate_revision: maximumRevision,
            expected_proposal_revision: '3',
            decision: 'approve',
        });

        expect(transport.calls).toEqual([
            {
                commandName: 'expire_generation_attempt_proposals',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        source_branch_id: 'branch-1',
                        limit: 100,
                    },
                },
            },
            {
                commandName: 'list_generation_attempt_proposals',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        source_branch_id: 'branch-1',
                        status: 'pending',
                        limit: 100,
                    },
                },
            },
            {
                commandName: 'list_retryable_generation_attempts',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        source_branch_id: 'branch-1',
                        limit: 100,
                    },
                },
            },
            {
                commandName: 'decide_generation_attempt_proposal',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        source_branch_id: 'branch-1',
                        generation_id: 'generation-1',
                        proposal_record_id: 'proposal-1',
                        expected_aggregate_revision: maximumRevision,
                        expected_proposal_revision: '3',
                        decision: 'approve',
                    },
                },
            },
        ]);
        expect(JSON.stringify(transport.calls)).not.toContain('user_action');
        expect(JSON.stringify(transport.calls)).not.toContain('arguments');
        expect(JSON.stringify(transport.calls)).not.toContain('operation_nonce');
        expect(JSON.stringify(transport.calls)).not.toContain('provider_request');
    });

    it('uses only durable IDs and exact review hashes for content-package lifecycle calls', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);
        const reviewSha = 'a'.repeat(64);
        const packagePlanSha = 'b'.repeat(64);
        const selectionPlanSha = 'c'.repeat(64);
        const importPlanSha = 'd'.repeat(64);
        const capabilityReviewSha = 'e'.repeat(64);
        const evidenceSha = 'f'.repeat(64);
        const approvalSha = '1'.repeat(64);
        const targetReviewSha = '2'.repeat(64);

        await client.listPendingContentPackageImports({ limit: 100 });
        await client.pickContentPackageImport();
        await client.reopenContentPackageImport({ import_id: 'import-1' });
        await client.selectContentPackageImport({
            import_id: 'import-1',
            expected_revision: 7,
            expected_package_plan_hash: packagePlanSha,
            expected_review_sha256: reviewSha,
            expected_capability_review_sha256: capabilityReviewSha,
            selected_component_ids: ['component-a'],
        });
        await client.approveContentPackageImport({
            import_id: 'import-1',
            expected_revision: 8,
            expected_package_plan_hash: packagePlanSha,
            expected_content_selection_plan_hash: selectionPlanSha,
            expected_review_sha256: reviewSha,
            expected_import_plan_sha256: importPlanSha,
            expected_capability_review_sha256: capabilityReviewSha,
            expected_normalization_evidence_sha256: evidenceSha,
            expected_target_review_sha256: targetReviewSha,
            confirmed_update_targets: [
                {
                    source_component_id: 'component-a',
                    component_document_ordinal: 0,
                    target_object_id: 'prompt-1',
                    expected_target_revision_id: 'prompt-revision-2',
                    expected_target_state_revision: 9,
                },
            ],
            approval_id: 'approval-1',
            enable_component_ids: [],
            approved_capabilities: ['transforms'],
        });
        await client.commitContentPackageImport({
            import_id: 'import-1',
            expected_revision: 9,
            expected_package_plan_hash: packagePlanSha,
            expected_content_selection_plan_hash: selectionPlanSha,
            expected_review_sha256: reviewSha,
            expected_import_plan_sha256: importPlanSha,
            expected_approval_sha256: approvalSha,
            expected_capability_review_sha256: capabilityReviewSha,
            expected_normalization_evidence_sha256: evidenceSha,
        });
        await client.discardContentPackageImport({
            import_id: 'import-2',
            expected_revision: 3,
            expected_review_sha256: reviewSha,
            expected_import_plan_sha256: null,
            expected_capability_review_sha256: capabilityReviewSha,
        });

        expect(transport.calls).toEqual([
            {
                commandName: 'list_pending_content_package_imports',
                args: { request: { limit: 100 } },
            },
            { commandName: 'pick_content_package_import', args: undefined },
            {
                commandName: 'reopen_content_package_import',
                args: { request: { import_id: 'import-1' } },
            },
            {
                commandName: 'select_content_package_import',
                args: {
                    request: {
                        import_id: 'import-1',
                        expected_revision: 7,
                        expected_package_plan_hash: packagePlanSha,
                        expected_review_sha256: reviewSha,
                        expected_capability_review_sha256: capabilityReviewSha,
                        selected_component_ids: ['component-a'],
                    },
                },
            },
            {
                commandName: 'approve_content_package_import',
                args: {
                    request: {
                        import_id: 'import-1',
                        expected_revision: 8,
                        expected_package_plan_hash: packagePlanSha,
                        expected_content_selection_plan_hash: selectionPlanSha,
                        expected_review_sha256: reviewSha,
                        expected_import_plan_sha256: importPlanSha,
                        expected_capability_review_sha256: capabilityReviewSha,
                        expected_normalization_evidence_sha256: evidenceSha,
                        expected_target_review_sha256: targetReviewSha,
                        confirmed_update_targets: [
                            {
                                source_component_id: 'component-a',
                                component_document_ordinal: 0,
                                target_object_id: 'prompt-1',
                                expected_target_revision_id: 'prompt-revision-2',
                                expected_target_state_revision: 9,
                            },
                        ],
                        approval_id: 'approval-1',
                        enable_component_ids: [],
                        approved_capabilities: ['transforms'],
                    },
                },
            },
            {
                commandName: 'commit_content_package_import',
                args: {
                    request: {
                        import_id: 'import-1',
                        expected_revision: 9,
                        expected_package_plan_hash: packagePlanSha,
                        expected_content_selection_plan_hash: selectionPlanSha,
                        expected_review_sha256: reviewSha,
                        expected_import_plan_sha256: importPlanSha,
                        expected_approval_sha256: approvalSha,
                        expected_capability_review_sha256: capabilityReviewSha,
                        expected_normalization_evidence_sha256: evidenceSha,
                    },
                },
            },
            {
                commandName: 'discard_content_package_import',
                args: {
                    request: {
                        import_id: 'import-2',
                        expected_revision: 3,
                        expected_review_sha256: reviewSha,
                        expected_import_plan_sha256: null,
                        expected_capability_review_sha256: capabilityReviewSha,
                    },
                },
            },
        ]);
        expect(JSON.stringify(transport.calls)).not.toContain('/Users/');
        expect(JSON.stringify(transport.calls)).not.toContain('path');
        expect(JSON.stringify(transport.calls)).not.toContain('bytes');
    });

    it('exports content sources through a selector-only command and returns only safe delivery evidence', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);
        const completedDescriptors = [
            {
                kind: 'lorepia_package' as const,
                source_id: 'import-1',
                sha256: 'b'.repeat(64),
                size_bytes: 8_192,
                suggested_file_name: 'package.synthetic.lorepia.zip',
            },
        ];
        const receipt = {
            kind: 'character_card_v3' as const,
            source_id: 'character-1',
            sha256: 'a'.repeat(64),
            size_bytes: 4_096,
            file_name: 'character-1.card.json',
        };
        transport.responses.set(
            LOREPIA_COMMANDS.listCompletedContentPackageExports,
            completedDescriptors,
        );
        transport.responses.set(LOREPIA_COMMANDS.exportContentSource, receipt);

        await expect(client.listCompletedContentPackageExports({ limit: 100 })).resolves.toEqual(
            completedDescriptors,
        );

        await expect(
            client.exportContentSource({
                kind: 'character_source',
                character_id: 'character-1',
            }),
        ).resolves.toEqual(receipt);

        transport.responses.set(LOREPIA_COMMANDS.exportContentSource, null);
        await expect(
            client.exportContentSource({
                kind: 'content_package',
                import_id: 'import-1',
            }),
        ).resolves.toBeNull();
        expect(transport.calls).toEqual([
            {
                commandName: 'list_completed_content_package_exports',
                args: { request: { limit: 100 } },
            },
            {
                commandName: 'export_content_source',
                args: {
                    request: {
                        kind: 'character_source',
                        character_id: 'character-1',
                    },
                },
            },
            {
                commandName: 'export_content_source',
                args: {
                    request: {
                        kind: 'content_package',
                        import_id: 'import-1',
                    },
                },
            },
        ]);
        expect(JSON.stringify(transport.calls)).not.toContain('path');
        expect(JSON.stringify(transport.calls)).not.toContain('bytes');
    });

    it('never places credentials or cURL source text in WebView invoke arguments', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);
        const connectionInput = {
            id: 'connection-2',
            template_id: 'template-1',
            template_version: 1,
            display_name: 'Synthetic',
            api_origin: 'https://example.test',
            api_base_path: null,
            network_mode: 'public' as const,
            local_network_approval: null,
            values: [],
            approved_credential_origin: 'https://example.test',
            timeout_seconds: 30,
        };
        const discoveryInput = {
            connection_id: 'connection-3',
            display_name: 'Captured cURL',
            docs_url: null,
            credential_binding_requested: false,
            preferred_assistant: null,
            connection_options: {
                values: [],
                api_base_path: null,
                timeout_seconds: 30,
                network_mode: 'public' as const,
                local_network_approval: null,
            },
            supplied_evidence_ids: [],
        };

        await client.captureCredential({
            kind: 'connection',
            connection_id: 'connection-1',
        });
        await client.credentialStatus({
            kind: 'discovery_session',
            session_id: 'discovery-1',
            expected_revision: 7,
        });
        await client.captureCredential({
            kind: 'discovery_session',
            session_id: 'discovery-1',
            expected_revision: 7,
        });
        await client.createProviderConnection(connectionInput);
        await client.beginProviderDiscoveryCurl(discoveryInput);
        await client.supplyProviderDiscoveryCurlEvidence('discovery-1', 3);
        await client.commitProviderDiscovery('discovery-1');

        expect(transport.calls).toEqual([
            {
                commandName: 'capture_credential',
                args: {
                    request: {
                        target: { kind: 'connection', connection_id: 'connection-1' },
                    },
                },
            },
            {
                commandName: 'credential_status',
                args: {
                    request: {
                        target: {
                            kind: 'discovery_session',
                            session_id: 'discovery-1',
                            expected_revision: 7,
                        },
                    },
                },
            },
            {
                commandName: 'capture_credential',
                args: {
                    request: {
                        target: {
                            kind: 'discovery_session',
                            session_id: 'discovery-1',
                            expected_revision: 7,
                        },
                    },
                },
            },
            {
                commandName: 'create_provider_connection',
                args: {
                    request: {
                        input: connectionInput,
                    },
                },
            },
            {
                commandName: 'begin_provider_discovery_curl',
                args: {
                    request: {
                        input: discoveryInput,
                    },
                },
            },
            {
                commandName: 'supply_provider_discovery_curl_evidence',
                args: {
                    request: {
                        session_id: 'discovery-1',
                        expected_revision: 3,
                    },
                },
            },
            {
                commandName: 'commit_provider_discovery',
                args: { request: { session_id: 'discovery-1' } },
            },
        ]);
        const serializedArgs = JSON.stringify(transport.calls.map((call) => call.args));
        expect(serializedArgs).not.toContain('"credential":');
        expect(serializedArgs).not.toContain('"curl":');
        expect(serializedArgs).not.toContain('secret');
        expect(rustInvokeHandler).not.toContain('commands::set_credential');
    });

    it('polls one discovery session without changing the existing global outbox API', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);

        await client.pollProviderDiscoveryEvents(100);
        await client.pollProviderDiscoveryEventsForSession('discovery-1', 100);

        expect(transport.calls).toEqual([
            {
                commandName: 'poll_provider_discovery_events',
                args: { request: { limit: 100 } },
            },
            {
                commandName: 'poll_provider_discovery_events_for_session',
                args: {
                    request: {
                        session_id: 'discovery-1',
                        limit: 100,
                    },
                },
            },
        ]);
    });

    it('uses the exact Rust request/input wrappers and camelCase channel argument', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);

        await client.getCharacter('character-1');
        await client.resolveAssetDelivery({
            selector: { kind: 'asset_id', asset_id: 'asset-avatar-1' },
        });
        await client.inspectImport('ticket-1');
        await client.commitImport('inspection-1');
        await client.discardImport('inspection-1');
        await client.createConversation('character-1', '새 대화', 'story');
        await client.getConversation('conversation-1');
        await client.selectBranch('conversation-1', 'branch-1');
        await client.listBranchMessages('branch-1');
        await client.listMessages('conversation-1');
        await client.createBranch('conversation-1', 'message-1', null);
        await client.editUserMessage(
            {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                expected_head: 'message-2',
                message_id: 'message-1',
                replacement_text: '고친 문장',
                selection: {
                    kind: 'target',
                    target: {
                        model_route_id: 'route-1',
                        generation_preset_id: 'preset-1',
                    },
                },
                generation_attempt_id: 'generation-attempt-edit-1',
            },
            'stream-edit-1',
            () => undefined,
        );
        await client.regenerateAssistantMessage(
            {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                expected_head: 'message-2',
                message_id: 'message-2',
                selection: {
                    kind: 'target',
                    target: {
                        model_route_id: 'route-1',
                        generation_preset_id: 'preset-1',
                    },
                },
                operation_nonce: 'nonce-regenerate-1',
            },
            'stream-regenerate-1',
            () => undefined,
        );
        await client.removeMessageFromBranch({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            expected_head: 'message-2',
            message_id: 'message-1',
        });
        await client.sendMessage(
            {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                expected_head: null,
                mode: 'chat',
                text: '안녕',
                selection: {
                    kind: 'target',
                    target: {
                        model_route_id: 'route-1',
                        generation_preset_id: 'preset-1',
                    },
                },
                operation_nonce: 'nonce-send-1',
            },
            'stream-send-1',
            () => undefined,
        );
        await client.sendReviewedPrompt(
            {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                expected_head: null,
                user_text: '검토한 안녕',
                generation_target: {
                    model_route_id: 'route-1',
                    generation_preset_id: 'preset-1',
                },
                prompt_preset_id: 'prompt-1',
                variable_overrides: { values: [] },
                expected_plan_hash: 'a'.repeat(64),
                generation_attempt_id: 'generation-attempt-1',
            },
            'stream-reviewed-1',
            () => undefined,
        );
        await client.subscribeGeneration(
            'generation-1',
            'conversation-1',
            'branch-1',
            7,
            'stream-subscribe-1',
            () => undefined,
        );
        await expect(client.disposeChatStream('stream-subscribe-1')).resolves.toBe(true);

        expect(transport.calls).toEqual([
            {
                commandName: 'get_character',
                args: { request: { character_id: 'character-1' } },
            },
            {
                commandName: 'resolve_asset_delivery',
                args: {
                    request: {
                        selector: { kind: 'asset_id', asset_id: 'asset-avatar-1' },
                    },
                },
            },
            {
                commandName: 'inspect_import',
                args: { request: { ticket_id: 'ticket-1' } },
            },
            {
                commandName: 'commit_import',
                args: { request: { inspection_id: 'inspection-1' } },
            },
            {
                commandName: 'discard_import',
                args: {
                    request: { kind: 'inspection', inspection_id: 'inspection-1' },
                },
            },
            {
                commandName: 'create_conversation',
                args: {
                    input: {
                        character_id: 'character-1',
                        title: '새 대화',
                        mode: 'story',
                    },
                },
            },
            {
                commandName: 'get_conversation',
                args: { request: { conversation_id: 'conversation-1' } },
            },
            {
                commandName: 'select_branch',
                args: {
                    input: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                    },
                },
            },
            {
                commandName: 'list_branch_messages',
                args: { request: { branch_id: 'branch-1' } },
            },
            {
                commandName: 'list_messages',
                args: { request: { conversation_id: 'conversation-1' } },
            },
            {
                commandName: 'create_branch',
                args: {
                    input: {
                        conversation_id: 'conversation-1',
                        from_message_id: 'message-1',
                        title: null,
                    },
                },
            },
            {
                commandName: 'edit_user_message',
                args: {
                    input: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        expected_head: 'message-2',
                        message_id: 'message-1',
                        replacement_text: '고친 문장',
                        selection: {
                            kind: 'target',
                            target: {
                                model_route_id: 'route-1',
                                generation_preset_id: 'preset-1',
                            },
                        },
                        generation_attempt_id: 'generation-attempt-edit-1',
                    },
                    streamId: 'stream-edit-1',
                    onEvent: { kind: 'test-channel' },
                },
            },
            {
                commandName: 'regenerate_assistant_message',
                args: {
                    input: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        expected_head: 'message-2',
                        message_id: 'message-2',
                        selection: {
                            kind: 'target',
                            target: {
                                model_route_id: 'route-1',
                                generation_preset_id: 'preset-1',
                            },
                        },
                        operation_nonce: 'nonce-regenerate-1',
                    },
                    streamId: 'stream-regenerate-1',
                    onEvent: { kind: 'test-channel' },
                },
            },
            {
                commandName: 'remove_message_from_branch',
                args: {
                    input: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        expected_head: 'message-2',
                        message_id: 'message-1',
                    },
                },
            },
            {
                commandName: 'send_message',
                args: {
                    input: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        expected_head: null,
                        mode: 'chat',
                        text: '안녕',
                        selection: {
                            kind: 'target',
                            target: {
                                model_route_id: 'route-1',
                                generation_preset_id: 'preset-1',
                            },
                        },
                        operation_nonce: 'nonce-send-1',
                    },
                    streamId: 'stream-send-1',
                    onEvent: { kind: 'test-channel' },
                },
            },
            {
                commandName: 'send_reviewed_prompt',
                args: {
                    input: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        expected_head: null,
                        user_text: '검토한 안녕',
                        generation_target: {
                            model_route_id: 'route-1',
                            generation_preset_id: 'preset-1',
                        },
                        prompt_preset_id: 'prompt-1',
                        variable_overrides: { values: [] },
                        expected_plan_hash: 'a'.repeat(64),
                        generation_attempt_id: 'generation-attempt-1',
                    },
                    streamId: 'stream-reviewed-1',
                    onEvent: { kind: 'test-channel' },
                },
            },
            {
                commandName: 'subscribe_generation',
                args: {
                    request: {
                        generation_id: 'generation-1',
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        sequence_baseline: 7,
                    },
                    streamId: 'stream-subscribe-1',
                    onEvent: { kind: 'test-channel' },
                },
            },
            {
                commandName: 'dispose_chat_stream',
                args: {
                    request: { stream_id: 'stream-subscribe-1' },
                },
            },
        ]);
        expect(transport.channelListener).not.toBeNull();
    });

    it('contains only commands registered by the Tauri invoke handler', () => {
        const registered = new Set(
            [...rustInvokeHandler.matchAll(/commands::([a-z_]+)/g)].map((match) => match[1]),
        );

        const clientCommands = Object.values(LOREPIA_COMMANDS);
        for (const commandName of clientCommands) {
            expect(commandName).not.toContain('plugin:');
            expect(registered.has(commandName), commandName).toBe(true);
        }
        expect([...clientCommands].sort()).toEqual([...registered].sort());
    });

    it('routes the production room workspace and full quick-settings save through bounded commands', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);
        const supportedFields = {
            prompt_preset_id: true,
            generation_preset_id: true,
            creator_values: true,
            variable_overrides: false,
            response_length: true,
            creativity: true,
            reasoning_effort: true,
            memory_enabled: true,
            knowledge_enabled: true,
        };
        const roomConfig = {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            prompt_preset_id: 'prompt-room',
            generation_preset_id: 'preset-room-b',
            response_length: 'long' as const,
            creativity: 73,
            reasoning_effort: 'high' as const,
            memory_enabled: true,
            knowledge_enabled: false,
            creator_values: { tone: 'warm' },
            variable_overrides: { values: [] },
            supported_fields: supportedFields,
        };
        const generationTarget = {
            model_route_id: 'route-room-b',
            generation_preset_id: 'preset-room-b',
        };
        transport.responses.set(LOREPIA_COMMANDS.getOrchestrationWorkspace, {
            expected_head: 'message-head',
            room_config_revision: 4,
            prompt_preset_revision: 9,
            interaction_state_revision: 2,
            generation_target: generationTarget,
            prompt_presets: [],
            room_config: roomConfig,
            prompt_blocks: [],
            creator_controls: [],
            knowledge_book_ids: [],
            memory_records: [],
        });
        transport.responses.set(LOREPIA_COMMANDS.saveRoomOrchestrationConfig, {
            room_config: roomConfig,
            revision: 5,
            generation_target: generationTarget,
        });

        await expect(
            client.getOrchestrationWorkspace('conversation-1', 'branch-1'),
        ).resolves.toMatchObject({ generation_target: generationTarget });
        await client.saveRoomOrchestrationConfig({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            prompt_preset_id: 'prompt-room',
            generation_preset_id: 'preset-room-b',
            response_length: 'long',
            creativity: 73,
            reasoning_effort: 'high',
            memory_enabled: true,
            knowledge_enabled: false,
            creator_values: { tone: 'warm' },
            variable_overrides: { values: [] },
            user_name_override: null,
            author_note: null,
            group_context: null,
            template_slots: [],
            expected_revision: 4,
        });

        expect(transport.calls).toEqual([
            {
                commandName: 'get_orchestration_workspace',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                    },
                },
            },
            {
                commandName: 'save_room_orchestration_config',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        prompt_preset_id: 'prompt-room',
                        generation_preset_id: 'preset-room-b',
                        response_length: 'long',
                        creativity: 73,
                        reasoning_effort: 'high',
                        memory_enabled: true,
                        knowledge_enabled: false,
                        creator_values: { tone: 'warm' },
                        variable_overrides: { values: [] },
                        user_name_override: null,
                        author_note: null,
                        group_context: null,
                        template_slots: [],
                        expected_revision: 4,
                    },
                },
            },
        ]);
    });

    it('uses exact request wrappers only for release-safe orchestration commands', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);
        const variables = { values: [] };
        await client.resolvePromptPreview({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            expected_head: 'message-1',
            user_text: '합성 사용자 입력',
            generation_target: {
                model_route_id: 'route-1',
                generation_preset_id: 'generation-1',
            },
            prompt_preset_id: 'prompt-1',
            variable_overrides: variables,
            expected_plan_hash: null,
            operation_nonce: 'nonce-preview-1',
        });
        await client.explainPromptPlan({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            expected_head: 'message-1',
            user_text: '합성 사용자 입력',
            generation_target: {
                model_route_id: 'route-1',
                generation_preset_id: 'generation-1',
            },
            prompt_preset_id: 'prompt-1',
            variable_overrides: variables,
            plan_hash: 'sha256:reviewed-plan',
            generation_attempt_id: 'generation-attempt-1',
        });
        const creatorPromptPreset = {
            id: 'prompt-1',
            name: '합성 프롬프트',
            schema_version: 1,
            blocks: [],
            controls: [],
            default_values: variables,
            default_generation_preset_id: null,
            memory_profile_id: null,
            knowledge_book_ids: [],
            transform_set_ids: [],
            module_ids: [],
            cache_boundaries: [],
            metadata: {
                description: '합성 프롬프트 문서',
                tags: [],
                provenance: {
                    source_kind: 'user_created' as const,
                    source_id: null,
                    source_hash: null,
                    author: null,
                    license: null,
                    imported_at: null,
                },
                created_at: '2026-08-03T00:00:00Z',
                updated_at: '2026-08-03T00:00:00Z',
                local_override_of: null,
            },
        };
        await client.upsertPromptPreset({
            value: creatorPromptPreset,
            expected_revision: 7,
        });
        await client.getPromptPreset({
            prompt_preset_id: 'prompt-1',
        });
        await client.getEditablePromptPreset({
            prompt_preset_id: 'prompt-1',
        });
        await client.listPromptPresets();
        await client.deletePromptPreset({
            prompt_preset_id: 'prompt-delete',
            expected_revision: 8,
        });
        await client.reorderPromptBlocks({
            prompt_preset_id: 'prompt-1',
            ordered_block_ids: ['block-2', 'block-1'],
            expected_revision: 7,
        });
        const taskProfile = {
            id: 'task-1',
            kind: 'memory_summary' as const,
            route_id: 'route-1',
            generation_preset_id: 'generation-1',
            fallback_route_ids: ['route-fallback'],
            embedding_dimensions: null,
            timeout_ms: 30_000,
            rate_limit: { requests: 4, per_seconds: 60 },
            concurrency_limit: 2,
        };
        await client.listTaskProfiles();
        await client.upsertTaskProfile({
            value: taskProfile,
            expected_revision: 3,
        });
        await client.deleteTaskProfile({
            task_profile_id: 'task-delete',
            expected_revision: 4,
        });
        await client.deleteMemoryRecord({
            memory_record_id: 'memory-delete',
            expected_revision: 9,
        });
        await client.getMemoryRecord({
            memory_record_id: 'memory-1',
        });
        await client.listPromptPresetBindings({
            scope: 'conversation',
            target_id: 'conversation-1',
        });
        await client.listMemoryRecords({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            include_invalidated: false,
        });
        await client.simulateKnowledgeActivation({
            knowledge_book_id: 'knowledge-1',
            sample_texts: ['합성 검사 문장'],
            manual_entry_ids: [],
            semantic_scores: [],
            variables,
            supported_capabilities: [],
            token_estimates: [],
            activation_seed: 7,
        });
        await client.listContentModuleBindings({ content_module_id: 'module-1' });
        await client.listContentModuleRevisions({ content_module_id: 'module-1' });
        await client.diffContentModuleRevisionDocuments({
            content_module_id: 'module-1',
            from_revision: 1,
            to_revision: 2,
        });
        await client.evaluateContentModuleShare({ content_module_id: 'module-1' });

        expect(transport.calls).toEqual([
            {
                commandName: 'resolve_prompt_preview',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        expected_head: 'message-1',
                        user_text: '합성 사용자 입력',
                        generation_target: {
                            model_route_id: 'route-1',
                            generation_preset_id: 'generation-1',
                        },
                        prompt_preset_id: 'prompt-1',
                        variable_overrides: variables,
                        expected_plan_hash: null,
                        operation_nonce: 'nonce-preview-1',
                    },
                },
            },
            {
                commandName: 'explain_prompt_plan',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        expected_head: 'message-1',
                        user_text: '합성 사용자 입력',
                        generation_target: {
                            model_route_id: 'route-1',
                            generation_preset_id: 'generation-1',
                        },
                        prompt_preset_id: 'prompt-1',
                        variable_overrides: variables,
                        plan_hash: 'sha256:reviewed-plan',
                        generation_attempt_id: 'generation-attempt-1',
                    },
                },
            },
            {
                commandName: 'upsert_prompt_preset',
                args: {
                    request: {
                        value: creatorPromptPreset,
                        expected_revision: 7,
                    },
                },
            },
            {
                commandName: 'get_prompt_preset',
                args: {
                    request: {
                        prompt_preset_id: 'prompt-1',
                    },
                },
            },
            {
                commandName: 'get_editable_prompt_preset',
                args: {
                    request: {
                        prompt_preset_id: 'prompt-1',
                    },
                },
            },
            {
                commandName: 'list_prompt_presets',
                args: undefined,
            },
            {
                commandName: 'delete_prompt_preset',
                args: {
                    request: {
                        prompt_preset_id: 'prompt-delete',
                        expected_revision: 8,
                    },
                },
            },
            {
                commandName: 'reorder_prompt_blocks',
                args: {
                    request: {
                        prompt_preset_id: 'prompt-1',
                        ordered_block_ids: ['block-2', 'block-1'],
                        expected_revision: 7,
                    },
                },
            },
            {
                commandName: 'list_task_profiles',
                args: undefined,
            },
            {
                commandName: 'upsert_task_profile',
                args: {
                    request: {
                        value: taskProfile,
                        expected_revision: 3,
                    },
                },
            },
            {
                commandName: 'delete_task_profile',
                args: {
                    request: {
                        task_profile_id: 'task-delete',
                        expected_revision: 4,
                    },
                },
            },
            {
                commandName: 'delete_memory_record',
                args: {
                    request: {
                        memory_record_id: 'memory-delete',
                        expected_revision: 9,
                    },
                },
            },
            {
                commandName: 'get_memory_record',
                args: {
                    request: {
                        memory_record_id: 'memory-1',
                    },
                },
            },
            {
                commandName: 'list_prompt_preset_bindings',
                args: {
                    request: { scope: 'conversation', target_id: 'conversation-1' },
                },
            },
            {
                commandName: 'list_memory_records',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        include_invalidated: false,
                    },
                },
            },
            {
                commandName: 'simulate_knowledge_activation',
                args: {
                    request: {
                        knowledge_book_id: 'knowledge-1',
                        sample_texts: ['합성 검사 문장'],
                        manual_entry_ids: [],
                        semantic_scores: [],
                        variables,
                        supported_capabilities: [],
                        token_estimates: [],
                        activation_seed: 7,
                    },
                },
            },
            {
                commandName: 'list_content_module_bindings',
                args: { request: { content_module_id: 'module-1' } },
            },
            {
                commandName: 'list_content_module_revisions',
                args: { request: { content_module_id: 'module-1' } },
            },
            {
                commandName: 'diff_content_module_revisions',
                args: {
                    request: {
                        content_module_id: 'module-1',
                        from_revision: 1,
                        to_revision: 2,
                    },
                },
            },
            {
                commandName: 'evaluate_content_module_share',
                args: { request: { content_module_id: 'module-1' } },
            },
        ]);
    });

    it('wraps explicit memory query retry commands in bounded request objects', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);

        await client.listRetryableMemoryQueryEmbeddings({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            limit: 32,
        });
        await client.retryMemoryQueryEmbedding({
            id: 'query-embedding-1',
            expected_revision: 4,
            acknowledge_unknown_outcome: true,
        });

        expect(transport.calls).toEqual([
            {
                commandName: 'list_retryable_memory_query_embeddings',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        limit: 32,
                    },
                },
            },
            {
                commandName: 'retry_memory_query_embedding',
                args: {
                    request: {
                        id: 'query-embedding-1',
                        expected_revision: 4,
                        acknowledge_unknown_outcome: true,
                    },
                },
            },
        ]);
    });

    it('wraps capability and assistant lifecycle commands in the exact request object', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);

        await client.listCapabilityObservations('route-1');
        await client.effectiveCapability('route-1', 'reasoning');
        await client.effectiveParameterSpecs('route-1');
        await client.upsertUserCapabilityOverride({
            id: 'override-1',
            model_route_id: 'route-1',
            key: 'streaming',
            value: { type: 'boolean', value: true },
            status: 'verified',
            expires_at: null,
        });
        await client.deleteUserCapabilityOverride('route-1', 'override-1');
        await client.getProviderDiscoveryAssistantResumeBoundary('discovery-1');
        await client.runProviderDiscoveryAssistantTurn('discovery-1');
        await client.resumeProviderDiscoveryAssistantCoreHostAction('discovery-1');
        await client.approveProviderDiscoveryAssistantRetry('discovery-1');
        await client.requestProviderDiscoveryAssistantRevision('discovery-1');
        await client.acceptProviderDiscoveryAssistantDraft('discovery-1');
        await client.recordProviderDiscoveryAssistantFailure('discovery-1', 'timeout', true);
        await client.interruptProviderDiscoveryAssistant('discovery-1', 'external_outcome_unknown');
        await client.restartProviderDiscoveryAssistantAfterInterruption('discovery-1');

        expect(transport.calls).toEqual([
            {
                commandName: 'list_capability_observations',
                args: { request: { model_route_id: 'route-1' } },
            },
            {
                commandName: 'effective_capability',
                args: { request: { model_route_id: 'route-1', key: 'reasoning' } },
            },
            {
                commandName: 'effective_parameter_specs',
                args: { request: { model_route_id: 'route-1' } },
            },
            {
                commandName: 'upsert_user_capability_override',
                args: {
                    request: {
                        input: {
                            id: 'override-1',
                            model_route_id: 'route-1',
                            key: 'streaming',
                            value: { type: 'boolean', value: true },
                            status: 'verified',
                            expires_at: null,
                        },
                    },
                },
            },
            {
                commandName: 'delete_user_capability_override',
                args: {
                    request: {
                        model_route_id: 'route-1',
                        observation_id: 'override-1',
                    },
                },
            },
            {
                commandName: 'get_provider_discovery_assistant_resume_boundary',
                args: { request: { session_id: 'discovery-1' } },
            },
            {
                commandName: 'run_provider_discovery_assistant_turn',
                args: {
                    request: {
                        session_id: 'discovery-1',
                    },
                },
            },
            {
                commandName: 'resume_provider_discovery_assistant_core_host_action',
                args: { request: { session_id: 'discovery-1' } },
            },
            {
                commandName: 'approve_provider_discovery_assistant_retry',
                args: { request: { session_id: 'discovery-1' } },
            },
            {
                commandName: 'request_provider_discovery_assistant_revision',
                args: { request: { session_id: 'discovery-1' } },
            },
            {
                commandName: 'accept_provider_discovery_assistant_draft',
                args: { request: { session_id: 'discovery-1' } },
            },
            {
                commandName: 'record_provider_discovery_assistant_failure',
                args: {
                    request: {
                        session_id: 'discovery-1',
                        kind: 'timeout',
                        retryable: true,
                    },
                },
            },
            {
                commandName: 'interrupt_provider_discovery_assistant',
                args: {
                    request: {
                        session_id: 'discovery-1',
                        outcome: 'external_outcome_unknown',
                    },
                },
            },
            {
                commandName: 'restart_provider_discovery_assistant_after_interruption',
                args: { request: { session_id: 'discovery-1' } },
            },
        ]);
        expect(JSON.stringify(transport.calls)).not.toContain('credential');
        expect(JSON.stringify(transport.calls)).not.toContain('"estimate"');
    });

    it('sends only bounded PromptPreset rollback request DTOs', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);

        await client.listPromptPresetRevisions({
            prompt_preset_id: 'preset-1',
            limit: 100,
        });
        await client.diffPromptPresetRevisions({
            prompt_preset_id: 'preset-1',
            from_revision: 3,
            to_revision: 1,
        });
        await client.reviewPromptPresetRollback({
            prompt_preset_id: 'preset-1',
            expected_current_revision: 3,
            target_revision: 1,
        });
        await client.applyPromptPresetRollback({
            prompt_preset_id: 'preset-1',
            expected_current_revision: 3,
            target_revision: 1,
            approval_id: 'approval-1',
            expected_review_sha256: 'a'.repeat(64),
        });

        expect(transport.calls).toEqual([
            {
                commandName: 'list_prompt_preset_revisions',
                args: {
                    request: {
                        prompt_preset_id: 'preset-1',
                        limit: 100,
                    },
                },
            },
            {
                commandName: 'diff_prompt_preset_revisions',
                args: {
                    request: {
                        prompt_preset_id: 'preset-1',
                        from_revision: 3,
                        to_revision: 1,
                    },
                },
            },
            {
                commandName: 'review_prompt_preset_rollback',
                args: {
                    request: {
                        prompt_preset_id: 'preset-1',
                        expected_current_revision: 3,
                        target_revision: 1,
                    },
                },
            },
            {
                commandName: 'apply_prompt_preset_rollback',
                args: {
                    request: {
                        prompt_preset_id: 'preset-1',
                        expected_current_revision: 3,
                        target_revision: 1,
                        approval_id: 'approval-1',
                        expected_review_sha256: 'a'.repeat(64),
                    },
                },
            },
        ]);
        expect(JSON.stringify(transport.calls)).not.toContain('target_document');
        expect(JSON.stringify(transport.calls)).not.toContain('binding_snapshot');
        expect(JSON.stringify(transport.calls)).not.toContain('provenance');
    });
});
