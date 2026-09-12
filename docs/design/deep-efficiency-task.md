# DEEP-EFFICIENCY-20260912

- 사용자 요청: Astra 병렬 조사로 반복 보안 검증·자원 누적·비효율 구조를 찾아 수정한다.
  추가로 1,000턴 이상 대화의 점진 조회와 장기 사용 비용을 우선 확인한다.
- baseline / main merge-base: `aac3b16e0697c710b1209f498caba71821746140`.
- branch: `codex/ux-responsiveness-20260912`.
- worktree: `/Users/codexer/.codex/worktrees/lorepia-ux-20260912`.
  이전 화면·UX·resource 작업이 미커밋 상태다. 이 상태의 1,854개 파일을
  `/tmp/lorepia-deep-efficiency-20260912/baseline-files/`에 복사하고 해시/diff를 기록했다.
  별도로 진행 중인 원본 작업 트리의 변경은 가져오거나 수정하지 않는다.

## 조사 및 변경 소유권

| 담당 | 범위 | 검증할 조건 |
| --- | --- | --- |
| Astra Storage | JSON 검증/직렬화 중복, runtime cache, 분기 message page | 검증·오류·hash 유지, 한 snapshot의 lineage, 선택된 content만 읽기 |
| Astra Frontend | 반복 context 계산, stream buffer, 화면 history와 최신 runtime context 분리 | epoch/순서/취소, scroll anchor, 유한 window, background context 완료 |
| Astra Assets/Runtime | native PNG CRC, 부분 history의 portable runtime | 모든 CRC 검사 유지, 원본 bytes 보존, 절대 index/override 보존 |
| 부모 | Core/Shell/IPC page 계약, 통합 검증 및 결과 | Core-owned presentation, 기존 API 호환, strict DTO, 양쪽 capability/registry 일치 |

사전 `rg` 조사로 현재 `list_branch_messages`가 content를 포함한 전체 recursive lineage를
조회하고, UI는 DOM만 최대 80개 가상화하며 전체 DTO를 보유한다는 점을 확인했다.
따라서 이 요청은 단순 refactor가 아니라 bounded history 조회 기능 추가를 포함한다.

## 메시지 페이지 계약

새 `list_branch_messages_page` IPC는 branch, 배타적인 before/after message ID와
1–128개 limit을 받는다. 무 anchor는 최근 페이지, before는 이전, after는 가까운 다음
페이지다. 반환은 chronological 순서의 messages, has_older/has_newer,
head_message_id, total_messages, start_index다. offset은 0부터 시작하며 원래 전체
lineage의 메시지 번호를 유지한다. 기존 전체 목록 API와 AI prompt 선택은 유지한다.

Storage는 현재 분기의 ancestor membership과 conversation 일치를 확인하고, 같은
DB read snapshot에서 페이지와 head/count를 얻는다. 다른 분기의 임의 anchor는 거절한다.
현재 schema에는 persisted lineage index가 없어 identity-only 탐색은 길이에 따라
증가할 수 있다. 다만 전체 content 복사/DTO/해시/화면 보유를 없애는 것이 이번 변경의
명확한 목표다. index migration이나 인증 cache를 새로 만들지 않는다.

UI 초기 목표는 30개 메시지(일반적인 사용자/AI 왕복 약 15턴), 가까운 스크롤에서
추가 30개 조회, 화면용 보유 window 약 90개다. 카드 런타임의 최근 128개/512KiB
문맥은 별도로 준비한다. 화면 window 밖으로 나간 것은 삭제된 메시지가 아니며,
portable override를 지우거나 전역 메시지 번호를 바꾸는 근거가 될 수 없다.

최종 교차 검토에서 실제 삭제된 override도 부분 window 밖에서는 남는 반례를 발견했다.
이를 해결하는 좁은 계약 확장으로 선택적인 `check_message_ids`(최대 256개)와
`retained_message_ids`를 추가한다. 이미 읽는 identity snapshot에서 후보의 현재 분기
membership만 확인하고 후보 순서를 보존한다. 요청하지 않으면 반환 필드도 생략한다.
명시적 삭제 이후의 상태 정리에 사용하며 전체 본문 재조회나 새 schema를 요구하지 않는다.

전체 배열에서 계산하던 카드용 마지막 assistant 문구도 보존한다. 가져온 대화에서
최근 128개가 모두 user/system이면 더 이전의 assistant 한 개가 필요하므로,
runtime tail 요청에만 `include_last_assistant` 선택 옵션을 사용한다. 출력의
`last_assistant_message`는 현재 분기의 가장 최근 assistant 한 개이며 기존 표시 검증을
거친다. 화면·worker 기록 배열을 확장하지 않고 UI 문구용 aggregate로만 보유한다.

## 수정 전 조건·검증

- 지배 자료: root, Storage, Core, Frontend AGENTS 및 ADR 0001–0006.
- 대상 entry: Storage `list_branch_messages_page`, Core의 새 presentation page,
  Shell API DTO/method, strict Tauri handler, IPC client와 root controller.
- 기존 symbols/public entry는 이동·rename하지 않는다. 새 private helper는 소유 모듈에 둔다.
- 새 공개 선언은 `config/core-storage-public-api-baseline.json`에 명시적으로 갱신하고,
  IPC manifest/생성물/handler/dev·release capability/permission을 함께 검토한다.
- 예상 크기: pagination과 회귀 검증으로 새 bounded 모듈 수 KiB–수십 KiB 증가.
  기존 giant file cap을 올리지 않으며 archived refactoring 자료는 수정하지 않는다.
- 위험: 잘못된 branch cursor, 중간 append/delete/fork, 일부 history를 전체로 오인,
  오래된 async 결과, 스크롤 점프, 페이지 eviction과 실제 삭제 혼동, 무한 prefetch.
- baseline Shell collection test와 각 담당의 표적 baseline을 실행한다. 변경 후
  2,000개 synthetic messages, 양방향 page, fork/foreign anchor, projection tamper,
  반복 스크롤/분기 전환/runtime context/stream race를 검증하고 통합 gate를 실행한다.
- 사용자 DB/이미지는 측정이나 검증에 사용하지 않는다. 보안 검증·durable phase·fsync·
  secret lifetime을 제거하지 않고, 같은 의미의 계산과 데이터 이동 비용을 줄인다.
