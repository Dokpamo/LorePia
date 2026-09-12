# 저장 공간·CPU·메모리 최적화 검토

2026-09-12 · `codex/ux-responsiveness-20260912`

앞선 [Toss UX 감사](toss-ux-responsiveness-audit.md)에 이어, 같은 기능과 표시 결과를
유지하면서 불필요한 조회·복사·해시·레이아웃 측정·장기 메모리 점유를 줄였다.
사용자 요청에 따라 Astra 에이전트 3개가 Storage/벡터 검색, Content/이미지,
프런트/portable renderer를 각각 조사·구현했다. 부모가 공통 로딩/SQLite 정책을
수정하고 통합 검증했으며, 에이전트 2개가 부모 변경도 독립 검토했다.

수정은 기존 격리 작업 트리에서 진행했다. 사용자 데이터베이스나 원본 작업 트리를
열어 정리하지 않았으며, 측정에는 임시 SQLite와 합성 콘텐츠만 사용했다.
사전 범위·불변조건·스냅샷은 [작업 기록](resource-optimization-task.md)에 있다.

## 확인된 비용과 반영 결과

| 경로 | 기존 비용 | 반영한 동작 |
| --- | --- | --- |
| SQLite WAL | 큰 transaction 후 커진 WAL 파일을 계속 재사용, 보관 상한 없음 | `journal_size_limit=8MiB`; SQLite가 안전하게 WAL을 재사용하는 시점에 초과 공간 회수 |
| 라이브러리/대화/분기 읽기 | 같은 고정 SELECT를 호출할 때마다 준비 | 기존 16개 LRU statement cache 재사용 |
| 승인 이미지 조회 | descriptor/range 요청마다 같은 SQL을 재준비 | SQL 준비 결과만 재사용; DB 승인 및 CAS 검증은 매번 기존 절차 수행 |
| 대화 표시용 sidecar | projection이 없는 배치에도 diagnostic JOIN 수행 | 없는 배치에서는 즉시 identity projection 반환; 있는 배치는 기존 검증 수행 |
| 지식 벡터 검색 | SQLite blob을 후보마다 새 `Vec<u8>`로 복사 | 현재 행의 bytes를 빌려 점수 계산, 다음 행 이동 전에 종료 |
| 메모리 벡터 검색 | DB 읽기 후 해시/점수/정렬까지 연결 잠금 유지 | 후보 snapshot을 읽은 직후 잠금 해제 |
| 메모리 벡터 점수 | 후보마다 `Vec<f32>` 생성, 값 검증/norm/dot을 별도 순회 | encoded bytes에서 직접 검사와 기존 순서의 f64 누적; 임시 float 벡터 제거 |
| 이미지 로딩 우선순위 | 4개 슬롯을 채울 때 대기열 위치를 반복 측정 | 한 배치에서 후보별 1회 측정, 최대 4개만 선별 |
| 동시에 끝난 이미지 요청 | 완료마다 대기열을 다시 측정 | 같은 microtask 묶음의 빈 슬롯을 함께 처리 |
| 썸네일 observer | 정렬 비교마다 공유 scroller 좌표 조회 | observer 전달 1회에 공통 bounds 1회 조회 |
| 화면 밖 썸네일 | 64MiB/64개 상한 안에서는 계속 DOM media 유지 | 재방문용 30초 보관 후 참조 해제; 전체 cache에 timer 하나, 비면 timer 제거 |
| PNG `tEXt` | 관심 없는 chunk도 keyword/text 전체 복제 | 원래 payload slice를 빌려 읽고, 다음 chunk까지 필요한 첫 legacy만 소유 |
| PNG base64 | 줄바꿈 없는 일반 입력도 compact buffer 복제 | 공백 없는 입력은 원래 slice에서 decode, 공백 있을 때만 compact |
| portable 에셋 선택 | 후보가 하나여도 긴 본문을 에셋 참조마다 해시 | 단일 후보는 해시 생략, 복수 후보는 빌드당 본문 prefix 해시 1회 |
| portable alias | 참조 없는 markup도 alias를 정규화; 비교 prefix 반복 생성 | 첫 실제 참조에서만 인덱스 구성, 참조당 비교 prefix 재사용 |
| room 영역 레이아웃 | 같은 조상의 computed style 반복 조회 | 한 report 동안만 style 재사용, 다음 report에서 다시 측정 |

## 재현 가능한 측정

