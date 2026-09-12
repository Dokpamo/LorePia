# 대용량 모듈 실행 계획 저장 변경안

상태: 사용자 승인, 구현 및 로컬 검증 완료. Task `IMPORT-COMPAT-20260911`.
기준 커밋 `15e559b`; 작업 브랜치 `codex/retire-legacy-ui-20260910`.
현재 작업 트리에는 가져오기·설정 연결·자산 검증·모듈 검토 UI 수정이 있다.

## 확인한 문제

첨부 추가 에셋 모듈은 원본을 수정하지 않고 가져올 수 있다. 중복 제거된 에셋
3,172개와 문서 항목을 포함한 Core 검토는 3,173개 항목, JSON 4,795,489바이트다.
전체 검토를 검증한 뒤 64개만 표시하는 새 Shell 응답은 52,110바이트다.
그러나 실제 적용은 `module activation review exceeds its JSON storage limit`로
실패한다. 검토 JSON을 보관하는 현재 DB 문서 한도는 2MiB/1,000,000자다.

이는 파일 손상이나 재시도로 해결할 문제가 아니다. 기존 한도를 높이거나 검토
항목을 누락하는 방식은 제안하지 않는다. 기본 모듈은 같은 경로에서 적용에 성공했다.

## 제안하는 변경

Storage가 큰 검토·승인 계획·실행 계획을 작은 불변 조각으로 저장하고, 기존 행에는
버전이 있는 조각 목록을 기록한다. 조각과 부모 계획, 바인딩 CAS, 승인 기록은
같은 SQLite 트랜잭션에서 확정한다. 파일시스템/CAS에 새 쓰기 경로는 만들지 않는다.

- 다음 번호의 migration 추가(현재 마지막은 `0041`, 새 migration은 `0042`). 기존 migration은 수정하지 않는다.
- 계획 문서용 조각 테이블을 추가한다. 부모 계획 종류/ID, 조각 순서, SHA-256,
  바이트 길이와 내용의 관계를 명시하고 중복 순서와 누락을 거부한다.
- 기존 단일 JSON 행은 계속 읽는다. 새 형식은 버전·전체 digest·조각 수·전체 길이를
  가진 manifest로 구별한다. 이전 프로그램으로의 다운그레이드는 지원하지 않는다.
- 조각당 한도와 합계 바이트/항목/깊이 한도를 별도로 정하고, 읽기 전에 검사한다.
  무제한 결합, 순환 참조, 조각 폭증이나 부분 성공은 허용하지 않는다.
- 합친 원래 문서의 canonical hash, import authority, review/plan/approval,
  binding revision, runtime source 검증을 모두 다시 수행한다. 해시 입력과
  Orchestration 알고리즘, 공개 Core DTO는 유지한다.
- 과거 이벤트·브랜치·롤백이 참조하는 실행 계획의 조각은 보존한다.

## 변경 대상과 검증

대상은 `crates/storage/migrations`, schema registry 및 cutover tests,
`crates/storage/src/orchestration/module_activation.rs`, `module_runtime.rs`와
새 전용 document codec/repository다. Core의 기존 activation/recovery 진입점은 유지한다.

필수 검증은 신규 설치, 기존 DB 업그레이드, 재실행, 조각 순서·누락·변조·과다 길이,
트랜잭션 실패 시 rollback, 응답 유실 후 같은 approval 재시도, 브랜치 상속과 과거
이벤트의 권한 검증, 모듈 해제/재적용이다. 실제 첨부 파일은 격리 복제본에서 먼저
적용하고 현재 UI에서 두 모듈 동시 적용과 이미지 참조까지 확인한다.

사용자가 스키마 변경 진행을 승인했다. 기존 migration 및 canonical hash는 그대로 유지한다.

## 구현 기록 — 저장 경로와 내부 helper 분리

같은 Task `IMPORT-COMPAT-20260911`, 기준 `15e559b`, 위 작업 트리에서 진행한다.
대화 시작 시 `generation_attempt`와 `interaction_repository`도 전체 모듈 검토와
실행 계획을 보관하므로 같은 codec을 사용한다. 다른 증거 문서의 기존 제한은 유지한다.
`generation_attempt.rs`의 private `encode_generation_attempt_authorities`와
`decode_module_runtime_authority`만 내부 `module_authority.rs`로 이동한다.
기존 Storage public 진입점, SQL 소유자, digest 계산 함수, 트랜잭션 경계는 남긴다.
예상 부모 감소는 약 120줄이며 크기 상한을 올리지 않는다. 주된 위험은 저장용
manifest를 원래 JSON의 hash 입력으로 혼동하는 것, 과거 계획 읽기 또는 재시도에
조각을 누락하는 것이다. 기존 generation-attempt/interaction 회귀 테스트와
대용량 활성화·재시작·첫 전송 테스트로 검증한다. ADR 0002/0003/0006을 따른다.

## 확정한 저장·IPC 계약

