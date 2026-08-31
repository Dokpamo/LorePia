import type { ChatStreamItemDto } from '../contracts';

import { normalizeClientError } from '../errors';

import type { LOREPIA_COMMANDS } from '../commands';

type CommandName = (typeof LOREPIA_COMMANDS)[keyof typeof LOREPIA_COMMANDS];

export interface LorepiaTransport {
    invoke(commandName: string, args?: Record<string, unknown>): Promise<unknown>;
    createChatChannel(onMessage: (message: ChatStreamItemDto) => void): unknown;
    listen(eventName: string, onPayload: (payload: unknown) => void): Promise<() => void>;
}

export abstract class ClientTransportBase {
    constructor(protected readonly transport: LorepiaTransport) {}

    protected async call<Result>(
        name: CommandName,
        args?: Record<string, unknown>,
    ): Promise<Result> {
        try {
            return (await this.transport.invoke(name, args)) as Result;
        } catch (error: unknown) {
            throw normalizeClientError(error);
        }
    }
}
