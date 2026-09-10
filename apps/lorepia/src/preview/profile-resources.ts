import type { CharacterRenderProfileDto } from '../lib/ipc/contracts';
import { DEMO_CHARACTER_PRESENTATIONS } from './profile-fixtures';

interface LoreSeed {
    name: string;
    keys: string[];
    body: string;
}
const loreSeeds: Record<string, LoreSeed[]> = {
    'character-noa': [
        {
            name: '동네 끝 작은 화실',
            keys: ['화실', '창문', '그림'],
            body: '화실은 오래된 빵집 위층에 있다. 오후 세 시가 되면 서쪽 창문으로 긴 햇빛이 들어온다. 창가에는 손님을 위한 빈 의자와 깨끗한 스케치북이 늘 준비되어 있다.\n\n노아는 방문객에게 그림을 완성하라고 요구하지 않는다. 아무것도 그리지 않은 종이도 그날의 기록으로 소중히 보관한다.',
        },
        {
            name: '꿈과 현실이 만나는 골목',
            keys: ['꿈', '골목', '기억'],
            body: '꿈속 골목의 가게들은 같은 위치에 있지만 간판과 날씨는 매번 달라진다. 꿈에서 가져온 물건은 현실에서 색 하나로만 남는다.\n\n노아는 이유를 단정하지 않고, 손님이 기억하는 세부를 함께 그려보자고 제안한다. 이 설정은 이야기에 신비로운 분위기를 더할 뿐 정해진 결말을 강요하지 않는다.',
        },
        {
            name: '서고의 사서 아리아',
            keys: ['아리아', '사서', '서고'],
            body: '노아의 꿈에 등장하는 책을 지키는 사람. 누구에게서 빌렸는지 알 수 없는 기억을 책으로 묶는다. 노아의 그림 한 귀퉁이에 종종 아리아가 두고 간 파란 책이 나타난다.',
        },
        {
            name: '노아와 가까워지는 속도',
            keys: ['친구', '이름', '다시'],
            body: '노아는 처음 만난 사람의 감정을 넘겨짚지 않는다. 이름을 들으면 기억하고, 이전에 나눈 이야기가 있다면 사소한 풍경부터 조심스럽게 이어간다.\n\n관계를 빠르게 단정하거나 사용자의 행동을 대신 결정하지 않는다.',
        },
    ],
    'character-aria': [
        {
            name: '달빛 서고의 세 가지 규칙',
            keys: ['서고', '규칙'],
            body: '첫째, 주인을 찾지 못한 기억은 소리 내어 읽지 않는다.\n둘째, 빈 책장은 누군가의 이야기를 위해 남겨 둔다.\n셋째, 책을 돌려줄 때는 기억하고 싶은 문장 하나를 적는다.\n\n규칙을 어긴 사람을 처벌하기보다 그 이유를 듣는 것이 아리아의 방식이다.',
        },
        {
            name: '제목 없는 항해 일지',
            keys: ['항해', '일지', '세라'],
            body: '서고 가장 높은 책장에 있는 일지. 마지막 장에는 지도에 없는 별의 좌표가 적혀 있다. 세라라는 이름이 여러 번 등장하지만, 언제 쓰였는지는 알려져 있지 않다.',
        },
        {
            name: '기억을 돌려주는 차',
            keys: ['차', '기억'],
            body: '아리아가 내어주는 차는 기억을 강제로 보여주지 않는다. 차를 마시는 동안 떠오른 풍경을 손님이 이야기하면, 비슷한 기록을 찾아준다.',
        },
    ],
    'character-kai': [
        {
            name: '밤에도 불이 켜진 정비소',
            keys: ['정비소', '수리'],
            body: '공구는 용도보다 손에 익은 순서대로 놓여 있다. 카이는 수리를 시작하기 전에 물건의 사연을 듣는다. 해결할 수 없는 고장은 솔직하게 말하고 가능한 다른 방법을 함께 찾는다.',
        },
        {
            name: '전원이 없는 라디오',
            keys: ['라디오', '음악'],
            body: '배터리를 빼도 아주 희미한 음악이 들리는 낡은 라디오. 특정한 골목에 들어가면 방송이 더 선명해진다. 방송 속에서 노아의 화실을 묘사하는 문장이 반복된다.',
        },
        {
            name: '폐역으로 향하는 의뢰서',
            keys: ['폐역', '의뢰서'],
            body: '도시 철도 지도에서 지워진 역. 카이는 이곳에 가 본 적이 있는 듯하지만 확답하지 않는다. 함께 가기로 했다면 손전등과 작은 공구 가방을 챙긴다.',
        },
    ],
    'character-sera': [
        {
            name: '소형 탐사선 루멘',
            keys: ['우주선', '루멘'],
            body: '관측실과 작은 식당, 두 개의 개인실이 있는 탐사선. 항로는 승무원끼리 합의해서 정한다. 큰 위험이 예상되면 세라는 선택의 이유와 대안을 먼저 설명한다.',
        },
        {
            name: '지도에 없는 푸른 별',
            keys: ['별', '항로'],
            body: '공식 항해 지도에 없지만 달빛 서고의 오래된 일지에는 같은 좌표가 남아 있다. 접근할수록 통신기에 낮은 음악이 들린다.',
        },
        {
            name: '관측실의 교대 시간',
            keys: ['교대', '밤', '고향'],
            body: '세라는 교대 전 따뜻한 음료 두 잔을 준비한다. 상대가 말하고 싶지 않은 사연은 캐묻지 않고, 창밖의 별이나 고향의 날씨처럼 가벼운 이야기로 돌아온다.',
        },
    ],
};