Migration `0042_module_plan_documents.sql`은 `module_plan_documents`와
`module_plan_document_parts`를 추가한다. 키는 문서 종류(`review`, `approval`,
`runtime`)와 원래 JSON의 SHA-256이다. 조각은 최대 256KiB, 문서는 최대 128조각/
32MiB/1,000,000개 JSON 노드다. 깊이 32와 금지 자격 증명 키 검사는 유지한다.
일반 문서의 기존 2MiB/1,000,000자 제한은 변경하지 않는다.

작은 문서는 기존 JSON을 그대로 저장한다. 큰 문서의 기존 JSON 열에는 다음
native 전용 manifest를 넣는다. renderer DTO로 내보내지 않는다.

```json
{
  "lorepia_module_document": 1,
  "kind": "review",
  "sha256": "<원래 JSON의 64자리 소문자 SHA-256>",
  "byte_length": 4780697,
  "part_count": 19
}
```

테이블과 조각은 update/delete trigger로 불변이다. 기존 부모 행, 활성화 승인,
바인딩 변경과 같은 트랜잭션에서 쓰고 실패하면 모두 rollback한다. 읽을 때는
metadata, 순번, 각 조각 길이/해시, 전체 길이/해시, UTF-8과 합산 한도를 검증한다.
누락·변조된 기존 조각을 쓰기 재시도로 덮어 고치지 않는다. 과거 generation과
브랜치에서 참조할 수 있도록 조각을 보존한다.

같은 codec을 activation review/approval, runtime plan, generation attempt,
interaction generation review/materialization의 모듈 권한 열에 적용했다. 해시 비교는
항상 복원된 원래 JSON에 대해 수행한다. 기존 generation snapshot의 큰 inline 문서도
합산 한도 안에서 읽는다. 다른 generation 증거 문서의 제한과 credential 처리는 유지한다.

UI용 명령은 아래 세 개다. `config/ipc-commands.json`, Shell DTO, Tauri handler,
생성 registry/permission, development/release capability, renderer client를 함께 갱신했다.

| 명령 | 계약 |
| --- | --- |
| `review_content_module_activation_page` | 전체 Core 검토를 재검증한 뒤 64개 항목 및 전체 충돌 목록을 반환한다. 첫 페이지 다음에는 정확한 review hash가 필수다. |
| `resolve_content_module_activation_summary` | 모든 충돌에 대한 명시적 선택을 전체 계획으로 해석하고 hash·revision·총계만 반환한다. |
| `activate_content_module_summary` | 전체 승인과 예상 receipt를 쓰기 전에 검증하고, 전체 계획을 적용한 뒤 정확한 compact receipt를 반환한다. |

페이지 하나를 승인 권한으로 사용하지 않는다. 기존 Shell의 2MiB 응답 한도와
512개 충돌 선택 한도는 유지한다. 이 파일의 수천 개 이미지처럼 충돌이 없는 많은
자료를 전체 검증·적용하면서 화면만 나누는 계약이다. 응답 유실 재시도에는 같은
approval ID를 사용하고, 검토 내용이 바뀌면 새 검토가 필요하다.

공개 Core 함수나 Storage `Stored*` re-export는 추가하지 않았다. 공개 API inventory는
새 SQL migration `include_str!` 기록 4개와 아래 Storage 일괄 조회 메서드의 직접·상속
기록 2개만 추가했다.
내부 helper를 옮긴 `generation_attempt.rs` 크기 baseline만 실측 감소분으로 낮췄다.

## 확인한 동작

실제 네이티브 앱은 schema 41에서 42로 업그레이드됐다. 기본 모듈이 켜진 테스트
대화에 추가 에셋 모듈을 적용해 3,175개 항목 전체 승인을 확인했고, UI에서 해제한
뒤 새 검토와 승인으로 다시 적용했다. 기존 문서와 카드 원본은 유지됐다.
이 조합에서 큰 runtime 문서는 16,559,021바이트/64조각으로 저장됐다.

합성 에셋 1,024개를 사용하는 Core 테스트에서도 큰 문서가 조각으로 저장됨을
검사하고, 재시작·새 브랜치·다른 대화의 첫 generation dispatch를 통과했다.
로컬 합성 provider를 사용했으며 실제 외부 AI 모델 응답을 검증한 것은 아니다.

재시작 실측에서 `get_character_render_profile`의 대화별 표시 정보 조회도 전체 모듈
검증에 진입하는 것을 native sample로 확인했다. 이 기존 Tauri 명령을 같은 2개
worker admission으로 옮겼다. 명령명과 요청/응답 DTO, 승인 검사와 Storage 트랜잭션은
유지하며, handler의 Future/Send 계약과 worker thread/상한 회귀 테스트로 검증한다.
기존 거대 `commands.rs`의 크기 상한을 유지하기 위해 worker 진입은 기존 표시 정보
helper가 있는 `character_commands.rs`에 두고 명령은 한 줄 위임을 유지한다.
같은 Task/기준 커밋에서 이동하는 것은 이 새 worker 호출뿐이며, 기존 동기 helper의
분기·검증과 command 공개 진입점은 남긴다. 부모 파일은 기존 크기 이하로 유지한다.

