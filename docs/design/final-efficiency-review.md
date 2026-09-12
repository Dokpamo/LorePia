# 마지막 자원 효율 개선 검토

브랜치: `codex/final-resource-efficiency-20260912`
기준 커밋: `e32c13c5b04c3d5db1b60637be115c2b61fcfb78`

사용자가 제공한 Pro 보고서의 PERF-01~09를 기존 F01~F12 감사와
대조했다. 아래는 확인된 원인과 채택한 수정이다. 측정 수치는 합성
회귀 조건에서의 작업량이며, 실제 기기의 응답 시간·RSS·배터리 측정은 아니다.

## Pro 보고서와 비교

| 지적 | 판단과 구현 |
| --- | --- |
| PERF-01 페이지마다 전체 계보 재구성 | 채택. 본문을 보관하지 않는 단일 8 MiB 계보 캐시. head·로컬 변경 수·외부 data_version을 같은 snapshot에서 대조한다. 첫 조회/변경 후 재검증은 유지한다. |
| PERF-02 async worker의 DB 대기와 긴 DB 잠금 | 채택. 스트리밍 checkpoint·종료 변환/저장·Core 파생 작업·메모리 claim/준비를 최대 2개 blocking 작업으로 분리했다. 지식 벡터를 제한된 snapshot으로 읽은 뒤 DB 잠금 밖에서 검증·점수 계산한다. 모든 동기 API를 자동으로 비동기로 바꾼 것은 아니다. |
| PERF-03 요청마다 Client 재생성 | 채택. 정확한 URL/전체 정책/승인 주소 집합/timeout에 묶인 최대 32개 transport, 고정 60초 수명. 두 DNS 조회와 동일성·실제 peer·credential 검사는 유지한다. |
| PERF-04 Portable 문서 전체 재작성 | 채택. 실행 중 1개·최신 대기 1개로 합치고, 같은 문서와 권한 범위는 iframe·bridge ID를 유지한다. 최신 메시지 번호만 바뀌는 정적 카드도 재로드하지 않는다. |
| PERF-05 서로 다른 에셋의 전역 잠금 | 채택. 전역 map은 조회·퇴출·예산만 담당하고, 파일별 잠금으로 검증/읽기를 처리한다. 동일 파일의 동시 cold 요청은 한 번만 검증한다. |
| PERF-06 PNG 범위 요청의 전체 CRC/읽기 | 채택. cold 동일-handle 검증 결과에 native 전용 정책 버전을 부여한다. warm 요청은 필요한 범위만 읽으며, cold 전체 GET은 검증 버퍼를 재사용한다. |
| PERF-07 / F12 올바른 권한에도 chmod 반복 | 채택. 현재 권한이 정확할 때 syscall만 생략한다. 링크·소유 경계·특수 권한 비트 검사는 그대로 유지한다. |
| PERF-08 누적 스트리밍 본문 재작성 | 채택. 새 migration 0043의 순서/바이트 watermark가 있는 청크 저널에 새 suffix를 저장한다. 조회·길이 필터·취소·복구는 합쳐진 정확한 본문을 보고, 종료 시 원래 메시지 행으로 materialize하고 저널을 정리한다. |
| PERF-09 일시 검증 예산 부족을 누락으로 처리 | 채택. recoverable busy로 전달하고 공용 4-slot admission과 제한된 재시도를 사용한다. 대기/취소가 실제 native 작업 종료 전에 슬롯을 돌려주지 않는다. |

## 기존 감사에서 함께 수정한 항목

- F01: lifecycle idle 조회에 기존 partial index와 일치하는 predicate를 적용.
- F02: queued job마다 같은 과거 JSON 이력을 반복 읽지 않고 공통 통계를 계산.
- F03: summary에 필요한 구간만 body로 읽고, 수동 구간 선택은 ID/role만 사용.
- F04: 표시 전 실제 이미지 크기 검사. 최대 변 8192, 16,777,216픽셀,
  bounded header reader. 원본 파일은 그대로 유지.
- F05/F06: alias가 같은 asset을 가리키면 한 번만 resolve하고 전역 admission 사용.
- F08/F09/F11: worker 중복 직렬화, 반복 branch projection, 설정 fallback 조회 제거.
- F10: 빈/전송 완료 draft만 제거. 미전송 draft는 삭제하지 않음.
- 메시지별 화면 밖 미디어는 일시 정지하고 정리한다. 방 BGM은 유지한다.

## 1,000턴 이상 대화의 실제 로딩 방식

live 페이지 API는 대화 열기와 분기 전환 모두 최근 30개 메시지(보통
사용자·응답 15턴)를 먼저 표시하고, 별도 runtime 문맥을 준비한다.
이번 최종 대조에서 발견한 분기 전환의 128개 선조회도 같은 순서로 고쳤다.
처음 30개를 표시하는 동안 변경 동작은 기존 loading 상태로 제한하며,
오래된 분기의 늦은 결과는 초기 화면과 최종 문맥 모두 덮어쓰지 못한다.

스크롤이 위·아래 경계 640px 안에 들어오면 30개씩 조회한다. UI 본문은
최대 90개, 가상화된 메시지 DOM은 최대 80개이고, 반대쪽 페이지와 측정
캐시는 정리된다. 이와 별개로 최신 runtime 문맥 128개와 창 밖 마지막
assistant 최대 1개를 사용하며, worker 문맥에는 128개/512 KiB 제한이 있다.
따라서 메모리에 오직 15턴만 존재한다는 뜻은 아니다.

