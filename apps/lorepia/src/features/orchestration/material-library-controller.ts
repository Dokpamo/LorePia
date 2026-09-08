import type { LorepiaClient } from '../../lib/ipc/contracts';
import { CreatorDocumentController } from './controllers/creator-document-controller';
import { OrchestrationStateController } from './controllers/orchestration-state-controller';
import { INITIAL_ORCHESTRATION_STATE } from './controllers/orchestration-state';

/** Authoring has its own lifecycle and does not require an active chat room. */
export class MaterialLibraryController {
    private readonly owner = new OrchestrationStateController();
    readonly state = this.owner.state;
    readonly documents: CreatorDocumentController;
    constructor(client: LorepiaClient) {
        this.documents = new CreatorDocumentController(client, this.owner);
    }
    async load() {
        const epoch = this.owner.beginContextLoad();
        const key = `material-library:${String(epoch)}`;
        this.owner.set({
            ...structuredClone(INITIAL_ORCHESTRATION_STATE),
            context_key: key,
            phase: 'loading',
        });
        await this.documents.loadEditableCreatorDocumentsForContext(key);
        if (!this.owner.isContextEpoch(epoch)) return;
        this.owner.update((state) => ({
            ...state,
            phase: state.editable_creator_documents_error ? 'error' : 'ready',
        }));
    }
    destroy() {
        this.owner.destroy();
        this.owner.set(structuredClone(INITIAL_ORCHESTRATION_STATE));
    }
}
