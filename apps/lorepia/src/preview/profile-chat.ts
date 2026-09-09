import type { CharacterGreetingSelectionInput } from '../lib/ipc/contracts';
import { DEMO_CHARACTER_PRESENTATIONS } from './profile-fixtures';

export function demoOpening(characterId: string, selection?: CharacterGreetingSelectionInput) {
    const introductions = DEMO_CHARACTER_PRESENTATIONS[characterId]?.introductions;
    if (!introductions) throw new Error('Unknown demo character');
    if (selection && selection.character_content_revision_id !== `${characterId}-content-r3`)
        throw new Error('Stale demo greeting revision');
    const id = selection ? selection.greeting_id : `${characterId}-greeting-default`;
    if (id === null) return null;
    const opening = introductions[id];
    if (!opening) throw new Error('Unknown demo greeting');
    return opening.body;
}

const replies: Record<string, string[]> = {
    'character-noa': [
        '노아는 연필을 내려놓고 당신의 이야기를 끝까지 들었다.\n\n“그 장면은 어떤 색이었을까요? 꼭 눈에 보이는 색이 아니어도 괜찮아요.”\n\n창밖에서 바람이 불자 아직 마르지 않은 그림 옆의 커튼이 작게 흔들렸다.',
        '“좋아요. 그 부분은 빈칸으로 남겨 두죠.” 노아가 스케치북을 조금 더 당신 쪽으로 밀었다.\n\n“나중에 떠오르면 그때 채워도 되니까요. 지금은 가장 기억나는 한 장면부터 이야기해 주세요.”',
        '노아가 작게 웃었다. “다음에 그곳을 그릴 때는 오늘 이야기도 같이 넣어야겠네요.”\n\n테이블 위로 빛이 조금 더 길어졌다. 화실의 시계는 여전히 느리게 가고 있었다.',
    ],
    'character-aria': [
        '아리아는 책갈피를 내려놓았다.\n\n“그 이야기와 닮은 기록이 있어요. 다만 같은 결말이라는 뜻은 아니랍니다.”\n\n그녀가 꺼낸 책의 첫 장에는 아직 아무것도 쓰여 있지 않았다.',
        '“천천히 생각해도 괜찮아요.” 아리아가 따뜻한 찻잔을 당신 앞으로 옮겼다. “기억은 기다려 준 사람에게 조금 더 선명해지는 법이거든요.”',
    ],
    'character-kai': [
        '카이는 손에 묻은 기름을 닦고 라디오의 뒷판을 닫았다.\n\n“그럴 수도 있겠네.” 잠시 생각하던 그가 작은 메모를 건넸다. “여기부터 확인해 보자. 혼자 가는 것보단 낫겠지.”',
        '“서두르면 놓치는 게 있어.” 카이가 작업등을 조금 낮췄다. 무심한 말투와 달리 당신이 앉을 자리에는 이미 깨끗한 천이 깔려 있었다.',
    ],
    'character-sera': [
        '세라가 항해 지도에 작은 표시를 남겼다.\n\n“그 방향으로 가 보죠. 돌아올 길도 같이 기록해 둘게요.”\n\n창밖의 푸른 별이 조금씩 가까워지고 있었다.',
        '“그 이야기는 항해 일지에 적어도 될까요?” 세라가 웃으며 물었다. “숫자와 좌표만으로는 오늘을 전부 남길 수 없을 것 같아서요.”',
    ],
};

export function demoReply(characterId: string, turn: number) {
    const choices = replies[characterId] ?? [];
    return choices[Math.max(0, turn) % choices.length] ?? 'This is a local preview response.';
}
