import type {
    CharacterGreetingSelectionInput,
    ChatStreamItemDto,
    ConversationBranchDto,
    ConversationDto,
    ConversationMode,
    ConversationStateDto,
    EditUserMessageInput,
    GenerateRuntimeTextInput,
    GenerationStartedDto,
    MessageDto,
    MessageActionGenerationDto,
    ReviewedPromptSendInput,
    RegenerateAssistantMessageInput,
    RemoveMessageInput,
    SendMessageInput,
    RuntimeTextGenerationDto,
} from '../contracts';

import type {
    GetPortableRuntimeStateDto,
    PortableRuntimeStateScopeInput,
    PutPortableRuntimeStateInput,
    PutPortableRuntimeStateResultDto,
} from '../portable-runtime-state-contracts';

import { LOREPIA_COMMANDS } from '../commands';

import { ContentImportClient } from './content-import';

export abstract class ConversationClient extends ContentImportClient {
    listConversations(characterId: string | null): Promise<ConversationDto[]> {
        if (characterId === null) {
            return this.call(LOREPIA_COMMANDS.listConversations);
        }
        return this.call(LOREPIA_COMMANDS.listConversationsForCharacter, {
            request: { character_id: characterId },
        });
    }

    createConversation(
        characterId: string,
        title: string,
        mode: ConversationMode,
        greeting?: CharacterGreetingSelectionInput,
    ): Promise<ConversationDto> {
        const input: {
            character_id: string;
            title: string;
            mode: ConversationMode;
            greeting?: CharacterGreetingSelectionInput;
        } = { character_id: characterId, title, mode };
        if (greeting !== undefined) input.greeting = greeting;
        return this.call(LOREPIA_COMMANDS.createConversation, {
            input,
        });
    }

    openConversation(characterId: string): Promise<ConversationDto> {
        return this.call(LOREPIA_COMMANDS.openConversation, {
            request: { character_id: characterId },
        });
    }

    openExistingConversation(conversationId: string): Promise<ConversationDto> {
        return this.call(LOREPIA_COMMANDS.openExistingConversation, {
            request: { conversation_id: conversationId },
        });
    }

    getConversation(conversationId: string): Promise<ConversationDto> {
        return this.call(LOREPIA_COMMANDS.getConversation, {
            request: { conversation_id: conversationId },
        });
    }

    getConversationState(conversationId: string): Promise<ConversationStateDto> {
        return this.call(LOREPIA_COMMANDS.getConversationState, {
            request: { conversation_id: conversationId },
        });
    }

    listBranches(conversationId: string): Promise<ConversationBranchDto[]> {
        return this.call(LOREPIA_COMMANDS.listBranches, {
            request: { conversation_id: conversationId },
        });
    }

    createBranch(
        conversationId: string,
        fromMessageId: string | null,
        title: string | null,
    ): Promise<ConversationBranchDto> {
        return this.call(LOREPIA_COMMANDS.createBranch, {
            input: {
                conversation_id: conversationId,
                from_message_id: fromMessageId,
                title,
            },
        });
    }

    selectBranch(conversationId: string, branchId: string): Promise<ConversationStateDto> {
        return this.call(LOREPIA_COMMANDS.selectBranch, {
            input: { conversation_id: conversationId, branch_id: branchId },
        });
    }

    setConversationMode(
        conversationId: string,
        mode: ConversationMode,
    ): Promise<ConversationStateDto> {
        return this.call(LOREPIA_COMMANDS.setConversationMode, {
            input: { conversation_id: conversationId, mode },
        });
    }

    listBranchMessages(branchId: string): Promise<MessageDto[]> {
        return this.call(LOREPIA_COMMANDS.listBranchMessages, {
            request: { branch_id: branchId },
        });
    }

    listMessages(conversationId: string): Promise<MessageDto[]> {
        return this.call(LOREPIA_COMMANDS.listMessages, {
            request: { conversation_id: conversationId },
        });
    }

    generateRuntimeText(input: GenerateRuntimeTextInput): Promise<RuntimeTextGenerationDto> {
        return this.call(LOREPIA_COMMANDS.generateRuntimeText, { input });
    }

    cancelRuntimeText(requestId: string): Promise<boolean> {
        return this.call(LOREPIA_COMMANDS.cancelRuntimeText, {
            request: { request_id: requestId },
        });
    }

    getPortableRuntimeState(
        scope: PortableRuntimeStateScopeInput,
    ): Promise<GetPortableRuntimeStateDto> {
        return this.call(LOREPIA_COMMANDS.getPortableRuntimeState, { request: { scope } });
    }

    putPortableRuntimeState(
        input: PutPortableRuntimeStateInput,
    ): Promise<PutPortableRuntimeStateResultDto> {
        return this.call(LOREPIA_COMMANDS.putPortableRuntimeState, { request: input });
    }

    sendMessage(
        input: SendMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<GenerationStartedDto> {
        const onEvent = this.transport.createChatChannel(onItem);
        return this.call(LOREPIA_COMMANDS.sendMessage, { input, streamId, onEvent });
    }

    sendReviewedPrompt(
        input: ReviewedPromptSendInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<GenerationStartedDto> {
        const onEvent = this.transport.createChatChannel(onItem);
        return this.call(LOREPIA_COMMANDS.sendReviewedPrompt, { input, streamId, onEvent });
    }

    editUserMessage(
        input: EditUserMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<MessageActionGenerationDto> {
        const onEvent = this.transport.createChatChannel(onItem);
        return this.call(LOREPIA_COMMANDS.editUserMessage, { input, streamId, onEvent });
    }

    regenerateAssistantMessage(
        input: RegenerateAssistantMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<MessageActionGenerationDto> {
        const onEvent = this.transport.createChatChannel(onItem);
        return this.call(LOREPIA_COMMANDS.regenerateAssistantMessage, {
            input,
            streamId,
            onEvent,
        });
    }

    removeMessageFromBranch(input: RemoveMessageInput): Promise<ConversationBranchDto> {
        return this.call(LOREPIA_COMMANDS.removeMessageFromBranch, { input });
    }

    cancelGeneration(generationId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.cancelGeneration, {
            request: { generation_id: generationId },
        });
    }

    subscribeGeneration(
        generationId: string,
        conversationId: string,
        branchId: string,
        sequenceBaseline: number,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<void> {
        const onEvent = this.transport.createChatChannel(onItem);
        return this.call(LOREPIA_COMMANDS.subscribeGeneration, {
            request: {
                generation_id: generationId,
                conversation_id: conversationId,
                branch_id: branchId,
                sequence_baseline: sequenceBaseline,
            },
            streamId,
            onEvent,
        });
    }

    disposeChatStream(streamId: string): Promise<boolean> {
        return this.call(LOREPIA_COMMANDS.disposeChatStream, {
            request: { stream_id: streamId },
        });
    }
}
