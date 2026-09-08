import type { MessageDto } from '../../lib/ipc/contracts';

export interface SampleMessage {
    id: string;
    role: 'assistant' | 'user' | 'system';
    text: string;
    status?: 'pending' | 'complete' | 'cancelled' | 'failed';
    sample?: boolean;
    source?: MessageDto;
}

export interface SampleBranch {
    id: string;
    title: string;
    messages: SampleMessage[];
}

export interface SampleConversation {
    id: string;
    title: string;
    date: string;
    messages: SampleMessage[];
    mode?: 'chat' | 'story';
    branches?: SampleBranch[];
    activeBranchId?: string;
    responsePreview?: 'complete' | 'failed' | 'slow';
}

export interface SampleCharacter {
    id: string;
    name: string;
    description: string;
    thumbnail: string;
    subpage: boolean;
    histories: SampleConversation[];
    avatarAssetId?: string | null;
}

export type Page = 0 | 1 | 2;
export type Overlay =
    'app-settings' | 'card-info' | 'card-settings' | 'room-settings' | 'new-chat' | 'add-character';
export type Appearance = 'system' | 'light' | 'dark';

export interface UiFormValues {
    name: string;
    description: string;
    title: string;
    subpage: boolean;
    mode?: 'chat' | 'story';
    responsePreview?: 'complete' | 'failed' | 'slow';
}
