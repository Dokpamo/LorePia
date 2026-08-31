# Storage public API audit

기준일: 2026-08-29

## 결론

`lorepia-storage`의 공개 API에는 진짜 repository 결과뿐 아니라 persistence row,
transaction command와 hash helper가 함께 노출돼 있다. 현재 Core와 호환성 테스트가 이
타입들을 사용하므로 일괄 `pub(crate)` 전환은 breaking change다. 새 의미 기반 view/port를
먼저 만들고 호출자를 이동한 뒤 단계적으로 축소한다.

## 분류

| 분류 | 대표 타입/함수 | 판단 |
| --- | --- | --- |
| 의미 있는 repository 결과 | `DatabaseStats`, `DiscoveredProviderGraph`, `PersonaCatalogPage`, `KnowledgeEmbeddingQueryResult` | 공개 유지 후보 |
| use-case input/output | `MemoryJobEnqueue`, `GenerationAttemptInput`, `PackageCommitInput`, `RuntimeModelAuditStart/Finish` | Core 전용 port로 이동 후보 |
| persistence row | `StoredGenerationAttempt`, `StoredInteractionProposal`, `StoredPromptMessage`, `StoredRevision`, `StoredRuntimeModelAudit` | storage 내부 또는 전용 read view로 축소 |
| transaction command | `*Commit`, `*Write`, `Prepared*`, `PersistDiscoveryTransition` | 직접 공개 대신 의미 기반 repository method 뒤로 이동 |
| internal helper/hash | `*_sha256`, deterministic id helper, validation helper | 필요한 계층에 좁은 facade를 제공한 뒤 visibility 축소 |
| compatibility export | schema-11/cutover 및 recovery fixture가 직접 쓰는 타입 | 제거 버전과 대체 API를 정한 뒤 deprecate |

## 현재 불변조건

- `Storage`가 SQLite Connection과 데이터 루트 경로를 소유한다.
- renderer/shell DTO에는 DB row, absolute path와 raw file handle을 노출하지 않는다.
- import, generation, discovery, credential 작업의 durable phase와 recovery는 storage
  repository에 있다.
- schema migration과 frozen schema-11 검증은 커밋된 fixture와 별도 harness로 확인한다.

## 진행 현황

- Core 공개 API에 새 `lorepia_storage::Stored*` 재노출을 추가하지 못하도록 architecture
  ratchet을 적용했다. allowlist는 비어 있으며 모든 `Stored*` 재노출을 거부한다.
- ENF-002에서 Core/Storage 공개 진입점 기준선을 v2로 올렸다. 기준선은 crate root의
  explicit re-export leaf/origin/조건부 attribute, 공개 module reachability, production
  `include!`의 root surface와 경로 비종속 public-item multiset을 기록한다. 함수·method
  signature, public field, enum variant, trait item/impl은 whitespace와 일반 함수 본문을
  제외한 token-normalized SHA-256 anchor로 고정한다. private 또는 `pub(crate)` type의
  unrestricted member는 포함하지 않지만 private module 안의 `pub` item은 공개 경계로
  이동하는 경우를 잡기 위해 보수적으로 inventory에 포함한다. 기존 Core wildcard 두
  개의 대상 module도 같은 방식으로 펼친다. local/workspace macro는 정의, invocation,
  transitive helper와 생성 owner를 함께 고정하며 증명하지 못한 production item macro
  binding은 실패한다. 현재 기준은 Core 5,656개, Storage 1,517개다. 새 anchor와 동일
  개수의 교체도 실패하며, 실제 제거와 기준선 축소만 허용한다.
- Cargo architecture 기준은 workspace package 10개, workspace dependency declaration
  22개, direct external dependency declaration 110개와 package feature activation을
  `kind`/`target`/`optional`/default features/feature list/rename까지 함께 고정한다. 패키지
  이름만이 아니라 canonical workspace path로 내부 edge를 판별하므로 registry/path
  dependency가 workspace 이름을 흉내 내는 경우도 허용되지 않는다.
- 사용처가 없던 `StoredPromptMessage`, `StoredGenerationAttempt` 재노출을 제거했다.
- discovery 후보 목록은 Core 소유 `ProviderDiscoveryCandidateView`를 반환하므로
  `StoredDiscoveryCandidate`를 Core에서 재노출하지 않는다.
