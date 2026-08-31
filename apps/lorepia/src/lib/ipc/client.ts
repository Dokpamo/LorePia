import { Channel, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import type { ContentModuleLifecycleClientApi } from '../../features/orchestration/module-lifecycle-contracts';
import type { PersonaClientApi } from '../../features/personas/persona-contracts';
import type {
    ChatStreamItemDto,
    ContentPackageClientApi,
    GenerationAttemptApprovalClientApi,
    LorepiaClient,
    OrchestrationClientApi,
    OrchestrationDocumentClientApi,
    PromptPresetHistoryClientApi,
    RoomInteractionClientApi,
} from './contracts';
import { LOREPIA_COMMANDS, LOREPIA_EVENTS } from './commands';
import { ModuleLifecycleClient } from './clients/module-lifecycle';
import type { LorepiaTransport } from './clients/transport';

export { LOREPIA_COMMANDS, LOREPIA_EVENTS };

export type { LorepiaTransport };

export class TauriTransport implements LorepiaTransport {
    invoke(commandName: string, args?: Record<string, unknown>): Promise<unknown> {
        return invoke(commandName, args);
    }

    createChatChannel(onMessage: (message: ChatStreamItemDto) => void): Channel<ChatStreamItemDto> {
        const channel = new Channel<ChatStreamItemDto>();
        channel.onmessage = onMessage;
        return channel;
    }

    listen(eventName: string, onPayload: (payload: unknown) => void): Promise<() => void> {
        return listen<unknown>(eventName, (event) => onPayload(event.payload));
    }
}

export class LiveLorepiaClient
    extends ModuleLifecycleClient
    implements
        LorepiaClient,
        OrchestrationClientApi,
        OrchestrationDocumentClientApi,
        PromptPresetHistoryClientApi,
        RoomInteractionClientApi,
        GenerationAttemptApprovalClientApi,
        ContentPackageClientApi,
        PersonaClientApi,
        ContentModuleLifecycleClientApi
{
    constructor(transport: LorepiaTransport = new TauriTransport()) {
        super(transport);
    }
}

export function createLiveLorepiaClient(): LorepiaClient &
    OrchestrationDocumentClientApi &
    PromptPresetHistoryClientApi &
    RoomInteractionClientApi &
    GenerationAttemptApprovalClientApi &
    ContentPackageClientApi &
    PersonaClientApi &
    ContentModuleLifecycleClientApi {
    return new LiveLorepiaClient();
}
