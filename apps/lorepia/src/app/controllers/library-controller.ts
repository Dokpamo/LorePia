import type { AppControllerContext } from './controller-context';

export class LibraryController {
    constructor(
        private readonly context: AppControllerContext,
        private readonly isAppEpochCurrent: (epoch: number) => boolean,
    ) {}

    async load(parentEpoch: number): Promise<void> {
        this.context.update((state) => ({
            ...state,
            library: { ...state.library, phase: 'loading', error: null },
        }));
        try {
            const characters = await this.context.client.listCharacters();
            if (!this.isAppEpochCurrent(parentEpoch)) return;
            this.context.update((state) => ({
                ...state,
                library: { phase: 'ready', error: null, characters },
            }));
        } catch (error: unknown) {
            if (!this.isAppEpochCurrent(parentEpoch)) return;
            this.context.update((state) => ({
                ...state,
                library: {
                    ...state.library,
                    phase: 'error',
                    error: this.context.errorLabel(error),
                },
            }));
        }
    }
}