const portraits = [
    { asset_id: 'portrait-noa', aliases: ['화가 노아'] },
    { asset_id: 'portrait-aria', aliases: ['서고의 사서 아리아'] },
    { asset_id: 'portrait-kai', aliases: ['정비공 카이'] },
    { asset_id: 'portrait-sera', aliases: ['항해사 세라'] },
];

/** Profile inspection data; scoped chat previews never execute sample scripts. */
export function demoRenderProfile(characterId: string, scoped = false): CharacterRenderProfileDto {
    const seeds = loreSeeds[characterId];
    if (!seeds) throw new Error(`Unknown demo character: ${characterId}`);
    const ownPortrait = characterId.replace('character-', 'portrait-');
    const assets = [
        ...portraits.filter((item) => item.asset_id === ownPortrait),
        ...portraits.filter((item) => item.asset_id !== ownPortrait),
    ];
    const scripts = scoped
        ? []
        : [
              {
                  id: `${characterId}-scene`,
                  name: '시작 장면 메타데이터',
                  event: 'start',
                  language: 'lua',
                  source: '-- UI에서 소스를 읽어보는 테스트 예시입니다.\n-- 이 미리보기에서는 실행하지 않습니다.\nlocal scene = {\n    time = "afternoon",\n    mood = "calm",\n    visited = false\n}\nreturn scene',
                  elevated_access: false,
              },
              {
                  id: `${characterId}-tone`,
                  name: '대화 길이에 따른 말투 분기',
                  event: 'output',
                  language: 'lua',
                  source: '-- 실행하지 않는 읽기용 예시\nlocal function choose_tone(message)\n    if #message < 40 then\n        return "short_and_gentle"\n    end\n    return "thoughtful_and_detailed"\nend\nreturn choose_tone("Hello")',
                  elevated_access: false,
              },
          ];
    return structuredClone({
        character_id: characterId,
        character_content_revision_id: `${characterId}-content-r3`,
        recommended_language: DEMO_CHARACTER_PRESENTATIONS[characterId]?.recommendedLanguage,
        greeting_previews: Object.entries(
            DEMO_CHARACTER_PRESENTATIONS[characterId]?.introductions ?? {},
        ).map(([id, item]) => ({
            id,
            title: item.title ?? null,
            excerpt: item.body,
            language: item.language,
            group_id: item.groupId,
        })),
        assets,
        background_markup: '',
        toggle_schema: '',
        initial_variables: {},
        output_transforms: [],
        display_transforms: scoped
            ? []
            : [{ pattern: '\\[장면:([^\\]]+)\\]', replacement: '*$1*', flags: 'g' }],
        runtime_scripts: scripts,
        required_runtime_capabilities: [],
        runtime_capabilities_declared: true,
        runtime_knowledge: seeds.map((item, index) => ({
            id: `${characterId}-lore-${String(index + 1)}`,
            name: item.name,
            content: item.body,
            enabled: true,
            primary_keys: item.keys,
            secondary_keys: [],
            constant: false,
            selective: false,
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            probability_basis_points: 10000,
            folder: false,
        })),
        runtime_script_count: scripts.length,
    });
}
