import type { LorepiaAppState } from '../../../app/app-controller';
import type { MemoryRecordSourceNavigationDto } from '../../../lib/ipc/contracts';
import type { OrchestrationController, OrchestrationState } from '../orchestration-controller';

export interface MemorySectionProps {
    appState: LorepiaAppState;
    orchestrationState: OrchestrationState;
    controller: OrchestrationController;
    detailPage?: string | null;
    onNavigateToMemorySource?: (source: MemoryRecordSourceNavigationDto) => void;
    memoryDrafts?: Record<string, string>;
    pendingMemoryDeleteId?: string | null;
    knowledgeSample?: string;
    transformRuleId?: string;
    transformSample?: string;
}
