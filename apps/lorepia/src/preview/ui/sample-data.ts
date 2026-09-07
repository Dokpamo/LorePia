export interface SampleMessage {
    id: string;
    role: 'assistant' | 'user';
    text: string;
    status?: 'pending' | 'complete' | 'cancelled' | 'failed';
    sample?: boolean;
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
    responsePreview?: 'complete' | 'failed';
}

export interface SampleCharacter {
    id: string;
    name: string;
    description: string;
    thumbnail: string;
    subpage: boolean;
    histories: SampleConversation[];
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
    responsePreview?: 'complete' | 'failed';
}

export function createSampleCharacters(): SampleCharacter[] {
    return [
        {
            id: 'a',
            name: '서연',
            description: '작은 도서관의 사서',
            thumbnail: 'A',
            subpage: true,
            histories: [
                {
                    id: 'a1',
                    title: '비 오는 오후',
                    date: '오늘',
                    messages: [
                        {
                            id: 'a1-m1',
                            role: 'assistant',
                            text: '비가 그칠 때까지 여기 있어도 괜찮아요. 어떤 책을 찾고 있었어요?',
                        },
                        { id: 'a1-m2', role: 'user', text: '오늘은 추천해 주는 책을 읽어볼게.' },
                        { id: 'a1-m3', role: 'assistant', text: '그럼, 이 책은 어때요?' },
                    ],
                },
                {
                    id: 'a2',
                    title: '처음 만난 날',
                    date: '어제',
                    messages: [
                        { id: 'a2-m1', role: 'assistant', text: '어서 오세요. 처음 오셨죠?' },
                    ],
                },
                {
                    id: 'a3',
                    title: '오래된 약속',
                    date: '9월 3일',
                    messages: [
                        {
                            id: 'a3-m1',
                            role: 'assistant',
                            text: '다시 와 줬네요. 기다리고 있었어요.',
                        },
                    ],
                },
            ],
        },
        {
            id: 'b',
            name: '하루',
            description: '동네 카페의 바리스타',
            thumbnail: 'B',
            subpage: true,
            histories: [
                {
                    id: 'b1',
                    title: '늦은 오후의 커피',
                    date: '오늘',
                    messages: [
                        { id: 'b1-m1', role: 'assistant', text: '늘 마시던 걸로 드릴까요?' },
                    ],
                },
            ],
        },
        {
            id: 'c',
            name: '도윤',
            description: '여행 중에 만난 친구',
            thumbnail: 'C',
            subpage: true,
            histories: [
                {
                    id: 'c1',
                    title: '낯선 도시에서',
                    date: '어제',
                    messages: [
                        {
                            id: 'c1-m1',
                            role: 'assistant',
                            text: '다음에는 어느 골목으로 가 볼까요?',
                        },
                    ],
                },
            ],
        },
    ];
}
