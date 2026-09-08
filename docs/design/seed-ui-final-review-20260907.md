# SEED Components / Foundations 추가 검토

2026-09-07 · 검토 ID `UI-SEED-REVIEW-20260907-FINAL`

후속 상태: 아래는 수정 전 검토 기록이다. 1–4번은 [수정 및 검증 기록](seed-ui-fixes-20260907.md)에 따라 보완했다. 5번의 폴더 묶기 버튼 추가는 사용자 요청으로 제외했다.

현재 미리보기에서 추가로 확인한 보완점은 하단 요소의 겹침, 편집 적용 시점, 빈 값 검증, 다크 모드 보조 글자의 대비, 폴더 기능의 진입 경로다. 기존 세 화면 구조와 중립 선택 표면을 유지하면서 해결할 수 있는 항목이다.

## 검토 범위

[Components](https://seed-design.io/components), [Foundations](https://seed-design.io/foundations)를 다시 확인하고 Field, Input Button, Dialog, Snackbar, Contextual Floating Button, Inclusive Design, State, Layout, Typography, Motion을 현재 코드와 대조했다. 사이트의 모든 플랫폼 API를 전부 검증했다는 의미는 아니다.

기준 커밋은 `6f22761a4305443541452e7409639114f4c10dfd`이며 기존 작업 트리의 변경을 포함한 `ui-preview.html`을 검토했다. 별도 브라우저의 394×854px 화면에서 메모리 샘플만 조작했다. 현재 Tauri 창의 대화와 작성 중인 초안은 보존했다. 이번 작업은 검토 문서만 추가하며 UI·Rust·IPC·의존성을 변경하지 않는다.

앞서 보완한 설정 변경 폐기 확인, 복사 결과 안내, 검색 결과, 응답 상태, 공통 누름 피드백은 누락 사항으로 다시 집계하지 않았다. 이 문서는 [이전 보완 기록](/Users/codexer/lorepia/docs/design/seed-ui-followup-20260907.md)의 후속 검토다.

## 1. 하단 알림이 최근 메시지 버튼을 덮는다 — 재현, 우선 수정

긴 대화에서 위로 스크롤한 뒤 메시지 도구의 복사를 누르면 알림과 ‘최근 메시지로 이동’ 버튼이 같은 위치에 나타난다.

| 394×854px 측정 | 위치와 크기 |
| --- | --- |
| 최근 메시지 버튼 | x=173, y=717, 48×48px |
| 복사 안내 | x=20, y=693, 354×64px |
| 겹치는 세로 길이 | 40px |

버튼 중심인 (197, 741)의 실제 클릭 대상도 알림이었다. 성공 알림이 사라질 때까지 버튼 대부분이 가려진다. 실패 알림은 닫기·재시도 전까지 남도록 구현되어 있으므로 같은 배치 문제가 더 오래 지속될 수 있다. 실패 상태에서의 겹침은 이번 브라우저에서 직접 재현하지 않았다.

원인은 [최근 메시지 버튼 위치](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ui-chat.css:117)와 [알림 위치](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ui-feedback.css:61)가 각각 입력바 높이만 기준으로 계산되기 때문이다.

**보완 방향:** 입력바, 최근 메시지 버튼, 알림이 서로 차지하는 높이를 공유해 순서대로 배치하고 알림 등장·퇴장 때도 간격을 유지한다. SEED는 떠 있는 버튼 주변의 고정 요소와 여백을 확보하고, 알림과 하단 액션의 위치 관계를 정의한다. [Contextual Floating Button](https://seed-design.io/components/contextual-floating-button), [Snackbar](https://seed-design.io/components/snackbar).

## 2. 메시지 편집의 완료·뒤로가기 의미가 불분명하다 — 재현, 우선 수정

‘비 오는 오후’의 두 번째 사용자 메시지를 편집해 ‘검토용 임시 수정’으로 바꾸고, 체크 표시의 ‘편집 완료’ 대신 왼쪽 뒤로가기를 눌렀다. 수정된 메시지가 남고 ‘분기 1’이 생성됐으며 뒤에 있던 응답은 새 분기에서 제외됐다. ‘원래 대화’를 선택하면 기존 메시지와 응답은 그대로 남아 있었다.

현재 [전체화면 편집기](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/TextEditor.svelte:33)는 입력할 때마다 콜백을 실행하고, [메시지 편집 콜백](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/sample-chat.svelte.ts:59)은 첫 변경 때 분기를 만든다. 완료와 뒤로가기는 모두 편집기를 닫는다. 설정 필드에 있는 ‘설정 화면에서 저장해요’ 같은 적용 방식 안내도 메시지 편집에는 없다.

**보완 방향:** 메시지 편집은 임시 값을 다듬은 뒤 완료할 때 분기를 만들도록 정리하고, 변경 후 뒤로가기에는 계속 수정·변경 폐기를 제공하는 편이 명확하다. 자동 적용을 유지한다면 그 사실과 원래 대화로 돌아가는 방법을 표시해야 한다. 이는 자동 저장 자체가 SEED 위반이라는 뜻이 아니다. SEED도 자동 저장이 있는 경우 이탈 경고를 생략할 수 있다고 설명한다. [Field](https://seed-design.io/components/field).

## 3. 공백 이름을 저장하면 설명 없이 원래 값으로 돌아간다 — 재현, 우선 수정

카드 설정 → 캐릭터 이름 → 공백 세 칸 입력 → 편집 완료 → 저장을 누르면 설정이 닫히고 이름은 ‘서연’ 그대로다. 오류나 필수 입력 안내는 나타나지 않는다.

[저장 처리](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/UiPreview.svelte:205)가 공백을 제거한 값이 비어 있으면 기존 이름을 유지한다. 채팅방 제목에도 같은 처리가 있다. 제목은 코드로 확인했고 공백 저장을 별도로 조작하지 않았다. 새 캐릭터·새 대화 생성 시 기본 이름을 사용하는 동작과는 구분한다.

**보완 방향:** 필수 값 여부를 표시하고, 공백 값에는 ‘이름을 입력해 주세요’처럼 고칠 방법을 해당 필드에 표시한다. 완료·저장 버튼과 전체화면 편집기가 같은 검증 결과를 사용하도록 한다. [Field](https://seed-design.io/components/field), [Inclusive Design](https://seed-design.io/foundations/inclusive-design).

## 4. 다크 모드의 작은 보조 글자가 SEED의 대비 기준에 못 미친다 — 측정

이번에는 이전 WCAG 상대 휘도 계산에 더해 APCA를 계산했다. 브라우저의 실제 글자·배경색을 읽고 공식 APCA 0.0.98G 구현의 `APCAcontrast`와 `sRGBtoY`로 불투명 sRGB 조합을 계산했다. 아래는 극성 부호를 제외한 Lc 절댓값이다. [계산 구현](https://github.com/Myndex/apca-w3/blob/master/src/apca-w3.js).

| 실제 조합 | 크기 / 굵기 | APCA 절댓값 |
| --- | --- | --- |
| 다크 채팅 날짜 `#9E9EA4` / `#252528` | 13px / 400 | 47.08 |
| 다크 선택 내역 날짜 `#9E9EA4` / `#303034` | 13px / 400 | 44.85 |
| 다크 캐릭터 소개 `#9E9EA4` / `#17171A` | 15px / 400 | 49.00 |
| 다크 채팅 본문 `#E4E4E7` / `#252528` | 17px / 400 | 87.58 |
| 밝은 채팅 날짜 `#596574` / `#F2F4F6` | 13px / 400 | 72.96 |
| 밝은 선택 내역 날짜 `#596574` / `#E9ECEF` | 13px / 400 | 68.18 |
| 주 버튼 `#30401C` / `#CDDEA4` | 색상 토큰 조합 | 72.44 |

SEED는 일반적인 보조 텍스트에 최소 Lc 60, 두 줄 이상 본문·입력 텍스트 등에는 최소 75를 제시한다. 작은 보조 텍스트의 굵기도 함께 고려한다. 따라서 다크 모드의 날짜·소개는 추가 보완이 필요하다. 이 결과를 WCAG 실패 판정과 혼동해서는 안 된다. [Inclusive Design](https://seed-design.io/foundations/inclusive-design).

**보완 방향:** [공통 `--ui-muted`](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ui-tokens.css:22)를 보조 콘텐츠와 입력 힌트의 역할로 나눠 명도·굵기를 조정한다. 피스타치오 주 버튼은 이번 색상 계산에서 일반 버튼 텍스트 대비 기준을 넘으므로 브랜드색 변경은 필요하지 않다. rem 기반 글자 크기는 이미 적용되어 있다. [Typography](https://seed-design.io/foundations/typography).

## 5. 처음 폴더를 만드는 단순 탭 경로가 드러나지 않는다 — 코드·키보드 경로 확인

카드 정리 화면 자체는 있다. 실제로 카드에서 Shift+F10 → ‘하루 카드와 묶기’를 눌러 폴더를 만들 수 있었다. 다만 첫 화면과 카드 설정에는 정리 메뉴로 들어가는 일반 버튼이 없고, 폴더를 만든 뒤에야 ‘폴더 설정’ 버튼이 나타난다.

[캐릭터 레일](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/CharacterRail.svelte:122)의 진입점은 우클릭·ContextMenu·Shift+F10이다. 터치는 [300ms 유지 후 드래그](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/card-drag.ts:94)로 이어지며 드래그 중에는 문맥 메뉴도 억제한다. 모바일 단순 탭만으로 첫 폴더를 만드는 경로는 확인되지 않았다. 실제 iOS·Android 길게 누르기와 보조 기술 동작은 아직 검증하지 않았다.

**보완 방향:** 기존 카드 설정 안에 ‘폴더로 묶기 / 폴더로 이동’을 제공해 현재 카드 정리 화면으로 연결한다. 드래그와 같은 결과를 몇 번의 탭으로도 얻을 수 있게 한다. SEED의 단순 조작 대안과 기능 발견성 기준에 해당한다. [Inclusive Design](https://seed-design.io/foundations/inclusive-design).

## 적용 순서와 남은 확인

하단 겹침 → 메시지 편집·빈 값 검증 → 다크 모드 보조 글자 → 폴더 진입 경로 순서를 권장한다. 입력바·도구 확장의 연결 모션은 이 수정 후 펼침·접힘·알림 등장 상태를 함께 확인해야 한다. SEED의 모션 기준은 전환과 누름 같은 동작의 역할을 구분한다. [Motion](https://seed-design.io/foundations/motion).

모바일 소프트 키보드, 큰 시스템 글자, VoiceOver·TalkBack, 제스처 취소와 OS 뒤로가기 충돌은 실기기 또는 시뮬레이터에서 이어서 검증해야 한다. 이번 브라우저 조작 중 오류 로그는 없었다. 소스 수정이 없어 테스트·빌드는 다시 실행하지 않았다. UI 소스 61개 파일의 검토 전후 해시를 비교해 변경이 없음을 확인했다.
