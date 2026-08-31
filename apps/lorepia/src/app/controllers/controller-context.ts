import type { LorepiaClient } from '../../lib/ipc/contracts';
import type { LorepiaAppState } from '../app-state';

export interface AppControllerContext {
    readonly client: LorepiaClient;
    readState(): LorepiaAppState;
    update(updater: (state: LorepiaAppState) => LorepiaAppState): void;
    announce(message: string): void;
    errorLabel(error: unknown): string;
}
