import type { SampleBranch } from './view-types';

/** Presentation actions; concrete workflow and stream ownership stay in controllers. */
export interface ChatSession {
    readonly busy: boolean;
    readonly canStop?: boolean;
    readonly submitting?: boolean;
    loadHistory?(direction: 'older' | 'newer' | 'latest'): Promise<void>;
    branches(): SampleBranch[];
    selectBranch(id: string): unknown;
    fork(id: string): unknown;
    edit(id: string): ((value: string) => void) | ((value: string) => Promise<boolean>);
    removeFrom(id: string): unknown;
    regenerate(id: string, retry?: boolean): unknown;
    stop(): unknown;
}