페이지 요청은 실행 1개·최신 예약 1개로 제한하고 분기 변경/폐기 시
epoch로 늦은 결과를 거부한다. IPC 자체에 물리적 취소는 없어 이미 시작한
요청의 임시 snapshot은 완료까지 남을 수 있다. 메시지 개수 제한도 전체
브라우저 heap의 절대 바이트 제한과는 다르다. 페이지 API가 없는 legacy
client의 전체 목록 fallback에는 이 부분 로딩 보장을 적용하지 않는다.

## 확인한 결과

- 공통 memory queue 통계: 10,000개 완료 이력 + 30개 queued 조건에서
  SQLite VM steps 8,126,554 → 272,208, 약 29.9배 감소.
- 실제 provider 20회 요청: TCP 연결 1개 재사용. 합성 credential A/B/없음의
  요청별 분리와, 첫 Tokio runtime 종료 후 새 runtime의 재연결도 통과.
- Portable: alias 128개 → resolve 1회, native 동시 최대 4개,
  연속 문서 변경 200개 → 실행 1개 + 최신 대기 1개를 회귀 테스트로 확인.
- 1 MiB 스트리밍 저장: 기존 전체 UPDATE 대비 실제 0043 SQL의 WAL bytes는
  64 KiB × 16회 조건에서 9,158,792 → 1,396,712,
  4 KiB × 256회 조건에서 138,691,592 → 6,464,312.
  회계를 위해 autocheckpoint만 끈 합성 실험이며 FULL은 유지했다.
  새 구조의 검증 캐시는 256회 조건의 VM steps를 1,285,389 → 68,037로
  줄였다. 다른 DB 쓰기 뒤에는 전체 검증을 다시 수행한다.
- 프런트 전체 156개 파일 / 842개 테스트, check와 production build 통과.
  Lua/regex sandbox 후속 검사도 통과. Build에 큰 chunk 경고가 있다.
- Python 검사 테스트 102개 통과. IPC 생성물, refactoring archive, i18n,
  strict context budget, source architecture, workflow security 검사 통과.
- Rust workspace의 unit/integration/doc-test 전체 실행과 보정된 회귀 테스트
  재검증을 완료했다. 전체 Clippy(`--workspace --all-targets -- -D warnings`),
  rustfmt와 Git diff 검사도 통과.

Rust 전체 실행에서는 1,965개 통과와 테스트 입력/기대값 문제 2개가 나왔다.
최신 스키마 기대값 42를 43으로 맞춘 Core 회귀 1개와, HEIF 파서가 오류를
삼키는 위치를 정확히 재현하도록 입력을 보정한 이미지 모듈 11개를 다시
실행해 모두 통과했다. 이 마지막 보정은 테스트에만 적용했다. 중복 실행을
제외하면 1,967개 사례가 검증됐으며, 기본적으로 ignored인 수동 성능 측정·
subprocess helper·사전 빌드 rollback runtime 의존 사례 14개는 별도다.

검증 환경은 macOS ARM64, Rust 1.96.0, Node 24.18.1이다. Rust 실행은
`CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, dev/test debug symbols 0,
`--test-threads=4`를 사용했다. 이는 로컬 빌드 자원 설정이며 저장소의 기본
debug 설정이나 제품의 최적화/내구성 정책을 바꾸지 않는다.

로컬 `preview.html`에서도 직접 캐릭터 선택→새 대화→초안 입력→대화
이동/복귀→모의 전송을 수행했다. 초안 유지, 전송 후 입력창 비움,
대화 응답을 확인했다. 이 화면은 모의 client이며 네이티브 성능 측정은 아니다.

## 채택하지 않은 제안과 보장 범위

DNS 집합 비교·CRC·파일 identity·FULL 내구성·credential 검사를 제거하지
않았다. 원본 이미지의 손실 압축이나 미전송 draft의 임의 퇴출도 하지 않았다.
압축/디코딩 작업을 무조건 늘리는 영구 thumbnail 파이프라인은 채택하지 않았다.

계보는 첫 요청과 변경 뒤 전체 검증 비용이 남는다. 캐시 안의 anchor·membership
탐색과, 최신 페이지에 assistant가 없을 때의 보충 조회도 전체 계보 크기에
따른 작업이 남아 있으므로 모든 페이지 연산을 상수 시간으로 만든 것은 아니다.
이미지 픽셀 제한은
애니메이션의 전체 프레임이나 화면 전체의 합산 디코딩 메모리를 보장하지
않는다. 기본 entry bundle의 큰 chunk 경고 역시 전체 앱 체감 개선 수치로
바꾸어 주장하지 않는다. 교차 플랫폼 CI와 실제 기기 profiling은 로컬
검사로 대체하지 않는다.

## 구현 계약과 상세 근거

- [작업 범위/불변 조건](final-resource-efficiency-task.md)
- [API·이미지 정책·의존성·0043 계약](final-efficiency-contracts.md)
- [Storage](final-efficiency-storage.md)
- [Native/Core/Provider](final-efficiency-native-providers.md)
- [Assets](final-efficiency-assets.md)
- [Frontend](final-efficiency-frontend.md)
