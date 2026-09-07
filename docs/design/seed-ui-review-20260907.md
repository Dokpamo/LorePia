# SEED Design과 LorePia UI 비교

검토일: 2026-09-07 · 대상: 새 `LorePia UI` Tauri 미리보기

> 이 문서는 수정 전 진단을 보존한 기록이다. 이후 반영 내용과 재검증 결과는 [SEED 보완 결과](seed-ui-followup-20260907.md)를 참고한다.

## 판단

현재 프레임을 유지하면서 **입력 보존, 행동 결과 안내, 크기별 반응, 작은 화면의 가독성**을 먼저 보완하는 것이 좋다. 이미 공통 크기·간격과 입력바 동작이 구현돼 있지만, 화면과 상태가 바뀔 때 같은 규칙을 적용하지 못하는 부분이 남아 있다.

SEED에서 가져올 핵심은 컴포넌트의 모양뿐 아니라 기본·선택·누름·로딩·오류·이탈까지 반복 가능한 규칙으로 정의하는 방식이다. 우리에게 필요한 규칙을 Svelte UI에 적용할 수 있다. [SEED 소개](https://seed-design.io/get-started), [SEED 개편 과정](https://seed-design.io/updates/how-seed-evolved)

## 검토 범위와 근거

- LorePia 기준 커밋: `6f22761a4305443541452e7409639114f4c10dfd`. 이번 피스타치오 CSS 변경을 포함한 작업 트리를 검토했다.
- 공식 SEED 문서 저장소 기준: [`9b33a483ae4b8a49ef141b6fd92f297444d3ac6a`](https://github.com/daangn/seed-design/tree/9b33a483ae4b8a49ef141b6fd92f297444d3ac6a/docs/content).
- 공식 저장소의 MDX 문서 **255개 목록**을 확인했다. Foundations 24개, Components 63개, Patterns 2개의 공통 가이드 전 범위와 시작 안내, 업데이트 글, 관련 구현 문서를 읽었다. 이 수에는 인덱스와 폐기된 컴포넌트 안내가 포함된다.
- React 110개·Lynx 40개 문서는 목록을 확인하고 상호작용 상태·축소 피드백·반응형·테마·도움말 등 관련 문서를 선택해 대조했다. 모든 API 예제, 생성된 전체 아이콘·토큰 사전, 외부 Figma 자료를 완독한 것은 아니다.
- 코드 검토와 macOS Tauri 앱 조작을 병행했다. 모바일 실기기, VoiceOver/TalkBack, APCA, 프레임 시간 프로파일링은 이번 검토에 포함하지 않았다.
- 지적 대상은 `ui-preview.html`에서 실행되는 새 미리보기다. 미리보기에 없는 실제 서버·저장 상태를 기존 Rust/라이브 앱에도 없는 기능으로 판단하지 않았다.

## 우선순위

| 순서 | 보완점 | 사용자가 겪는 문제 | 근거 종류 |
| --- | --- | --- | --- |
| 1 | 설정 변경 보존과 이탈 처리 | 편집 완료 후 뒤로가면 입력이 사라짐 | 앱에서 재현 + 코드 |
| 2 | 복사 결과를 화면에 표시 | 눌렀는지, 실패했는지 알기 어려움 | 코드 |
| 3 | 좁은 화면의 최소 터치·글자 크기 | 화면과 함께 조작 대상·텍스트도 작아짐 | 크기 계산 |
| 4 | 버튼 크기에 맞춘 누름 피드백 | 아이콘·카드·행의 눌리는 느낌이 다름 | CSS |
| 5 | 배경별 보조 텍스트 대비 | 연한 배경 위 작은 회색 글씨가 흐림 | 색상 계산 |
| 6 | 검색 입력과 결과 연결 | 검색 중 결과를 보려면 편집 화면을 닫아야 함 | 컴포넌트 구조 |
| 7 | 낯선 아이콘의 의미 안내 | 분기 등 도구의 기능을 눌러보기 전 알기 어려움 | 컴포넌트 구조 |
| 8 | 실제 데이터 상태별 화면 규칙 | 정상 샘플 이외의 상황에서 완성도가 달라질 수 있음 | 미리보기 범위 |

### 1. 설정에서 나갈 때 작성 내용을 보호해야 한다

**재현:** 카드 설정 → 캐릭터 이름 → 전체화면에서 이름 수정 → 체크 버튼 → 설정 화면의 뒤로가기. 별도 안내 없이 원래 이름으로 돌아왔다. 수정한 이름은 저장되지 않았다. 검토용 변경은 저장하지 않았고 원래 캐릭터·테마로 복원했다.

설정은 이름·소개 등을 로컬 상태로 관리하고, 별도 저장 버튼을 눌러야 캐릭터에 반영한다. 반면 전체화면 편집기의 체크와 뒤로가기는 모두 편집기를 닫으며, 사용자는 체크를 저장 완료로 받아들일 수 있다. 설정 페이지의 뒤로가기에는 변경 여부 검사가 없다. [SettingsPage.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/SettingsPage.svelte:45), [TextEditor.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/TextEditor.svelte:90), [UiPreview.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/UiPreview.svelte:197)

**제안:** 전체화면 편집 방식은 유지한다. 필드의 ‘편집 완료’와 설정 전체의 ‘저장’을 구분하고, 저장 전 이탈에는 임시 저장 또는 변경 폐기 확인을 적용한다. 버튼·스와이프·Escape가 같은 처리 경로를 사용해야 한다. 변경이 없을 때는 바로 돌아간다. SEED Field도 저장되지 않은 변경의 이탈 보호와 폼 간 일관성을 다룬다. [Field](https://seed-design.io/components/field), [Top Navigation](https://seed-design.io/components/top-navigation)

**검증 기준:** 변경 후 이탈해도 입력을 회수할 수 있고, 변경하지 않은 화면에서는 불필요한 확인이 뜨지 않는다.

### 2. 메시지 복사 성공·실패가 일반 화면에 보이지 않는다

클립보드 성공·실패 문구는 준비돼 있지만, 출력 위치가 `ui-sr`인 `role="status"` 하나다. 이 클래스는 1px 영역으로 잘라 숨긴다. 화면 낭독기를 위한 안내는 있으나 시각적 결과 안내가 없다. [ChatMessage.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ChatMessage.svelte:33), [ui-content.css](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ui-content.css:141)

**제안:** 입력바 위에 짧은 ‘복사했어요’ 안내를 보여주고 기존 접근성 안내를 유지한다. 실패에는 다시 시도할 수 있는 설명을 제공한다. 안내는 하나씩 표시하고 키보드·입력바를 가리지 않도록 한다. SEED Snackbar는 이런 일시적인 행동 결과에 해당한다. 중요한 오류는 해당 작업 위치에도 남겨야 한다. [Snackbar](https://seed-design.io/components/snackbar)

**검증 기준:** 클립보드 허용·거절 모두 화면에서 결과를 확인할 수 있다.

### 3. 비율 축소와 최소 조작 크기를 함께 설계해야 한다

현재 폭이 360px 미만이면 360px짜리 UI 전체를 축소한다. 따라서 레이아웃뿐 아니라 텍스트와 버튼의 실제 크기도 함께 줄어든다. 사용자가 요청한 비율 유지의 결과이며, 접근성 목표와 조정할 지점이다. [responsive-layout.ts](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/responsive-layout.ts:20), [ui-tokens.css](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ui-tokens.css:40)

| 항목 | 기본값 | 폭 320px에서 계산한 값 |
| --- | --- | --- |
| 버튼 터치 영역 | 48px | 약 42.7px |
| 본문 | 17px | 약 15.1px |
| 보조 캡션 | 13px | 약 11.6px |

SEED는 여건이 허락할 때 44×44px 조작 영역을 권장한다. 42.7px를 곧바로 WCAG 위반이라고 판단하는 것은 부정확하지만 권장 목표보다 작아진다. 위 수치는 코드 계산이며 모바일 실기기 측정값은 아니다. [Inclusive Design](https://seed-design.io/foundations/inclusive-design)

**제안:** 좌측 관리·중앙 채팅·우측 서브페이지 연결과 바깥 여백 없는 구성을 유지하면서, 좁은 폭에서는 패널·문장 배치를 조정하고 터치 영역·최소 글자 크기를 별도로 보장한다. 기기마다 다른 화면비를 하나로 고정하기보다 주어진 폭과 높이에 맞춰 콘텐츠를 배치한다. 이는 현재 비율 동작을 바꾸는 제안이므로 이번 색상 작업에 섞어 적용하지 않았다. [Layout](https://seed-design.io/foundations/layout), [Typography](https://seed-design.io/foundations/typography)

### 4. 누름 애니메이션의 기준이 컴포넌트마다 다르다

일반 버튼은 0.98배, 아이콘 버튼은 0.9배, 캐릭터 카드는 0.96배로 줄어든다. 누를 때 70ms, 돌아올 때 160ms이며 전환 속성에는 `transform`만 들어 있다. 48px 아이콘 버튼의 시각 영역은 4.8px 줄고 같은 폭의 캐릭터 카드는 1.92px 줄어드는 차이가 생긴다. 배경색 변화와 축소의 시간도 연결돼 있지 않다. [ui-motion.css](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ui-motion.css:44)

SEED는 컨트롤 크기를 반영해 축소량을 계산하고 색·축소 반응을 함께 시작하도록 정의한다. 같은 비율을 모든 크기에 적용하면 체감 강도가 달라지기 때문이다. SEED의 150ms와 계산식은 우리 사용감을 조정할 출발점으로 참고할 수 있다. [Feedback](https://seed-design.io/foundations/feedback), [Scale](https://seed-design.io/foundations/feedback/scale)

**제안:** 공통 누름 규칙을 만들고 실제 조작 영역은 고정한다. 즉시 반응을 유지하면서 크기에 맞는 작은 축소와 표면색 변화를 동기화한다. 모션 줄이기에서는 축소를 끄고 표면색으로 반응한다. 현재 모션 줄이기는 전체 시각 요소의 투명도를 0.65로 바꿔 글자·아이콘까지 흐리게 한다. [Color Feedback](https://seed-design.io/foundations/feedback/color)

이 차이는 사용감의 일관성 문제다. 이번 검토에서 프레임 시간은 측정하지 않았으므로 이를 현재 버벅임의 확정 원인이라고 단정하지 않는다.

### 5. 작은 회색 텍스트의 대비가 배경에 따라 부족하다

밝은 테마의 날짜 표시는 `#6B7684` 글씨를 `#F2F4F6` 배경 위에 사용하며 기본 글자 크기는 13px이다. 이 조합의 WCAG 2 상대 휘도 대비는 약 **4.19:1**이다. 같은 글자색이라도 흰 배경에서는 약 4.62:1이므로 배경과 짝지어 관리해야 한다. [ui-chat.css](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ui-chat.css:28), [ui-tokens.css](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ui-tokens.css:17)

WCAG 2.2의 일반 텍스트 최소 대비는 4.5:1이다. 따라서 해당 날짜 조합은 기준보다 낮다. SEED의 APCA 기준과 이 대비 비율은 서로 다른 측정법이다. 이번에는 APCA 평가를 수행하지 않았다. [W3C Contrast Minimum](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html), [SEED Inclusive Design](https://seed-design.io/foundations/inclusive-design)

**제안:** `보조 텍스트 / 연한 표면`처럼 배경과 의미를 함께 정의하고 밝은·어두운 테마를 검사한다. 텍스트 대비 문제를 브랜드색 추가로 해결하지 않는다. [Color Roles](https://seed-design.io/foundations/color/color-role)

### 6. 검색할 때 결과를 함께 볼 수 있어야 한다

채팅 내역 검색은 전체화면 텍스트 편집기를 열고, 입력값은 뒤에 있는 목록을 필터링한다. 편집기에는 검색 결과가 없어서 결과를 확인하려면 닫아야 한다. 목록 화면에는 검색어 지우기 버튼과 결과 없음 문구가 이미 있다. [CharacterPage.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/CharacterPage.svelte:41), [TextEditor.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/TextEditor.svelte:95)

**제안:** 검색도 전체화면으로 열되 입력과 결과 목록을 같은 화면에 놓는다. 검색어 수정·지우기·결과 선택이 이어지게 하고, 결과 없음 화면에서 바로 검색어를 바꿀 수 있도록 한다. 이는 SEED의 결과 상태와 다음 행동 연결 원칙을 우리 검색에 적용한 제안이다. [Result Section](https://seed-design.io/components/result-section)

### 7. 낯선 도구는 아이콘 의미를 알려줘야 한다

공통 아이콘 버튼에는 `aria-label`이 있다. 그러나 시각적으로 보이는 도움말은 없고, 메시지 도구의 분기·재생성 같은 기능은 처음 사용하는 사람이 아이콘만 보고 구분하기 어렵다. 캐릭터 레일에는 이미 이름 `title`이 있으므로 레일 아래 이름을 다시 붙일 필요는 없다. [IconButton.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/IconButton.svelte:17), [ChatMessage.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ChatMessage.svelte:130)

**제안:** PC에서는 hover·키보드 focus에 짧은 도움말을 제공한다. 모바일의 낯선 작업에는 도구 메뉴 안에 동사형 이름을 제공한다. 현재의 배경 없는 SVG 버튼과 단순한 레일 형태를 유지할 수 있다. [Help Bubble Tooltip](https://seed-design.io/react/components/help-bubble-tooltip), [Menu Sheet](https://seed-design.io/components/menu-sheet)

### 8. 정상 화면 외의 상태를 공통 규격으로 확장해야 한다

새 미리보기에는 응답 대기·실패·취소·재시도, 생성 중 중지 버튼이 이미 있다. 앱 설정에도 샘플 연결과 창 안에서만 보관되는 데이터임을 표시한다. 실제 카드 가져오기, 최초 데이터 로딩, 네트워크·저장 실패까지 연결된 제품 상태 전체를 검증한 단계는 아니다. [ChatMessage.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ChatMessage.svelte:82), [MessageComposer.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/MessageComposer.svelte:130), [SettingsPage.svelte](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/SettingsPage.svelte:115)

**제안:** 기존 Core/컨트롤러가 제공하는 상태를 기준으로 처음 진입·불러오는 중·내용 없음·일부 성공·실패·취소·재시도의 화면을 정의한다. 콘텐츠 로딩에는 자리를 보존하는 표시를, 전송에는 즉각적인 접수 반응과 응답 대기를 구분한다. 알 수 없는 AI 진행률을 임의의 퍼센트로 표시하지 않는다. 실패 후에도 초안과 이미 생성된 내용을 회수할 수 있어야 한다. [Loading](https://seed-design.io/patterns/loading), [State](https://seed-design.io/foundations/state)

## 이미 갖춘 부분과 유지할 방향

- **규격:** 4px 단위 간격, 48px 기본 컨트롤, 24px 아이콘, 용도별 글자 크기가 토큰에 있다. 이후 추가되는 변형도 이 기준에서 파생해야 한다.
- **프레임:** 캐릭터와 채팅 내역, 대화, 선택형 서브페이지를 연결한 구조는 유지한다. SEED의 패널 구성을 참고하되 우리에게 없는 홈·하단 탭을 새로 만들 근거로 사용하지 않는다. [Side Panel](https://seed-design.io/components/side-panel)
- **입력바:** 두 줄부터 확장 기능을 제공하고 일반 높이에서는 열 줄까지 자동으로 늘어난다. 작은 높이에서는 화면을 가리지 않도록 제한한다. 대화별 초안, 전체화면 왕복, IME 입력을 다루는 코드와 테스트도 있다. 일반 폼 textarea의 권장 줄 수를 채팅 입력바에 일괄 적용하지 않는다. [composer-measure.test.ts](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/composer-measure.test.ts:49), [UiPreview.test.ts](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/UiPreview.test.ts:50)
- **채팅 도구:** 대화·스토리 모드, 메시지 도구 펼침, 분기, 삭제 전 확인이 있다. 삭제가 즉시 실행된다는 지적은 해당하지 않는다. [ChatInteractions.test.ts](/Users/codexer/lorepia/apps/lorepia/src/preview/ui/ChatInteractions.test.ts:62)
- **접근성 기반:** 버튼 이름, 키보드 포커스 복원, 비활성 배경의 `inert`, 제스처의 버튼 대안이 있다. 다음 검증은 작은 화면·큰 글자·화면 낭독기를 함께 사용한 실제 조작이다.
- **평평한 화면:** 여백과 표면색으로 그룹을 구분할 수 있다. SEED도 모든 경계에 그림자나 구분선을 요구하지 않는다. 현재 그림자 없는 방향을 유지할 수 있다. [Elevation](https://seed-design.io/foundations/elevation), [Divider](https://seed-design.io/components/divider)

## 피스타치오 적용 결과

전송·전체화면 전송·설정 저장 및 같은 종류의 주요 확정 버튼에 `#CDDEA4`를 적용했다. 버튼 안의 글자·아이콘은 `#30401C`, 누름 표면은 `#B2C686`이다. 기본 버튼 내부 대비는 약 **7.77:1**이다. 선택된 캐릭터·채팅 내역은 중립 회색이며, 일반 아이콘·배경·레이아웃·모션 시간은 기존 규칙을 유지했다.

SEED Action Button은 브랜드색을 핵심 행동에 제한하고, 중립색으로 행동의 우선순위를 표현하도록 안내한다. 피스타치오를 모든 선택·아이콘·말풍선에 확대할 이유는 없다. [Action Button](https://seed-design.io/components/action-button), [Action Button 개편 배경](https://seed-design.io/updates/whats-new-in-action-button)

변경 파일은 `ui-tokens.css`, `ui-composer.css`, `ui-motion.css` 3개다. 이번 비교에서 나온 기능 보완 제안은 아직 구현하지 않았다.

## 실행 순서와 검증

1. **작성 보존·결과 안내:** 설정 이탈 규칙과 보이는 복사 결과부터 정리한다. 성공·실패·뒤로가기·제스처를 같은 시나리오로 검증한다.
2. **작은 화면·가독성:** 320/360/393/430px와 두 칸·세 칸 화면에서 터치 크기, 큰 글자, 긴 제목, 키보드 표시를 검증한다. 테마별 텍스트·배경 대비를 함께 검사한다.
3. **누름 규칙:** 아이콘·카드·긴 행을 나란히 비교해 반응을 맞춘다. 축소가 조작 영역과 배치를 움직이지 않는지, 모션 줄이기가 유지되는지 확인한다. 버벅임은 실제 프레임 시간을 측정해 판단한다.
4. **검색·도구 안내:** 전체화면 검색 안에서 입력→결과→대화로 이어지게 하고 낯선 도구의 이름을 제공한다.
5. **실제 상태 연결:** 기존 Rust 계약과 프런트엔드 컨트롤러를 기준으로 상태별 예시 화면을 갖춘다. 같은 컴포넌트의 밝은·어두운·누름·로딩·오류·긴 텍스트 상태를 비교할 수 있는 작은 검증 화면이 도움이 된다.

이번 색상 변경 검증: 프런트엔드 검사 오류 0개(기존 `ChatPane.svelte` 경고 2개), 미리보기 통합·반응형 테스트 15개 통과, Lua·정규식 후속 검사 통과, Tauri UI 빌드 성공. 새 앱에서 밝은·어두운 테마의 주요 버튼과 중립 선택 표시를 확인했다. 전체 Rust/크로스플랫폼 게이트나 위 제안들의 구현 완료를 의미하지 않는다.
