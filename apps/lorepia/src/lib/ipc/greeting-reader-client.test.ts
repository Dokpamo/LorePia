import { expect, it, vi } from 'vitest';
import { LiveLorepiaClient, type LorepiaTransport } from './client';
import { createPreviewClient } from '../../preview/mock-client';

it('reads one opening by exact revision and passes through its source without starting chat', async () => {
    const input = {
        character_id: 'character',
        character_content_revision_id: 'revision',
        greeting_id: 'alternate-1',
    };
    const response = { ...input, text: 'Full scene.' };
    const invoke = vi.fn(() => Promise.resolve(response));
    const transport: LorepiaTransport = {
        invoke,
        createChatChannel: vi.fn(),
        listen: vi.fn(() => Promise.resolve(() => undefined)),
    };
    const client = new LiveLorepiaClient(transport);
    await expect(client.getCharacterGreetingDetail(input)).resolves.toEqual(response);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('get_character_greeting_detail', {
        request: input,
    });
});

it('keeps preview reads synthetic and rejects mismatched revision or unknown opening', async () => {
    const client = createPreviewClient();
    const character = (await client.listCharacters())[0];
    if (!character || !client.getCharacterGreetingDetail) throw new Error('No demo reader');
    const catalog = await client.getCharacterGreetingCatalog(character.id);
    const greeting = catalog.greetings[0];
    if (!greeting || !catalog.character_content_revision_id) throw new Error('No demo opening');
    const input = {
        character_id: character.id,
        character_content_revision_id: catalog.character_content_revision_id,
        greeting_id: greeting.id,
    };
    const read = await client.getCharacterGreetingDetail(input);
    expect(read.text.length).toBeGreaterThan(0);
    read.text = 'changed locally';
    expect((await client.getCharacterGreetingDetail(input)).text).not.toBe('changed locally');
    await expect(
        client.getCharacterGreetingDetail({ ...input, character_content_revision_id: 'stale' }),
    ).rejects.toThrow();
    await expect(
        client.getCharacterGreetingDetail({ ...input, greeting_id: 'unknown' }),
    ).rejects.toThrow();
});
