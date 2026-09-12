# 장기 사용·메시지 누적 비용 개선

Task: `DEEP-EFFICIENCY-20260912` · branch: `codex/ux-responsiveness-20260912`.
기준은 HEAD `aac3b16e0697c710b1209f498caba71821746140`에 이전 UX·자원 개선이
미커밋 상태로 적용된 시점이다. [사전 범위와 계약](deep-efficiency-task.md)에
별도 스냅샷, 담당 경계와 검증 조건을 기록했다. Astra 세 에이전트가 Storage,
프런트, 자산·런타임을 병렬로 수정하고 상대 경로를 교차 검토했다.

## 1,000턴 채팅은 어떻게 달라졌는가

기존에는 DB에서 현재 분기의 **전체 메시지 본문을 읽어 프런트 상태에 보관**했다.
화면 DOM만 최대 80개로 가상화했으므로, 보이지 않는 메시지도 본문·DTO 메모리와
표시 검증 비용을 발생시켰다. 이전부터 15턴씩 DB 페이지를 읽던 구조는 아니었다.

| 단계 | 변경 후 상한·동작 |
| --- | --- |
| 최초 화면 | 최근 30개 메시지. 일반적인 사용자·AI 왕복 약 15턴 |
| 이전·다음 기록 | 보유 영역 끝 640px 이내에서 30개씩 조회 |
| 화면용 메시지 상태 | 최대 90개. 반대쪽 페이지를 제거하고 스크롤 기준 메시지를 유지 |
| 실제 메시지 DOM | 기존 최대 80개 가상화 유지 |
| 카드 런타임 입력 | 별도로 최근 128개 준비. 첫 화면은 먼저 표시하며 준비 완료 후 런타임 사용 |
| 런타임 직렬화 문맥 | 기존 512KiB 상한 유지. 메시지 번호는 전체 대화 기준 |
| 카드용 마지막 AI 문구 | 최근 window 밖에 필요한 답변이 있을 때 한 개만 보충. 일반 대화 중복 본문 없음 |
| DB 본문 페이지 | 요청당 1–128개. 선택된 메시지의 표시 데이터만 읽고 검증 |
| 스크롤 요청 | 프런트 동시 1개, 반복 최신 이동 합치기, 실패 후 명시적 재시도 |
| Native DB 작업 | UI 실행 흐름 밖의 blocking worker, 실행 최대 2개·실행 포함 접수 최대 8개 |

2,000개 메시지가 각각 1KiB 본문인 예에서 첫 화면 조회의 본문 행 수는
2,000개에서 30개로 줄고, 본문 데이터는 2,048,000바이트에서 30,720바이트로
줄어든다. 이후 런타임용 최근 128개 조회는 별도다. 이는 정해진 fixture의 행 수와
본문 크기 비교이며 앱 전체 메모리나 실제 사용자 지연 시간이 98.5% 감소했다는 뜻은 아니다.

현재 스키마에는 분기별 영구 순번 인덱스가 없어 **ID와 부모 관계 탐색은 여전히
대화 길이에 비례한다**. 전체 본문·표시 sidecar·DTO 로드를 제거했지만, DB 총 작업이
완전히 일정해진 것은 아니다. 분기 공유·삭제·복구까지 다루는 순번 인덱스는 별도
마이그레이션과 측정이 필요한 후속 구조 개선이다. 이번에는 스키마를 변경하지 않았다.

## 함께 줄인 누적·반복 비용

| 경로 | 확인한 비용과 수정 |
| --- | --- |
| 스트림 reconciliation | 복구 대기 중 쓰기만 하고 읽지 않던 배열 제거. 1,000개 오래된 8KiB 이벤트가 남지 않는 회귀 검증 |
| Provider SSE | 이벤트마다 새 `Vec`를 만들던 복사를 기존 버퍼 borrow로 변경. 처리 완료 전 버퍼 변경은 Rust가 차단 |
| 런타임 문맥 | 각 메시지 JSON을 예산 계산과 결과 계산에서 두 번 만들던 작업을 한 번으로 통합 |
| lore 문맥 문자열 | 전체 기록을 join한 뒤 suffix를 잘라내던 작업을 필요한 suffix부터 수집하도록 변경 |
| Provider 설정 갱신 | 실제 비교가 필요 없는 credential refresh 경로의 이전 값 JSON 비교표 생성을 제거 |
| Portable state 저장 | 이미 검증·직렬화한 JSON 재사용. 저장 직전 같은 내용의 재직렬화 제거 |
| Module 문서 | 검증한 JSON Value 재사용. 동일 문서 재파싱 제거 |
| Module cache admission | 32MiB 적재 가능 여부만 확인하기 위한 전체 직렬화 버퍼를 counting writer로 대체 |
| Native PNG 검증 | CRC 검사 범위는 유지하고 slicing-by-8 계산 적용. 고정 읽기 전용 테이블 7KiB 증가 |

PNG CRC의 로컬 최적화 빌드 측정은 1MiB 입력 **2.096ms → 0.498ms**,
16MiB 입력 **34.044ms → 7.640ms**였다. CRC 계산 자체의 결과이며 파일 읽기,
SHA-256, 이미지 디코딩을 포함한 앱 전체 속도 수치가 아니다.

부분 기록 도입 후에는 화면에서 빠진 메시지를 삭제로 오인하지 않도록 했다.
동시에 명시적 삭제 후 오래된 override가 256-key 저장 한도까지 쌓이는 반례를
교차 검토로 발견했다. 새 페이지 계약은 최대 256개 후보 ID의 현재 분기 소속도
같은 ID snapshot에서 확인한다. 전체 본문을 다시 읽지 않고 실제 삭제된 상태만
정리할 수 있도록 이 증거를 런타임에 연결한다.