| 측정 | 이전/비교 조건 | 변경 후 | 해석 |
| --- | --- | --- | --- |
| 16MiB SQLite transaction 후 WAL | 16,916,752 bytes | 다음 작은 transaction에서 8,388,608 bytes | 약 50.4% 회수. 활성 WAL 전체의 hard cap은 아님 |
| 고정 읽기 6,000회, cache 0/16 교차 실행 | 92,062 / 94,300 µs | 23,709 / 23,662 µs | 평균 약 74.6% 감소. 빈 목록/없는 에셋 조회의 SQL 준비 비용 비교 |
| 대기 이미지 200개에서 4개 선택 | 200+199+198+197=794회 priority 평가 | 200회 | 배치 drain 구간의 평가 수; enqueue 시 기본 확인은 양쪽에 존재 |
| observer가 200개 썸네일 전달 | 비교마다 root bounds 조회 | 전체 1회 | 이미지 순서와 준비 범위 유지 |
| 262,144자 본문, 128개 다중 후보 참조 | 33,555,072 UTF-16 code unit 순회 | 262,657회 | 약 99.2% 감소. 245개 입력 조합에서 기존 선택과 동일 |
| 단일 후보 에셋 | 참조마다 본문 해시 | 0회 | 선택 가능한 결과가 이미 하나인 경우 |

SQLite 측정은 프로젝트의 `rusqlite 0.40.2` / bundled `SQLite 3.53.2`,
macOS arm64, Rust 1.96.0 debug test에서 실행했다. SQL cache 비교는 같은 구현에서
cache 용량을 0과 기존 기본값 16으로 바꾼 실험이다. 실제 대규모 라이브러리 전체의
속도나 앱 시작 시간이 4배 빨라진다는 의미가 아니다. CI는 흔들리는 시간값으로
합격 여부를 판정하지 않는다.

WAL 측정과 cache 실험은 다음 테스트로 재실행할 수 있다.

```bash
cargo test -p lorepia-storage --lib database::pragmas::tests -- --nocapture
cargo test -p lorepia-storage --lib prepared_read_paths_reuse_sql_without_caching_rows -- --ignored --nocapture
```

복사 제거는 allocation 구조의 개선이다. PNG는 최대 16MiB chunk의 추가 text 복제와,
공백 없는 base64의 encoded 크기만큼의 추가 buffer를 피한다. 지식 검색은 후보당
최대 128KiB blob 복사를, 메모리 검색은 후보당 최대 128KiB float vector를 피한다.
이는 전체 프로세스 RSS가 그만큼 항상 줄어든다는 실측 주장은 아니다. 브라우저의
decoded image cache와 OS 메모리 회수는 엔진 정책을 따른다. 30초 정책은 앱이 유지하던
화면 밖 media의 DOM 참조를 제거하여 회수가 가능하게 만든다.

## 저장과 이미지 품질을 지킨 방식