- Core의 discovery read path는 `DiscoveryCandidateSnapshot`과
  `Storage::read_discovery_candidates`를 사용한다. 기존 transaction input과
  호환 호출자를 위해 `StoredDiscoveryCandidate`와 `list_discovery_candidates`는
  아직 공개 상태로 유지한다.
- derived-event drain은 content-free `InteractionDerivedDrainReceipt`만 반환하므로
  `StoredInteractionEvent`를 Core에서 재노출하지 않는다.
- provider credential journal은 Core 소유 `ProviderCredentialOperationView`로 투영하므로
  `StoredProviderCredentialOperation`을 Core와 Shell 공개 경계에서 제거했다.
- interaction proposal/effect는 Core 소유 view/claim/history 타입으로 투영하므로 세 가지
  persistence row를 Core와 Shell 공개 경계에서 제거했다.
- 범용 `StoredRevision`의 Core 호환 재노출도 제거해 공개 `Stored*` 예외가 남지 않았다.

## 단계별 축소 계획

1. Core가 실제로 읽는 필드만 가진 domain/use-case view를 정의한다.
2. `Stored*` 타입을 반환하는 호출을 새 view/repository method로 교체한다.
3. transaction 입력은 공개 struct 조립 대신 storage의 의미 기반 method 인자로 감싼다.
4. 외부 호환 소비자가 없는 것을 `cargo public-api` 또는 동등한 diff로 확인한다.
5. persistence row와 helper를 `pub(crate)`로 내리고 제거 예정 export를 문서화한다.

이 문서는 분류 감사이며 API 축소 완료 선언이 아니다.

## ENF-002 검사 범위와 잔여 한계

이 검사는 pinned stable toolchain에 별도 `cargo-public-api` 의존성을 추가하지 않고
동작하는 source-level ratchet이다. Core/Storage facade의 각 leaf는 workspace source의
유일한 production public 정의로 해석하며, 정의가 없거나 둘 이상이면 임의 선택하지
않고 실패한다. 공개 선언 위치와 method 본문은 일반 anchor에서 제외하므로 같은
reachability의 child-module extraction은 공개 계약이 같으면 기준선을 바꾸지 않는다.
반면 root/public module, root `include!`, public-emitting item macro의 reachability 이동은
의도적으로 별도 context anchor를 바꾼다.

- named/tuple struct는 public field shape와 public constructor 가능 여부만 기록하므로
  이미 private field가 있는 타입의 private field 내부 변경은 공개 표면으로 세지 않는다.
- trait default method와 일반 함수/method 본문은 제외한다. 반면 trait header/item과
  exported type의 explicit/derived trait surface는 기록한다.
- Core의 기존 `lorepia_domain::discovery::*`와
  `lorepia_domain::orchestration::*`는 legacy wildcard anchor로 고정하고 대상 module을
  동일 규칙으로 펼친다. 새 wildcard나 펼친 surface는 실패한다.
- local macro dependency는 같은 crate의 helper chain과 cross-file signature까지 추적한다.
  workspace macro는 direct path, item import alias와 crate-prefix alias를 해석한다. 그 밖의
  production item binding은 owner를 추측하지 않고 실패하므로 새 binding에는 검사 정책이
  먼저 필요하다. public-generating macro 구현만 바뀌어도 재검토가 필요할 수 있다.
- production item-scope `include!`는 source root 안의 plain relative literal만 재귀적으로
  읽는다. 동적·누락·root 탈출 target은 실패하고, 포함 파일의 `#[cfg(test)]` scope는
  production surface에서 제외한다.
- compiler가 모든 target/feature에서 계산하는 완전한 `cfg` reachability와 procedural
  macro expansion을 재구성하지는 않는다. cross-platform build, test, lint와 함께
  사용하며, legacy wildcard를 explicit leaf로 바꾸는 것이 최종 해소책이다.

따라서 기준선은 완전한 ABI 증명이 아니라 review ratchet이다. 항목 추가나 교체는
허용하지 않고, 실제 공개 surface 제거와 함께 기준선을 줄이는 것만 허용한다.
