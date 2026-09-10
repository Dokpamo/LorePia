import { describe, expect, it } from 'vitest';
import type { ChatStreamItemDto } from '../lib/ipc/contracts';
import { createPreviewCharacterPresentations, createPreviewClient } from './mock-client';
import { previewImageSource } from './fixture-media';

describe('character profile demo', () => {
    it('opens each chosen situation as the first message and keeps sessions independent', async () => {
        const client = createPreviewClient();
        const presentations = createPreviewCharacterPresentations();
        const baseline = await client.listConversations(null);
        for (const character of await client.listCharacters()) {
            const catalog = await client.getCharacterGreetingCatalog(character.id);
            expect(catalog.greetings).toHaveLength(3);
            for (const greeting of catalog.greetings) {
                const conversation = await client.createConversation(
                    character.id,
                    greeting.id,
                    'story',
                    {
                        character_content_revision_id: catalog.character_content_revision_id,
                        greeting_id: greeting.id,
                    },
                );
                const state = await client.getConversationState(conversation.id);
                const messages = await client.listBranchMessages(state.active_branch_id);
                expect(state.selected_mode).toBe('story');
                expect(messages).toHaveLength(1);
                expect(messages[0]).toMatchObject({
                    content: presentations[character.id]?.introductions?.[greeting.id]?.body,
                    role: 'assistant',
                    parent_id: null,
                    status: 'complete',
                });
                expect((await client.listBranches(conversation.id))[0]?.head_message_id).toBe(
                    messages[0]?.id,
                );
            }
        }
        expect(await createPreviewClient().listConversations(null)).toEqual(baseline);
    });

    it('rejects invalid openings before mutation and supports starting without a greeting', async () => {
        const client = createPreviewClient();
        const initial = await client.listConversations(null);
        await expect(
            client.createConversation('character-noa', 'test', 'chat', {
                character_content_revision_id: 'stale',
                greeting_id: 'character-noa-greeting-default',
            }),
        ).rejects.toThrow('Stale');
        await expect(
            client.createConversation('character-noa', 'test', 'chat', {
                character_content_revision_id: 'character-noa-content-r3',
                greeting_id: 'character-aria-greeting-default',
            }),
        ).rejects.toThrow('Unknown');
        expect(await client.listConversations(null)).toEqual(initial);
        const conversation = await client.createConversation('character-noa', 'test', 'chat', {
            character_content_revision_id: 'character-noa-content-r3',
            greeting_id: null,
        });
        const state = await client.getConversationState(conversation.id);
        expect(await client.listBranchMessages(state.active_branch_id)).toEqual([]);
    });

    it('bundles every displayed image and keeps inspection resources out of executable chat profiles', async () => {
        const client = createPreviewClient();
        if (!client.getCharacterRenderProfile) throw new Error('Profile API missing');
        const presentations = createPreviewCharacterPresentations();
        for (const character of await client.listCharacters()) {
            expect(previewImageSource(character.avatar_asset_id ?? '')).toBeDefined();
            expect(
                previewImageSource(presentations[character.id]?.creator?.avatarAssetId ?? ''),
            ).toBeDefined();
            const profile = await client.getCharacterRenderProfile(character.id);
            expect(profile.assets).toHaveLength(4);
            expect(profile.runtime_knowledge.length).toBeGreaterThanOrEqual(3);
            expect(profile.runtime_scripts).toHaveLength(2);
            expect(profile.display_transforms).toHaveLength(1);
            for (const asset of profile.assets)
                expect(previewImageSource(asset.asset_id)).toBeDefined();
            profile.assets.splice(0);
            expect((await client.getCharacterRenderProfile(character.id)).assets).toHaveLength(4);
            const conversation = await client.createConversation(character.id, 'test', 'chat');
            const state = await client.getConversationState(conversation.id);
            const scoped = await client.getCharacterRenderProfile(character.id, {
                conversation_id: conversation.id,
                branch_id: state.active_branch_id,
            });
            expect(scoped.runtime_scripts).toEqual([]);
            expect(scoped.display_transforms).toEqual([]);
            expect(scoped.runtime_script_count).toBe(0);
        }
        expect(previewImageSource('https://example.com/image.png')).toBeUndefined();
        expect(previewImageSource('__proto__')).toBeUndefined();
    });

    it('continues a new conversation with deterministic replies and ordered stream events', async () => {
        const client = createPreviewClient();
        const conversation = await client.createConversation('character-noa', 'test', 'chat');
        const state = await client.getConversationState(conversation.id);
        const replies: string[] = [];
        for (const text of ['오늘은 초록색이 떠올라요.', '그림의 빈칸도 남겨 두고 싶어요.']) {
            const [branch] = await client.listBranches(conversation.id);
            if (!branch) throw new Error('Branch missing');
            const events: ChatStreamItemDto[] = [];
            await client.sendMessage(
                {
                    conversation_id: conversation.id,
                    branch_id: branch.id,
                    expected_head: branch.head_message_id,
                    mode: 'chat',
                    text,
                    operation_nonce: `demo-turn-${String(replies.length)}`,
                    selection: {
                        kind: 'target',
                        target: {
                            model_route_id: 'route-openai-demo',
                            generation_preset_id: 'preset-balanced-demo',
                        },
                    },
                },
                'demo-stream',
                (item) => events.push(item),
            );
            expect(
                events.map((event) => (event.type === 'event' ? event.payload.sequence : null)),
            ).toEqual([1, 2, 3, 4]);
            const messages = await client.listBranchMessages(state.active_branch_id);
            expect(messages.at(-2)?.content).toBe(text);
            const reply = messages.at(-1);
            expect(reply).toMatchObject({
                role: 'assistant',
                status: 'complete',
                parent_id: messages.at(-2)?.id,
            });
            replies.push(reply?.content ?? '');
        }
        expect(replies[0]).toContain('노아는 연필을 내려놓고');
        expect(replies[1]).toContain('빈칸으로 남겨 두죠');
        expect(await client.listBranchMessages(state.active_branch_id)).toHaveLength(5);
    });
});