SQLite의 WAL/SHM은 복구와 동시 읽기에 필요한 파일이다. 삭제된 행의 빈 DB 페이지도
다음 쓰기에 재사용된다. WAL은 SQLite가 reset 가능한 시점에만 줄이며 `WAL`,
`synchronous=FULL`, 기본 1,000페이지 auto-checkpoint를 유지했다. 읽기 transaction이
붙잡은 WAL이 상한을 넘더라도 잘라내지 않는 테스트를 추가했다.
[SQLite WAL 설명](https://www.sqlite.org/wal.html),
[journal_size_limit](https://www.sqlite.org/pragma.html#pragma_journal_size_limit).

매번 VACUUM을 수행하면 전체 DB 재작성과 잠금, 추가 디스크 공간이 필요하다.
그래서 매 시작/메시지 저장 시 vacuum, WAL 수동 삭제, fsync 축소를 넣지 않았다.
`PRAGMA optimize`의 통계 갱신은 공간 정리와 다른 작업이다. 이번에는 통계/스키마와
실행 계획 선택을 바꾸는 새 자동 유지보수 루프도 추가하지 않았다.
[VACUUM의 비용](https://www.sqlite.org/lang_vacuum.html),
[PRAGMA optimize](https://www.sqlite.org/pragma.html#pragma_optimize).

prepared statement는 SQL 실행 준비 결과만 보관한다. 호출 때마다 최신 행을 읽고,
반납할 때 statement를 reset하고 이전 바인딩을 지운다. 승인 descriptor 자체나
사용자 결과를 영구 cache하지 않는다.
[rusqlite 0.40.2 prepare_cached](https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html#method.prepare_cached).

PNG/CHARX 원본은 캐릭터 메타데이터, provenance, digest 및 정확한 export 대상이다.
이번 PNG 변경은 파싱 과정의 복사만 줄인다. 이미지 bytes와 화질, 애니메이션,
원본 hash는 그대로다. renderer는 승인 descriptor와 canonical digest URL만 받으며,
동일 열린 handle의 전후 identity/hash/MIME/범위 검증 및 기존 budget을 유지한다.

## 검토했으나 이번에 바꾸지 않은 항목

| 항목 | 판단 |
| --- | --- |
| 압축된 썸네일의 디스크 cache | 큰 원본을 작은 화면에 표시하는 비용을 더 줄일 수 있다. codec, 픽셀/메모리 예산, 원본→파생 digest/승인 관계, 원본 변경 시 무효화, cache 용량·LRU 회수 설계가 필요하다. 공개 계약/의존성을 바꾸는 후속 작업이며 이번 패치에 포함하지 않았다. |
| 원본 이미지 일괄 재압축 | metadata/원본 식별자/정확한 export를 바꾸므로 원본 교체 방식으로 적용하지 않았다. |
| 과거 embedding/DB generation 자동 삭제 | 불변 행 trigger와 rollback 복구 증거를 보존하는 정책을 먼저 변경해야 한다. 이름만으로 제거하지 않았다. |
| 검색용 새 index | 현재 SQL 결과의 비용을 더 줄일 가능성은 있으나 실제 실행 계획/데이터 분포 검증과 새 migration이 필요하다. |
| 모든 조회/파일을 큰 cache에 보관 | 메모리와 무효화 비용이 늘고 승인 경계를 흐릴 수 있다. 기존 bounded SQL/handle cache와 짧은 화면 cache만 사용한다. |
| 메모리 검색 top-k | 현재 Core 호출자가 이미 제한된 후보 전체를 결과로 요청하므로 전체정렬을 바꿀 이득이 작다. |
| portable state LRU 조기 종료 | 현재 최대 1,024개 metadata이며 뒤쪽 손상 행 검출 의미가 달라질 수 있다. |
| archive의 과도한 metadata 예약 의심 | 앞선 caller가 4MiB 한도를 이미 검사한다. 재검토 후 오탐으로 제외했다. |
| 이미지 range buffer 복제 의심 | native PNG 경로는 이미 같은 buffer의 copy_within/truncate를 사용한다. CRC 검증을 생략하지 않았다. |

## 검증 기록

- 변경 전: Storage lib 299개, assets/thumbnail 44개, PNG integration 8개, scoped memory embedding baseline 통과.
- 변경 후: Storage lib 304개(수동 측정 1개 ignored), Content 전체 114개,
  관련 Core integration 15개, 지식 embedding integration 3개 통과.
- 프런트 전체 140개 파일 / 770개 테스트와 Lua sandbox·regex worker 회귀 검사 통과.
  기본 동시 실행에서 worker 시작/응답 시간초과가 발생해, 다른 무거운 검사를 마친 후
  `--maxWorkers=2`로 재실행했다. 테스트 제한 시간이나 assertion은 완화하지 않았다.
- `npm run check`(format/i18n/ESLint/TypeScript/Svelte), production build 통과.
  Svelte 진단 0개. 초기 JS는 765.48kB(gzip 202.61kB)이며, 앞선 UX 작업의
  764.78kB보다 약 0.70kB 증가했다. 이번 개선은 실행 중 반복 비용을 줄이는 데 집중했고,
  기존 500kB chunk 및 Wasmoon 빌드 경고는 남아 있다.
- `cargo fmt --all --check`, Storage/Content `clippy --all-targets -- -D warnings`,
  Python 102개 테스트, IPC 생성물·source architecture·i18n·AI context strict budget·
  refactoring archive·GitHub workflow security 검사와 `git diff --check` 통과.
- Rust 전체 workspace test/clippy와 native/cross-platform CI는 이번 검증 범위에
  포함하지 않았다. 병합 전 저장소의 전체 gate와 실제 기기 성능 검증이 필요하다.
- 고정 preview client의 실제 WorkspaceApp에서 홈→프로필→전체 이미지→이미지 상세를 직접 클릭하고 표시/돌아가기를 확인했다. 콘솔 warn/error 없음. 이 브라우저 검사는 native IPC/실제 이미지 디코딩 속도 실측을 대신하지 않는다.
- 독립 검토에서 동기 재진입 중 추가된 foreground 요청의 우선순위 반례를 발견해 배치를 무효화하고 다시 고르도록 수정했다. native 완료 전 슬롯을 반환하지 않는 기존 규칙은 유지한다.
- 별도 감사 자료: [Storage](resource-storage-findings.md), [Content/이미지](resource-assets-findings.md), [프런트](resource-frontend-findings.md).

## 검토용 결과물

변경은 `codex/ux-responsiveness-20260912`의 격리 작업 트리에 미커밋 상태로 남겼다.
이번 추가 최적화만 담은 패치는 `/tmp/lorepia-resource-20260912/resource-efficiency.patch`다.
이 패치의 기준은 위 작업 트리의 이번 작업 직전 snapshot이며, 최초에 상속한 화면 수정과
앞선 UX 수정은 포함하지 않는다. 적용 검사는 그 snapshot 사본에서 수행했다.
개별 검사 로그와 파일별 전후 크기/해시는 같은 디렉터리에 보관했다.
