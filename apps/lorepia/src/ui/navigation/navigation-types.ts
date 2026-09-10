export const ROOT_TABS = ['home', 'chats', 'create', 'settings'] as const;
export type RootTab = (typeof ROOT_TABS)[number];
export type LibrarySortOrder = 'newest' | 'oldest' | 'name';

export interface ConversationListItem {
    id: string;
    characterId: string;
    characterName: string;
    title: string;
    date: string;
    updatedAt: string;
}
