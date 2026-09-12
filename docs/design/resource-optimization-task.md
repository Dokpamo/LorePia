# RESOURCE-OPTIMIZATION-20260912

- 요청: UX 응답성 작업에 이어 SQLite, 이미지, 프런트/백엔드의 저장 공간과 반복 연산을 줄인다.
- 기준 commit / main merge-base: `aac3b16e0697c710b1209f498caba71821746140`.
- 브랜치: `codex/ux-responsiveness-20260912`.
- 작업 트리: `/Users/codexer/.codex/worktrees/lorepia-ux-20260912`. 이전 UX 작업과 최초에 상속한 화면 작업이 미커밋 상태다. 원래 작업 트리는 수정하지 않는다.
- 이번 작업 직전 파일/해시와 diff: `/tmp/lorepia-resource-20260912/baseline.json`, `baseline-files/`, `baseline.patch`.

## 범위와 불변조건

성능 동작 변경이며 단순 코드 이동 작업이 아니다. 기존 공개 함수, DTO, SQL schema,
IPC, 의존성은 유지한다. 코드 이동/rename은 예정하지 않는다.

추가 요청으로 Astra 3개가 병렬 조사·구현했다. 메모리 벡터 점수 계산과 portable
에셋 선택은 최적화의 동등성 검증을 위해 private helper로 분리했다. 공개 이름과
진입점은 유지하고 이동/의미 변경 범위 및 테스트를 각 감사 기록에 남겼다.

| 대상 | 공개 진입점과 소유권 | 변경 | 유지할 조건 |
| --- | --- | --- | --- |
| Storage `database/pragmas.rs` | `Storage::open` 내부 연결 설정 | WAL 재사용 공간 상한 | WAL/FULL, 자동 checkpoint, 복구/transaction/fsync 순서 |
| Storage `database/{asset_delivery,character_catalog,conversations,branches,messages}.rs` | 기존 읽기 메서드 | 반복 SQL 준비 결과 재사용 | 매번 최신 DB 조회, 기존 정렬/오류/권한 검증, bounded statement cache |
| Storage `message_display_projection/loading.rs` | `get_message_display_projections` | 빈 결과 후속 조회 생략, SQL 재사용 | 배치당 snapshot, 해시/진단 검사, 입력 순서 |
| Storage `knowledge_embedding.rs` | 기존 cosine 조회 | 행의 벡터 bytes를 복사 없이 읽기 | 한 행의 수명, hash/길이/finite 검사, 연산 순서/점수/작업 예산 |
| Storage `memory_queue.rs`, private `memory_queue/embedding_scoring.rs` | 기존 메모리 embedding 조회 | snapshot 이후 DB 잠금 해제, encoded bytes 직접 점수 계산 | 동일 f64 누적 순서/오류/정렬/후보 예산, decoder의 기존 caller 유지 |
| Content `png.rs` | 기존 PNG metadata inspection | text와 공백 없는 base64 입력 빌리기 | 원본 bytes/hash/export, 첫 legacy 및 v3 우선순위, bounded inflate |
| UI `PortableMessage.svelte`, private `portable-asset-selection.ts` | 기존 portable message 렌더링 | lazy alias index, 본문 prefix 해시 공유 | 후보 순서, UTF-16/FNV 연산, fallback, alias 한도 |
| UI `portable-renderer-bridge.js` | 기존 room report | report 안에서만 computed style 재사용 | 다음 report의 최신 레이아웃, message overflow 경로 |
| UI `features/assets/asset-delivery-loader.ts` | `loadAssetDelivery` | 한 번에 열리는 슬롯의 우선순위 계산 합치기 | 최대 4개 실제 IPC, 취소/timeout/retry, 최신 스크롤 우선순위 |
| UI `ui/navigation/thumbnail-{visibility,retention}.ts` | 기존 관찰/유지 함수 | observer 배치 좌표 읽기 축소, 화면 밖 decoded media 회수 | 보이는 이미지 보호, LRU 상한, observer/timer 정리 |

예상 크기 변화: production 약 +1–2 KiB, 검증/측정 코드 및 문서는 별도.
주요 위험: cached statement가 DB 결과 캐시로 오해되거나 transaction을 연장하는 것,
WAL 활성 데이터를 임의로 자르는 것, queue 재진입/취소 경쟁, offscreen 재방문 시 재디코딩 비용.
각 기존 소유자 안에서 수정하고 새로운 장기 worker, 전역 DB 연결, 승인 캐시를 추가하지 않는다.

## 확인 자료와 검증

- root / Storage / Frontend AGENTS, ADR 0001–0003, 0005–0006,
  `docs/architecture/storage-public-api-audit.md`, Storage/Core facade 및 관련 호출자를 확인했다.
- `rg`로 심볼, SQL, 테스트, 기존 이미지 admission/검증/retention을 조사했다.
- 변경 전 Storage lib / assets 및 thumbnail 테스트를 실행한다.
- 실제 bundled SQLite의 WAL 재사용/reader/rollback/reopen, 반복 조회 변경 반영,
  display projection 검증, cosine 정렬/변조/예산 테스트를 확인한다.
- 프런트는 우선순위 변경/취소/timeout/레이아웃 읽기 횟수/retention 수명과 실제 화면을 확인한다.
- 변경 후 Storage/Core 및 필요한 Rust gate, 전체 frontend gate, IPC/architecture/i18n/context/archive 검사를 실행한다.
- 반복 실행 측정은 synthetic 임시 DB/이미지로 수행하며 사용자 DB를 열거나 정리하지 않는다.

## 조사 원칙

SQLite의 WAL/SHM, rollback용 DB generation, journal, CAS 원본은 이름만으로 삭제 가능한
찌꺼기가 아니다. PNG/CHARX 원본은 메타데이터와 원본 hash를 포함한다. 원본 재압축은
의미/품질/호환성이 달라질 수 있어 이번 수정은 이미 승인된 bytes 전달을 보존한다.
별도 thumbnail 저장은 새 파생물의 무효화·검증·공간 회수·codec 경계 설계가 필요한 후속 항목이다.

## 완료 기록

- Storage/Content/프런트 담당 Astra 3개의 구현을 통합했고, 2개가 공통 변경을
  독립 검토했다. queue 동기 재진입 시 새 foreground 요청이 늦어지는 반례를 고쳐
  회귀 테스트를 추가했으며, SQLite blob 오류 형식도 기존과 동일하게 유지했다.
- `memory_queue.rs`의 실제 감소분 2,250 bytes / 61 lines만큼 기존 source size cap을
  낮췄다. 새 giant-file 예외나 cap 증가는 없다.
- Storage 304개, Content 114개, 관련 Core 15개, 지식 embedding integration 3개,
  프런트 770개와 Python 102개 테스트 통과. formatter/lint/type/build 및 저장소
  계약 검사는 [최종 감사](resource-efficiency-audit.md)의 검증 기록을 따른다.
- 코드와 문서는 미커밋 상태로 남겼고, 이번 snapshot 기준 추가 변경만 별도 패치로
  만들었다. 원본 작업 트리, 사용자 DB/이미지, 공개 API/DTO/IPC/schema/의존성은
  수정하지 않았다. 원본 이미지 재압축과 파생 thumbnail 디스크 cache는 미포함이다.
