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

## 단계별 축소 계획

1. Core가 실제로 읽는 필드만 가진 domain/use-case view를 정의한다.
2. `Stored*` 타입을 반환하는 호출을 새 view/repository method로 교체한다.
3. transaction 입력은 공개 struct 조립 대신 storage의 의미 기반 method 인자로 감싼다.
4. 외부 호환 소비자가 없는 것을 `cargo public-api` 또는 동등한 diff로 확인한다.
5. persistence row와 helper를 `pub(crate)`로 내리고 제거 예정 export를 문서화한다.

이 문서는 분류 감사이며 API 축소 완료 선언이 아니다.