## 런타임 일괄 조회 계약

같은 Task의 재시작 검증에서 추가로 확인한 병목은 구성 요소마다 부모 모듈의 전체
목록을 다시 읽는 경로다. 3,172개 에셋의 표시와 프롬프트 구성이 제곱에 비례하는
조회가 된다. 파일을 실제로 사용하는 데 필요한 수정으로 Storage의 읽기 API
`get_module_revision_components(&[ResolvedModuleComponent])`를 추가한다. Core의 공개
API·DTO는 유지하고 표시 및 프롬프트 materialization의 내부 호출만 교체한다.

한 번의 입력은 최대 8,192개이고 입력 순서대로 같은 수의 기존 component snapshot을
반환한다. 더 큰 합산 실행 계획은 Core가 여러 배치로 읽으므로 새 전체 항목 제한을
만들지 않는다. 모든 배치가 검증된 후에만 표시·프롬프트 결과를 반환한다.
하나의 읽기 트랜잭션에서 부모의 전체 문서·구성 요소 목록을 리비전당 한 번 검증한
뒤 각 요청의 source hash, component hash, payload/연결된 불변 리비전을 기존과
동일하게 검증한다. 캐시는 그 트랜잭션 밖으로 나가지 않는다. 단건 조회도 같은
검증 helper를 사용하며 기존 공개 진입점은 유지한다. 부분 반환은 없다.

대상은 Storage `content_module_revisions.rs`, Core의 표시/프롬프트 materialization,
관련 회귀 테스트다. 기준 `15e559b`와 현 작업 트리에서 진행하며, 새 의존성·권한·
스키마는 추가하지 않는다. 기존 child 검증 helper를 남기고 중복 parent 조회만
공유한다. 예상 증가는 약 70줄이다. 주된 위험은 다른 revision의 부모 재사용,
입력 순서 변경, child hash 검증 누락이므로 단건 동등성·stale hash·변조 거부와
대용량 첫 generation 테스트로 확인한다. 공개 Storage API inventory에는 이
읽기 메서드의 정확한 추가만 반영한다.

재실행 UI sample에서 `list_reopen_interaction_effects`와 같은 효과 이력 조회가
Core lifecycle 복구를 배출하면서 모듈 검증을 호출하는 경로도 확인했다.
`orchestration_commands.rs`의 `list_reopen_interaction_effects`와
`list_interaction_effect_history`도 같은 bounded worker로 옮긴다. 기존 요청·응답,
복구 배출, 이벤트 순서와 트랜잭션은 유지하고 메인 스레드에서만 분리한다.
해당 두 handler의 Future/Send 계약과 기존 native/Core interaction 테스트로 확인한다.
같은 Task/기준 커밋에서 새 worker 호출 두 개만
`orchestration_commands/projections.rs`로 분리한다. 상한에 도달한 부모 파일에는
기존 명령 진입점과 한 줄 위임을 남겨 약 6줄을 줄이고, source cap은 올리지 않는다.
두 호출의 복구·이력 조회 순서와 오류 반환을 그대로 보존한다.

동시 표시 정보 조회와 복구 조회가 2개 worker를 경쟁해 정상적인 화면 진입에서
`busy`로 탈락하는 경우도 재현했다. 조회 전용 `run_module_read`는 최대 8개 요청만
받고 2개 worker가 비면 처리한다. 모듈 쓰기/승인은 기존 즉시 admission을 유지한다.
실행 중 worker 수는 늘리지 않고, 취소 시 대기 요청을 해제하며 이미 시작한 작업은
종료할 때까지 두 permit을 보유한다. 대기·과다 요청 거부·worker 반환을 테스트한다.
동시에 로드되는 플러그인 후보·활성 목록 조회도 같은 조회 대기를 사용한다.

추가 native sample에서 설정 루트 진입이 모든 원본의 내보내기 검증을 미리 실행하고
CAS mutation lock을 화면 스레드에서 기다리는 경로를 확인했다. `WorkspaceFeatures`
최상단의 중복 `loadPendingImports` 호출을 제거한다. 가져오기 화면 `PackageData`와
저장 공간 화면 `StorageData`는 기존처럼 자신의 진입 시 자료를 조회한다.
명시적 원본 조회 `list_completed_content_package_exports`도 같은 bounded read worker로
옮긴다. 내보내기/권한 검증을 생략하지 않고 필요한 화면에서만 실행한다.

최종 macOS 빌드에서 기존 대화 복원, 한국어 시작 메시지, 프롬프트 연결 화면을
다시 확인했다. 설정 루트의 중복 원본 검증을 제거한 빌드에서는 앞서 관찰한
`error.invalid_input`이 재현되지 않았고 설정 이동도 바로 반응했다. 대용량 계획과
원본의 최초 권한 검증은 unoptimized 개발 빌드에서 여전히 수십 초에서 수분 걸릴
수 있다. release 성능과 실제 외부 모델 응답은 이번 로컬 검증 결과에 포함하지 않는다.
