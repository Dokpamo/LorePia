import type { LorepiaAppController, LorepiaAppState } from '../../../app/app-controller';
import type { LorepiaClient, OrchestrationDocumentClientApi } from '../../../lib/ipc/contracts';
import type { ContentModuleLifecycleClientApi } from '../../orchestration/module-lifecycle-contracts';
import type {
    OrchestrationController,
    OrchestrationState,
} from '../../orchestration/orchestration-controller';
import type {
    ContentPackageController,
    ContentPackageState,
} from '../../orchestration/content-package-controller';

export interface SettingsServices {
    client: LorepiaClient &
        Partial<OrchestrationDocumentClientApi & ContentModuleLifecycleClientApi>;
    appState: LorepiaAppState;
    appController: LorepiaAppController;
    orchestrationState: OrchestrationState;
    orchestrationController: OrchestrationController;
    contentPackageState: ContentPackageState;
    contentPackageController: ContentPackageController;
}
