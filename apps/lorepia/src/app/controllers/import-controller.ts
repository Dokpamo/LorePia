import { t } from '../../lib/i18n';
import type { AppControllerContext } from './controller-context';

export class ImportController {
    constructor(private readonly context: AppControllerContext) {}

    async begin(): Promise<void> {
        this.context.update((state) => ({
            ...state,
            import_flow: { phase: 'loading', error: null, inspection: null },
        }));
        try {
            const ticket = await this.context.client.selectImportSource();
            if (ticket === null) {
                this.context.update((state) => ({
                    ...state,
                    import_flow: { phase: 'idle', error: null, inspection: null },
                }));
                return;
            }
            const inspection = await this.context.client.inspectImport(ticket.ticket_id);
            this.context.update((state) => ({
                ...state,
                import_flow: { phase: 'ready', error: null, inspection },
            }));
            this.context.announce(t('import.notice.review', { name: inspection.display_name }));
        } catch (error: unknown) {
            this.context.update((state) => ({
                ...state,
                import_flow: {
                    phase: 'error',
                    error: this.context.errorLabel(error),
                    inspection: null,
                },
            }));
        }
    }

    async commit(): Promise<void> {
        const inspection = this.context.readState().import_flow.inspection;
        if (inspection?.allowed !== true) return;
        this.context.update((state) => ({
            ...state,
            import_flow: { ...state.import_flow, phase: 'loading', error: null },
        }));
        try {
            const character = await this.context.client.commitImport(inspection.inspection_id);
            this.context.update((state) => ({
                ...state,
                library: {
                    phase: 'ready',
                    error: null,
                    characters: [
                        character,
                        ...state.library.characters.filter((item) => item.id !== character.id),
                    ],
                },
                import_flow: { phase: 'idle', error: null, inspection: null },
            }));
            this.context.announce(t('import.notice.added', { name: character.name }));
        } catch (error: unknown) {
            this.context.update((state) => ({
                ...state,
                import_flow: {
                    ...state.import_flow,
                    phase: 'error',
                    error: this.context.errorLabel(error),
                },
            }));
        }
    }

    async discard(): Promise<void> {
        const inspection = this.context.readState().import_flow.inspection;
        this.context.update((state) => ({
            ...state,
            import_flow: { phase: 'idle', error: null, inspection: null },
        }));
        if (inspection === null) return;
        try {
            await this.context.client.discardImport(inspection.inspection_id);
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }
}
