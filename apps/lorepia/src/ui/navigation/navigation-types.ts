export type RootTab = 'home' | 'chats' | 'create' | 'settings';
export type LibrarySortOrder = 'newest' | 'oldest' | 'name';

export interface ConversationListItem {
    id: string;
    characterId: string;
    characterName: string;
    title: string;
    date: string;
    updatedAt: string;
}