삭제 상태 정리는 같은 scope/head의 증거를 받은 뒤 적용하고 기존 저장 drain을
기다려 런타임을 재생성한다. 512회 반복 삭제, 늦은 응답, 초기 런타임 생성 중 삭제,
지연된 저장을 회귀 검증했다. context metadata 자체의 512KiB 초과도 거절하며,
비동기 표시 변환 후 scope를 다시 확인해 이전 방의 runtime 재게시를 막았다.
카드의 전역 메시지 번호와 마지막 AI 답변은 실제 iframe 테스트로 확인했다.

## 유지한 검증과 미해결 범위

자산의 no-follow open, 같은 handle 검증·읽기, ID 확인, SHA-256, PNG CRC,
경로·URL·archive·credential 경계는 유지했다. fsync, transaction, journal/recovery
순서도 변경하지 않았다. 실행 중인 DB의 WAL/SHM 파일을 쓰레기로 간주해 지우지 않는다.
이전 작업의 WAL journal 크기 제한과 짧은 썸네일 보유 정책은 그대로 유지된다.

Native 구독·작업 레지스트리에는 기존 32/16개 상한과 Drop 정리가 확인되었다.
조사한 경로에서 영구 map 누적의 근거는 찾지 못했다. 500ms supervisor polling을
이벤트로 대체하려면 durable job의 모든 깨우기 경로를 확인해야 하므로 이번에는
변경하지 않았다. 이 검토가 며칠간 실행한 메모리 soak test를 대체하지는 않는다.

원본 이미지의 손실 압축, 디스크 썸네일 저장소, 자동 VACUUM, 영구 분기 인덱스는
추가하지 않았다. 원본 보존·캐시 무효화·쓰기 비용까지 함께 검증할 설계가 필요하다.
전체 대화 호환 API는 유지하지만 live UI에서는 bounded page API를 사용한다.

## 검증 근거

로컬 검증 결과:

| 검사 | 결과 |
| --- | --- |
| Frontend 전체 | 149개 파일, 803개 테스트 통과. Lua timeout·regex worker 검사 통과 |
| Frontend check/build | format·i18n·ESLint·TypeScript·Svelte 통과, 오류·경고 0. 프로덕션 빌드 통과 |
| Providers 전체 lib | 400개 통과 |
| Storage 전체 lib | 312개 통과, 기존 1개 ignored. 이후 membership/aggregate 최종 페이지 7개 추가 표적 검증 통과 |
| Shell 전체 lib | 119개 통과. 이후 계약 확장 최종 페이지·projection 6개 표적 검증 통과 |
| Core vertical/retry/replay/smoke | 15개 통과 |
| Native 전체 lib | 201개 통과, 기존 2개 ignored. CRC·worker admission 포함 |
| Rust 형식·Clippy | 최종 `cargo fmt --all --check`, workspace all-targets `-D warnings` 통과 |
| Python 도구 회귀 | 102개 통과 |
| 계약·구조 검사 | IPC codegen, public API/source architecture, i18n, context, refactoring archive, workflow 검사 통과 |

전체 frontend 검증 중 새 비DOM 삭제 테스트의 대기 방식과 IPC 테스트의 unbound-method
lint를 수정한 후 위 전체 suite를 다시 통과했다. 크기 제한은 올리지 않았다.
페이지 상태·보완 계약이 추가되어 entry JS는 이전 자원 개선의 765.48kB에서
774.76kB(gzip 205.24kB)로 증가했다. 빌드의 기존 500kB chunk 경고는 남아 있다.
이 약 9.28kB 증가와 긴 대화의 본문·상태 보유 감소를 구분해야 한다.

직접 클릭 검증은 `qa-ux.html?fast=1&history=2000`의 합성 대화로 진행했다.
초기 DOM은 1971–2000번 30개였다. 위로 스크롤하며 1941, 1911, 1881번부터의
이전 페이지를 확인했으며 관찰한 DOM은 45–54개 수준으로 전체 2,000개가 아니었다.
다시 아래로 이동해 이미 제거된 최신 방향 페이지를 읽고 2000번까지 복귀했다.
1.8초 지연 fixture에서는 즉시 채팅 틀·로딩 상태를 표시했고, 이전 페이지를 받은 뒤
1974–1982번을 읽는 위치가 안정적으로 유지되었으며 최신 이동 버튼도 동작했다.
브라우저 warning/error 로그는 비어 있었다. 이는 UI 기능 검증이며 native DB·실제
이미지·며칠간 실행한 메모리/FPS 성능 측정으로 확대해 해석하지 않는다.

전체 Rust workspace 테스트와 다른 OS CI는 이번 로컬 표적·crate 검증으로 대체하지
않는다. 병합 전 저장소가 요구하는 전체 gate와 cross-platform 검증이 남는다.
변경은 작업 브랜치의 미커밋 상태이며 push/merge는 수행하지 않았다.

세부 자료:

- [프런트와 페이지 상태](deep-efficiency-frontend.md)
- [Storage와 SQL](deep-efficiency-storage.md)
- [Native CRC 측정](deep-efficiency-assets.md)
- [SSE 버퍼](deep-efficiency-provider-stream.md)
- [부분 기록 런타임](../deep-efficiency-runtime.md)
- [Native 수명 조사](deep-efficiency-native-lifecycle.md)
- 이전 [Toss UX 조사·적용](toss-ux-responsiveness-audit.md), [첫 자원 개선](resource-efficiency-audit.md)

실행 로그·fixture·독립 patch는 `/tmp/lorepia-deep-efficiency-20260912/`에 둔다.
