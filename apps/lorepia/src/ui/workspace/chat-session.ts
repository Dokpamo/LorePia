import type { SampleBranch } from './view-types';

/** Presentation actions; concrete workflow and stream ownership stay in controllers. */
export interface ChatSession {
    readonly busy: boolean;
    branches(): SampleBranch[];
    selectBranch(id: string): unknown;
    fork(id: string): unknown;
    edit(id: string): (value: string) => void | boolean | Promise<boolean | void>;
    removeFrom(id: string): unknown;
    regenerate(id: string, retry?: boolean): unknown;
    stop(): unknown;
}
