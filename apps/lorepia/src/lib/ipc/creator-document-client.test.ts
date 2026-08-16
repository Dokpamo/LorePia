import { describe, expect, it } from 'vitest';

import type {
    CreatorContentModuleDocumentDto,
    CreatorInteractionRuleSetDocumentDto,
    CreatorKnowledgeBookDocumentDto,
    CreatorMemoryProfileDocumentDto,
    CreatorTransformSetDocumentDto,
} from './contracts';
import { LiveLorepiaClient, type LorepiaTransport } from './client';

class RecordingTransport implements LorepiaTransport {
    readonly calls: { commandName: string; args?: Record<string, unknown> }[] = [];

    invoke(commandName: string, args?: Record<string, unknown>): Promise<unknown> {
        this.calls.push({ commandName, args });
        return Promise.resolve(undefined);
    }

    createChatChannel(): unknown {
        return {};
    }

    listen(): Promise<() => void> {
        return Promise.resolve(() => undefined);
    }
}

const MEMORY_PROFILE: CreatorMemoryProfileDocumentDto = {
    id: 'memory-default',
    name: 'Default memory',
    summary_task: 'summary-task',
    embedding_task: null,
    turns_per_summary: 8,
    recent_raw_budget: { max_tokens: 2_048 },
    episodic_budget: { max_tokens: 1_024 },
    semantic_budget: { max_tokens: 1_024 },
    retrieval_count: 8,
    recency_weight: 1,
    similarity_weight: 1,
    importance_weight: 1,
    preserve_invalidated_records: false,
    summary_schema: 'summary-v1',
};

const KNOWLEDGE_BOOK: CreatorKnowledgeBookDocumentDto = {
    id: 'knowledge-default',
    name: 'Default knowledge',
    entries: [],
    scan_depth: 8,
    token_budget: { max_tokens: 4_096 },
    recursive: false,
    max_recursion_depth: 0,
};

const TRANSFORM_SET: CreatorTransformSetDocumentDto = {
    id: 'transform-default',
    name: 'Default transforms',
    enabled: false,
    rules: [],
    max_rules_per_phase: 64,
    max_output_chars: 65_536,
};

const INTERACTION_RULE_SET: CreatorInteractionRuleSetDocumentDto = {
    id: 'interaction-default',
    name: 'Default interactions',
    rules: [],
    max_actions_per_event: 128,
};

const CONTENT_MODULE: CreatorContentModuleDocumentDto = {
    id: 'module-default',
    name: 'Default module',
    version: '0.1.0',
    prompt_fragments: [],
    knowledge_book_ids: [],
    control_specs: [],
    transform_set_ids: [],
    interaction_rule_set_ids: [],
    asset_ids: [],
    required_capabilities: [],
    metadata: {
        author: null,
        license: 'proprietary',
        redistribution_allowed: false,
        homepage: null,
        description: '',
        tags: [],
    },
};

describe('Creator document transport boundary', () => {
    it('wires list/get/upsert/delete with exact CAS requests for every safe document kind', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);

        await client.listMemoryProfiles();
        await client.getMemoryProfile({ memory_profile_id: MEMORY_PROFILE.id });
        await client.upsertMemoryProfile({ value: MEMORY_PROFILE, expected_revision: 3 });
        await client.deleteMemoryProfile({
            memory_profile_id: MEMORY_PROFILE.id,
            expected_revision: 4,
        });

        await client.listKnowledgeBooks();
        await client.getKnowledgeBook({ knowledge_book_id: KNOWLEDGE_BOOK.id });
        await client.upsertKnowledgeBook({ value: KNOWLEDGE_BOOK, expected_revision: null });
        await client.deleteKnowledgeBook({
            knowledge_book_id: KNOWLEDGE_BOOK.id,
            expected_revision: 1,
        });

        await client.listTransformSets();
        await client.getTransformSet({ transform_set_id: TRANSFORM_SET.id });
        await client.upsertTransformSet({ value: TRANSFORM_SET, expected_revision: 5 });
        await client.deleteTransformSet({
            transform_set_id: TRANSFORM_SET.id,
            expected_revision: 6,
        });

        await client.listInteractionRuleSets();
        await client.getInteractionRuleSet({
            interaction_rule_set_id: INTERACTION_RULE_SET.id,
        });
        await client.upsertInteractionRuleSet({
            value: INTERACTION_RULE_SET,
            expected_revision: 7,
        });
        await client.deleteInteractionRuleSet({
            interaction_rule_set_id: INTERACTION_RULE_SET.id,
            expected_revision: 8,
        });

        await client.listContentModules();
        await client.getContentModule({ content_module_id: CONTENT_MODULE.id });
        await client.upsertContentModule({ value: CONTENT_MODULE, expected_revision: 9 });
        await client.deleteContentModule({
            content_module_id: CONTENT_MODULE.id,
            expected_revision: 10,
        });

        expect(transport.calls.map(({ commandName }) => commandName)).toEqual([
            'list_memory_profiles',
            'get_memory_profile',
            'upsert_memory_profile',
            'delete_memory_profile',
            'list_knowledge_books',
            'get_knowledge_book',
            'upsert_knowledge_book',
            'delete_knowledge_book',
            'list_transform_sets',
            'get_transform_set',
            'upsert_transform_set',
            'delete_transform_set',
            'list_interaction_rule_sets',
            'get_interaction_rule_set',
            'upsert_interaction_rule_set',
            'delete_interaction_rule_set',
            'list_content_modules',
            'get_content_module',
            'upsert_content_module',
            'delete_content_module',
        ]);
        expect(transport.calls[2]?.args).toEqual({
            request: { value: MEMORY_PROFILE, expected_revision: 3 },
        });
        expect(transport.calls[6]?.args).toEqual({
            request: { value: KNOWLEDGE_BOOK, expected_revision: null },
        });
        expect(transport.calls[11]?.args).toEqual({
            request: { transform_set_id: TRANSFORM_SET.id, expected_revision: 6 },
        });
        expect(transport.calls[14]?.args).toEqual({
            request: { value: INTERACTION_RULE_SET, expected_revision: 7 },
        });
        expect(transport.calls[19]?.args).toEqual({
            request: { content_module_id: CONTENT_MODULE.id, expected_revision: 10 },
        });
        expect(JSON.stringify(transport.calls)).not.toContain('provenance');
        expect(JSON.stringify(transport.calls)).not.toContain('imported_author_enabled');
    });
});
