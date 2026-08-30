# LorePia AI-First Refactoring Master Plan

> **문서 성격:** 구현 순서와 금지사항까지 포함한 리팩터링 실행 계약서
>
> **대상 독자:** Codex, Claude Code, GPT 계열 코딩 에이전트와 사람 리뷰어
>
> **기준 저장소:** `Dokpamo/LorePia`
>
> **기준 브랜치/커밋:** `main@7114187cf4866f38d91bede82ef9bea8f96e39a2`
>
> **작성 기준일:** 2026-08-30 KST
>
> **상태:** Proposed — 각 작업은 독립 PR로 실행하고, 단계별 승인 게이트를 통과한 뒤 다음 단계로 이동한다.

---

## 0. 이 문서를 사용하는 법

이 문서는 단순한 아이디어 목록이 아니다. AI 코딩 에이전트는 아래 규칙을 **반드시** 따른다.

1. **한 번에 하나의 Task ID만 수행한다.** 여러 Task ID를 한 브랜치나 PR에 합치지 않는다.
2. 작업 시작 전에 `main`의 최신 커밋과 CI 상태를 확인한다. 기준 커밋 이후 코드가 바뀌었다면 작업 대상과 크기 수치를 다시 측정한다.
3. Task의 `read_first`, `allowed_paths`, `forbidden_changes`, `validation`, `done_when`을 먼저 읽는다.
4. 리팩터링 Task는 명시적으로 허용되지 않은 한 **동작, 공개 API, 저장 스키마, 직렬화 형식, 오류 코드, IPC command 이름을 변경하지 않는다.**
5. 먼저 기존 동작을 고정하는 테스트를 확인하거나 추가하고, 그다음 코드를 이동한다.
6. 코드를 이동하는 커밋과 의미를 정리하는 커밋을 분리한다. 첫 이동 단계에서는 이름 변경과 알고리즘 개선을 하지 않는다.
7. 부모 거대 파일의 source-size baseline은 **감소만 허용**한다. 새 모듈을 만들었다는 이유로 기존 baseline을 상향하지 않는다.
8. targeted test가 통과해도 PR 병합 전에는 전체 필수 검사를 실행한다.
9. 예상하지 못한 공개 API 변경, 마이그레이션 필요, 보안 경계 변화, 비결정성 변화가 발견되면 현재 Task를 중단하고 별도 Task 제안으로 기록한다.
10. Task 완료 후 다음 Task를 자동으로 시작하지 않는다. 결과 보고와 리뷰를 먼저 받는다.

### 규범 용어

- **MUST / 반드시:** 위반하면 Task 실패다.
- **MUST NOT / 금지:** 예외는 별도 ADR 또는 승인된 Task 수정이 있어야 한다.
- **SHOULD / 권장:** 특별한 기술적 이유가 없다면 따른다.
- **MAY / 선택:** 품질을 해치지 않는 범위에서 적용할 수 있다.

---

## 1. 목표

### 1.1 최우선 목표

LorePia의 기존 안전성, 결정론, 복구 가능성, 플랫폼 호환성을 유지하면서 다음 상태를 만든다.

- AI가 일반적인 기능 수정 시 저장소 전체나 수십만 줄짜리 파일을 읽지 않아도 된다.
- 기능의 진입점, 상태 소유자, 영속화 경계, 테스트 위치를 파일 구조만 보고 찾을 수 있다.
- 한 기능 변경이 보통 **운영 코드 3~8개 파일, 테스트 1~3개 파일** 안에서 끝난다.
- AI에게 제공되는 기본 context bundle은 총 **250KB 이하**를 목표로 한다.
- 공개 API와 IPC는 작은 facade 뒤에 유지되고, 내부 구현은 bounded context별 child module로 분리된다.
- 새 코드가 다시 거대 파일로 합쳐지지 않도록 CI ratchet이 막는다.

### 1.2 성공 기준

최종적으로 다음 기준을 충족한다.

| 대상 | 최종 목표 |
|---|---:|
| Rust/TypeScript 운영 파일 | 권장 400~700줄, hard target 900줄 또는 40KB 이하 |
| Svelte component | 권장 250~450줄, hard target 600줄 또는 30KB 이하 |
| 테스트 파일 | 권장 500~900줄, hard target 1,200줄 또는 60KB 이하 |
| facade / barrel / `mod.rs` | 500줄 이하, 실제 로직 최소화 |
| 단일 AI Task의 `read_first` | 기본 15개 파일 이하 |
| 단일 AI Task의 총 context bundle | 기본 250KB 이하 |
| 새 source-size baseline 예외 | 원칙적으로 0개 |
| 기존 baseline | 전용 리팩터링 PR에서 감소만 허용 |

이 수치는 무조건 잘게 쪼개기 위한 규칙이 아니다. 100줄 미만의 의미 없는 `part1.rs`, `helpers2.ts`를 양산하는 것도 실패다. **파일은 줄 수가 아니라 변경 이유와 불변조건 소유권으로 나눈다.**

### 1.3 비목표

이번 리팩터링에서는 다음을 하지 않는다.

- Tauri, Svelte, SQLite, Rust core 구조를 다른 프레임워크로 교체하지 않는다.
- 새로운 상태 관리 라이브러리나 ORM을 도입하지 않는다.
- provider protocol, prompt semantics, ranking, memory selection을 재설계하지 않는다.
- UI 디자인을 전면 변경하지 않는다.
- crate를 무분별하게 추가하지 않는다.
- 모든 중복을 generic, macro, trait로 제거하지 않는다.
- 마이그레이션 0001~0011을 수정하지 않는다.
- 리팩터링 PR에 의존성 업그레이드를 섞지 않는다.
- 보안 검증을 “코드가 길다”는 이유로 삭제하거나 완화하지 않는다.

---

## 2. 기준 상태와 우선순위

현재 상위 계층 설계는 좋다. `domain → orchestration → core → shell-api → native shell/UI` 방향이 문서화되어 있고, `storage`와 `providers`는 Core가 사용하는 adapter다. 문제는 crate 구성이 아니라 **crate 내부의 거대 구현 파일**이다.

### 2.1 P0 거대 파일

기준 커밋의 `config/source-size-baseline.json`에 등록된 주요 파일은 다음과 같다.

| 우선순위 | 파일 | 현재 크기 | 현재 줄 수 | 최종 facade 목표 |
|---|---|---:|---:|---:|
| P0 | `crates/core/src/app.rs` | 784,071B | 19,693 | 40KB / 1,000줄 이하 |
| P0 | `crates/storage/src/discovery_repository.rs` | 769,337B | 18,495 | 30KB / 700줄 이하 |
| P0 | `crates/storage/src/database.rs` | 694,566B | 17,973 | 40KB / 900줄 이하 |
| P0 | `crates/storage/src/interaction_repository.rs` | 686,563B | 16,862 | 40KB / 900줄 이하 |
| P0 | `crates/storage/src/orchestration.rs` | 523,360B | 14,051 | 35KB / 800줄 이하 |
| P0 | `crates/core/src/provider_discovery.rs` | 491,502B | 11,811 | 35KB / 800줄 이하 |
| P0 | `crates/core/src/orchestration_runtime.rs` | 302,113B | 7,484 | 40KB / 900줄 이하 |
| P0 | `apps/lorepia/src/features/orchestration/OrchestrationStudio.svelte` | 263,299B | 4,839 | 30KB / 600줄 이하 |
| P0 | `apps/lorepia/src/app/app-controller.ts` | 126,165B | 3,216 | 40KB / 900줄 이하 |
| P0 | `apps/lorepia/src/features/chat/ChatPane.svelte` | 115,371B | 2,665 | 30KB / 600줄 이하 |

### 2.2 P1 거대 파일

| 파일 | 현재 크기/줄 수 | 계획 |
|---|---:|---|
| `apps/lorepia/src-tauri/src/provider_commands.rs` | 223,628B / 5,933 | command 이름 유지, provider use case별 module 분리 |
| `apps/lorepia/src-tauri/src/credential_operations.rs` | 192,563B / 4,813 | credential operation lifecycle별 분리 |
| `crates/providers/src/parameter_mapping.rs` | 235,689B / 6,545 | wire dialect와 parameter evaluation 분리 |
| `crates/providers/src/registry.rs` | 218,947B / 5,578 | built-in registry, manifest validation, resolution 분리 |
| `crates/providers/src/setup_assistant.rs` | 184,720B / 5,010 | evidence, candidate, plan, review 단계 분리 |
| `crates/core/src/content_package.rs` | 230,230B / 5,599 | inspect, review, commit, lifecycle 분리 |
| `crates/storage/src/package_repository.rs` | 265,267B / 6,646 | read, plan, transaction, recovery 분리 |
| `apps/lorepia/src/lib/ipc/contracts.ts` | 124,911B / 4,041 | bounded context별 contract 파일 + barrel 유지 |
| `apps/lorepia/src/styles/app.css` | 120,338B / 4,344 | feature stylesheet로 이동 |
| `apps/lorepia/src/features/providers/ProviderSettings.svelte` | 84,143B / 2,073 | settings shell + section components |

### 2.3 이미 잘된 구조는 유지한다

- 계층 ADR과 Storage transaction ADR
- deterministic prompt orchestration과 golden fixture
- OS keychain credential envelope와 `zeroize`
- URL policy, DNS 재검증, local/public network mode
- Tauri capability allowlist와 command registry 검사
- schema migration registry와 frozen migration 검증
- Rust multi-platform CI, frontend CI, security CI
- source architecture ratchet

리팩터링의 목적은 이 구조를 약화하는 것이 아니라 **작은 모듈에서 더 쉽게 검증하도록 만드는 것**이다.

---

## 3. 절대로 깨면 안 되는 불변조건

모든 Task의 에이전트는 작업 전 이 절을 읽는다.

### 3.1 계층과 의존 방향

- `crates/domain`은 다른 workspace crate에 의존하지 않는다.
- `crates/orchestration`은 I/O, 네트워크, SQLite, Tauri, wall clock을 직접 사용하지 않는다.
- `storage`와 `providers`는 Core가 사용하는 adapter다.
- UI는 `shell-api` DTO와 명시적인 IPC contract만 사용한다.
- renderer에 DB row, absolute host path, raw file handle, raw credential을 노출하지 않는다.
- 새 모듈을 만들기 위해 계층을 역참조하지 않는다.

### 3.2 Storage transaction과 crash consistency

- SQLite transaction, CAS publish/fsync, durable intent/phase, startup recovery는 Storage가 소유한다.
- 하나의 원자적 use case를 Core가 여러 Storage method 호출 순서로 다시 조립하게 만들지 않는다.
- transaction helper를 분리할 때는 동일 transaction scope 안에서 `&Transaction`을 전달한다.
- `TransactionBehavior`, lock acquisition order, fsync 순서, journal phase를 리팩터링 중 변경하지 않는다.
- incomplete state는 정상 데이터처럼 노출하지 않는다.
- unknown outcome은 기존 정책대로 fail-closed 또는 명시적 review 상태를 유지한다.

### 3.3 Credential와 secret 수명

- credential carrier에 `Clone`, `Serialize`, `Display`를 추가하지 않는다.
- `Debug`는 계속 redacted여야 한다.
- secret value는 기존 drop boundary에서 `zeroize`되어야 한다.
- native dispatch lease와 credential zeroize의 drop 순서를 바꾸지 않는다.
- credential authority epoch, binding hash, operation lease 검증을 생략하지 않는다.
- credential을 일반 `String` DTO나 로그에 복사하지 않는다.

### 3.4 Generation과 비동기 동작

- durable generation attempt가 기록되기 전에 provider dispatch가 시작되지 않도록 한다.
- operation nonce와 resume attempt는 계속 상호 배타적이어야 한다.
- cancellation, partial checkpoint, catch-up snapshot, stream ordering을 바꾸지 않는다.
- process/provider/conversation admission limit을 바꾸지 않는다.
- timeout 이후 결과의 side-effect certainty 분류를 바꾸지 않는다.
- lock guard의 유지 범위를 코드 이동 과정에서 넓히지 않는다.

### 3.5 결정론

- stable sorting, tie-break, hash input, canonical serialization 순서를 바꾸지 않는다.
- `HashMap`을 새로 도입해 직렬화나 ranking 순서가 비결정적으로 되지 않도록 한다.
- golden fixture/hash가 바뀌면 단순 리팩터링 실패로 간주한다. 의미 변경 Task가 따로 승인된 경우만 갱신한다.

### 3.6 IPC와 플랫폼

- command 이름, DTO field 이름, serde rename, API version을 리팩터링 중 변경하지 않는다.
- `config/ipc-commands.json`, generated registries, Tauri handler, development/release capability의 일치를 유지한다.
- generated permission 파일을 수동으로 의미 변경하지 않는다.
- Android/iOS/macOS/Windows에서 조건부 compile 경계가 달라지지 않도록 한다.

### 3.7 Database migration

- migration 0001~0011은 동결이다.
- 단순 코드 이동 Task에서 `SCHEMA_VERSION`을 변경하지 않는다.
- 기존 migration SQL을 formatting 목적으로 수정하지 않는다.
- migration registry와 replay fixture가 바뀌지 않아야 한다.

### 3.8 공개 API

- 기존 public method signature는 facade로 유지한다.
- 내부 구현 이동 때문에 `pub`를 무작정 추가하지 않는다.
- child module에 필요한 visibility는 `pub(super)` 또는 `pub(crate)` 중 가장 좁은 범위를 사용한다.
- `pub use *`를 추가하지 않는다.
- `Stored*`, transaction command, persistence row를 Core/Shell에 새로 재노출하지 않는다.

---

## 4. AI 작업 프로토콜

### 4.1 작업 전 확인

AI는 코드 수정 전에 아래 결과를 먼저 만든다.

```text
Task ID:
Baseline commit:
Target files:
Public entry points:
Owned invariants:
Relevant tests:
Symbols to move:
Symbols that must remain:
Expected size reduction:
Potential semantic risks:
```

### 4.2 읽기 순서

1. 이 문서에서 해당 Phase와 Task 설명
2. `config/refactoring/task-manifest.yaml`의 Task 항목
3. 루트 `AGENTS.md`
4. 가장 가까운 하위 `AGENTS.md` 또는 code map
5. 관련 ADR
6. Task의 `read_first`
7. 해당 코드의 호출자와 테스트

AI는 처음부터 저장소 전체를 읽지 않는다. 거대 파일은 먼저 symbol inventory를 만들고 필요한 범위를 나누어 읽는다.

### 4.3 권장 inventory 명령

Rust:

```bash
rg -n '^(pub(\([^)]*\))?\s+)?(async\s+)?(fn|struct|enum|trait|type)\b|^impl\b|^mod\b' <file>
rg -n '#\[cfg\(test\)\]|mod tests|^\s*#\[test\]|^\s*#\[tokio::test\]' <file>
rg -n '<symbol_name>' crates apps plugins
```

TypeScript/Svelte:

```bash
rg -n '^(export\s+)?(class|interface|type|function|const)\b|^\s*(public|private|protected)?\s*(async\s+)?[A-Za-z_][A-Za-z0-9_]*\(' <file>
rg -n 'describe\(|it\(|test\(' <test-file>
rg -n 'from ["'"'].*<module>["'"']|<symbol_name>' apps/lorepia/src
```

### 4.4 안전한 extraction 순서

1. 기존 테스트를 실행해 green baseline을 기록한다.
2. 이동할 symbol과 직접 dependency를 표로 만든다.
3. 새 child module을 만들고 코드를 **그대로** 이동한다.
4. 필요한 import와 최소 visibility만 조정한다.
5. 기존 public entry point는 wrapper/delegation 또는 re-export로 유지한다.
6. targeted test를 실행한다.
7. formatting과 lint를 실행한다.
8. 호출 경로가 안정되면 테스트를 feature별 파일로 이동한다.
9. parent file의 source-size baseline을 실제 감소분에 맞춰 낮춘다.
10. 전체 validation을 실행하고 size delta를 보고한다.

### 4.5 중단 조건

다음 상황이면 현재 Task에서 임의 해결하지 않고 중단 보고한다.

- public API 또는 serialized DTO를 바꿔야만 extraction이 가능함
- DB schema 변경이 필요함
- 테스트가 기존 코드에서도 불안정하거나 실패함
- credential lifetime이나 transaction boundary가 불명확함
- deterministic golden hash가 변경됨
- 새로운 crate dependency가 필요함
- Task 허용 경로 밖을 수정해야 함
- 하나의 Task가 운영 코드 12개 이상 파일을 실질적으로 변경하게 됨

---

## 5. 모듈 설계 규칙

### 5.1 좋은 모듈 경계

파일 이름은 사용 사례나 불변조건을 표현한다.

좋은 예:

- `generation/admission.rs`
- `generation/operation_identity.rs`
- `discovery/transition_store.rs`
- `interaction/checkpoint_repository.rs`
- `chat/composer-state.svelte.ts`
- `providers/model-sync-controller.ts`

나쁜 예:

- `part1.rs`
- `helpers2.ts`
- `misc.rs`
- `common.ts`
- `manager.rs`
- `utils.rs`에 서로 무관한 함수 누적

### 5.2 facade 원칙

거대 파일은 바로 삭제하지 않고 작은 facade로 축소한다.

facade가 맡을 수 있는 것:

- public type과 method의 안정된 위치
- subsystem 조립
- child module delegation
- 제한된 re-export
- lifecycle 시작/종료

facade가 맡으면 안 되는 것:

- 장문의 SQL
- provider wire encoding
- 상태 머신 전이 구현
- 수백 줄 validation
- UI gesture나 scrolling 알고리즘
- 대규모 test fixture

### 5.3 visibility 원칙

- 외부 crate/API가 필요하지 않으면 `pub` 금지
- sibling module 공유는 작은 context-owned type으로 제한
- child module이 parent private item을 이용할 수 있으면 불필요한 public화를 하지 않는다.
- parent가 child item을 써야 하면 `pub(super)`를 우선한다.
- facade의 re-export는 이름을 명시한다. `pub use module::*` 금지

### 5.4 추상화 원칙

- 동일한 불변조건과 실패 모델을 가진 코드만 공통화한다.
- 서로 다른 use case를 generic repository 하나로 합치지 않는다.
- trait는 대체 구현이나 테스트 seam이 실제로 필요할 때만 추가한다.
- macro는 보안/transaction 흐름을 숨기지 않아야 한다.
- harmless한 5~15줄 중복은 복잡한 generic보다 나을 수 있다.

### 5.5 module README / AGENTS 형식

각 큰 bounded context에는 100~200줄 이하 문서를 둔다.

```markdown
# Scope

# Public entry points

# State and transaction owner

# Security / determinism invariants

# Module map

# Common change recipes

# Targeted tests

# Forbidden dependencies
```

구현 세부를 장황하게 복제하지 않는다. AI가 “어디를 읽을지” 판단하는 지도 역할만 한다.

---

## 6. 목표 디렉터리 구조

아래 구조는 최종 방향이다. 실제 이동은 Task 단위로 점진적으로 수행한다.

### 6.1 Core

```text
crates/core/src/
  app.rs                              # Core/CoreInner 조립과 public facade
  app/
    core_state.rs
    shutdown.rs
    imports/
      mod.rs
      inspect.rs
      commit.rs
      pending.rs
    generation/
      mod.rs
      types.rs
      operation_identity.rs
      credential.rs
      admission.rs
      target_resolution.rs
      preparation.rs
      dispatch.rs
      events.rs
      checkpoint.rs
      recovery.rs
      tests/
    providers/
      mod.rs
      connection.rs
      credential.rs
      catalog.rs
      model_sync.rs
      routing.rs
    runtime/
      mod.rs
      control.rs
      generation.rs
      state.rs
      tests/

  provider_discovery.rs               # stable public facade
  provider_discovery/
    types.rs
    views.rs
    begin.rs
    known_provider.rs
    deterministic.rs
    assistant.rs
    action.rs
    approval.rs
    commit.rs
    credential.rs
    recovery.rs
    schema_fixture.rs
    tests/

  orchestration.rs                    # stable facade
  orchestration/
    documents.rs
    presets.rs
    variables.rs
    transforms.rs
    memory.rs
    knowledge.rs
    modules.rs
    tests/

  orchestration_runtime.rs            # stable facade
  orchestration_runtime/
    plan.rs
    interaction.rs
    module_runtime.rs
    prompt.rs
    auxiliary_tasks.rs
    persistence.rs
    recovery.rs
    tests/
```

### 6.2 Storage

```text
crates/storage/src/
  database.rs                         # Storage open/bootstrap/schema facade
  database/
    connection.rs
    pragmas.rs
    bootstrap.rs
    schema.rs
    migration_registry.rs
    migration_runner.rs
    migration_verification.rs
    health.rs
    stats.rs
    asset_delivery.rs
    connection_metrics.rs
    private_path.rs
    tests/

  discovery_repository.rs             # stable Storage methods + explicit exports
  discovery_repository/
    types.rs
    contract_codec.rs
    validation.rs
    row_mapping.rs
    queries.rs
    transition_store.rs
    approval_store.rs
    commit_store.rs
    credential_execution.rs
    recovery.rs
    repository_io.rs
    errors.rs
    tests/

  interaction_repository.rs           # stable facade
  interaction_repository/
    types.rs
    conversations.rs
    branches.rs
    messages.rs
    projections.rs
    proposals.rs
    effects.rs
    effect_history.rs
    checkpoints.rs
    derived_outbox.rs
    recovery.rs
    row_mapping.rs
    tests/

  orchestration.rs                    # stable facade
  orchestration/
    types.rs
    builtins.rs
    prompt_documents.rs
    presets.rs
    variables.rs
    transforms.rs
    memory.rs
    knowledge.rs
    modules.rs
    module_authority.rs
    runtime_plans.rs
    row_mapping.rs
    tests/

  package_repository.rs               # stable facade
  package_repository/
    types.rs
    inspection.rs
    queries.rs
    review.rs
    commit.rs
    cas_promotion.rs
    lifecycle.rs
    recovery.rs
    row_mapping.rs
    tests/
```

### 6.3 Frontend

```text
apps/lorepia/src/
  app/
    App.svelte
    app-controller.ts                 # facade/composition only
    app-state.ts
    controllers/
      bootstrap-controller.ts
      library-controller.ts
      import-controller.ts
      conversation-controller.ts
      generation-controller.ts
      chat-stream-controller.ts
      memory-controller.ts
      provider-controller.ts
      discovery-controller.ts
    operations/
      epoch-guard.ts
      serialized-mutation.ts
      operation-identity.ts
    tests/

  features/chat/
    ChatPane.svelte                   # composition only
    ChatViewport.svelte
    ChatMessageList.svelte
    ChatMessageActions.svelte
    ChatComposer.svelte
    ChatFullscreenComposer.svelte
    ChatUtilityDrawer.svelte
    chat-scroll.svelte.ts
    composer-state.svelte.ts
    message-actions.ts
    utility-swipe.ts
    portable-runtime-lifecycle.svelte.ts
    interaction-room-controller.ts
    tests/

  features/orchestration/
    OrchestrationStudio.svelte        # composition only
    studio/
      StudioNavigation.svelte
      PromptDocumentsSection.svelte
      MemorySection.svelte
      KnowledgeSection.svelte
      TransformSection.svelte
      ModuleSection.svelte
      InteractionSection.svelte
      RuntimePreviewSection.svelte
    orchestration-controller.ts       # facade
    controllers/
    tests/

  features/providers/
    ProviderSettings.svelte           # composition only
    settings/
      ConnectionSection.svelte
      CredentialSection.svelte
      ModelRouteSection.svelte
      CapabilitySection.svelte
      DiscoverySection.svelte
      CatalogSection.svelte
      ModelSyncSection.svelte
    tests/

  lib/ipc/
    contracts.ts                      # explicit barrel exports only
    contracts/
      common.ts
      platform.ts
      import.ts
      character.ts
      conversation.ts
      generation.ts
      memory.ts
      orchestration.ts
      provider.ts
      discovery.ts
      portable-runtime.ts
    client.ts                         # LorepiaClient facade
    clients/
      bootstrap-client.ts
      content-client.ts
      conversation-client.ts
      generation-client.ts
      memory-client.ts
      orchestration-client.ts
      provider-client.ts
```

### 6.4 Tauri shell

```text
apps/lorepia/src-tauri/src/
  provider_commands.rs                # command re-exports/registry-facing facade
  provider_commands/
    connections.rs
    credentials.rs
    model_routes.rs
    model_sync.rs
    capabilities.rs
    catalog.rs
    discovery.rs

  credential_operations.rs            # stable facade
  credential_operations/
    types.rs
    prepare.rs
    execute.rs
    authority.rs
    recovery.rs
    cleanup.rs
    tests/
```

기존 command 함수 이름과 `#[tauri::command]` 노출은 유지한다.

---

## 7. 전체 실행 순서와 승인 게이트

### Phase A — 안전장치와 AI 지도

목표: 실제 구현을 움직이기 전에 에이전트가 좁은 문맥으로 작업할 수 있게 한다.

| Task | 내용 | 완료 기준 |
|---|---|---|
| `GOV-001` | 현재 hotspot/size/public API baseline 보고서 생성 | 재현 가능한 report script와 committed snapshot |
| `GOV-002` | 루트 및 core/storage/frontend `AGENTS.md` 작성 | 각 문서 200줄 이하, 경계·테스트·금지사항 포함 |
| `GOV-003` | `config/ai-context-map.json`과 검사 스크립트 | Task별 파일 목록, 총 byte 계산, 누락 경로 검사 |
| `GOV-004` | test source size 측정 추가 | `.test.ts`, Rust external/child test도 별도 ratchet 가능 |

**Gate A:** production logic 변경 없이 전체 CI green. 문서 경로 검사와 context map unit test가 있어야 한다.

### Phase B — 테스트 분리

목표: 운영 코드를 이동할 때 거대한 테스트 파일 전체를 읽지 않도록 한다.

| Task | 대상 |
|---|---|
| `TEST-CORE-001` | `crates/core/src/app.rs` inline tests를 `app/tests/` 기능별 파일로 이동 |
| `TEST-STOR-001` | `discovery_repository` tests 분리 |
| `TEST-STOR-002` | `database` tests 분리 |
| `TEST-STOR-003` | `interaction_repository` tests 분리 |
| `TEST-STOR-004` | `storage/orchestration/tests.rs`를 기능별로 분리 |
| `TEST-FE-001` | `app-controller.test.ts`, provider test를 기능별 분리 |
| `TEST-FE-002` | `ChatPane.test.ts`를 rendering/scroll/composer/actions/runtime로 분리 |
| `TEST-FE-003` | `OrchestrationUI.test.ts`를 section별 분리 |

테스트 이름, assertion, fixture 의미를 바꾸지 않는다. 공통 fixture를 뽑을 때도 mutable global fixture를 만들지 않는다.

**Gate B:** 운영 파일의 의미 변경 0, test count와 주요 test name 목록 유지, 전체 CI green.

### Phase C — Storage 분해와 공개 API 축소

Storage부터 하는 이유는 Core가 persistence row와 transaction command에 덜 의존해야 Core 분해가 쉬워지기 때문이다.

| Task | 내용 |
|---|---|
| `STOR-API-001` | discovery에서 Core가 필요한 의미 기반 view/method 확정 |
| `STOR-DISC-001` | types, validation, codec 추출 |
| `STOR-DISC-002` | read query와 row mapping 추출 |
| `STOR-DISC-003` | transition/approval persistence 추출 |
| `STOR-DISC-004` | commit, credential execution, recovery 추출 |
| `STOR-DB-001` | connection, pragma, bootstrap 추출 |
| `STOR-DB-002` | schema, migration registry/runner/verification 추출 |
| `STOR-DB-003` | health, stats, test support 추출 |
| `STOR-INT-001` | conversation/branch/message read와 projection 추출 |
| `STOR-INT-002` | proposals/effects/checkpoints write 추출 |
| `STOR-INT-003` | derived outbox와 recovery 추출 |
| `STOR-ORCH-001` | prompt documents/presets/variables/transforms 추출 |
| `STOR-ORCH-002` | memory/knowledge/modules/runtime plans 추출 |
| `STOR-PKG-001` | package query/review/commit/CAS/recovery 분리 |

**Gate C:** Storage transaction 경계와 migration fixture 유지, Core의 새 `Stored*` 노출 0, source baseline 하향, 전체 CI green.

### Phase D — Core 분해

| Task | 내용 |
|---|---|
| `CORE-APP-001` | `Core`, `CoreInner`, subsystem 조립과 lifecycle만 `app.rs`에 남길 준비 |
| `CORE-GEN-001` | generation types, operation identity, credential, admission 추출 |
| `CORE-GEN-002` | target resolution, preset validation, prompt preparation 추출 |
| `CORE-GEN-003` | dispatch, event, checkpoint, recovery 추출 |
| `CORE-PROV-001` | provider connection/catalog/model sync/routing 추출 |
| `CORE-IMPORT-001` | pending import, inspect, commit 추출 |
| `CORE-DISC-001` | discovery types/views/begin/action 추출 |
| `CORE-DISC-002` | known provider, deterministic, assistant resolution 추출 |
| `CORE-DISC-003` | approval, commit, credential, recovery 추출 |
| `CORE-ORCH-001` | orchestration document use case 분리 |
| `CORE-ORCH-002` | runtime plan/interaction/module runtime 분리 |
| `CORE-ORCH-003` | auxiliary task/persistence/recovery 분리 |
| `CORE-CONTENT-001` | content package inspect/review/commit/lifecycle 분리 |

**Gate D:** `app.rs`, `provider_discovery.rs`, `orchestration_runtime.rs`가 facade 목표에 근접하고 public Core API와 golden hash가 유지되어야 한다.

### Phase E — Shell, IPC, Frontend 분해

| Task | 내용 |
|---|---|
| `SHELL-001` | shell-api DTO를 bounded context별 파일로 정리, 기존 re-export 유지 |
| `TAURI-001` | provider commands 분리, command 이름/권한 유지 |
| `TAURI-002` | credential operations 분리, lease/drop/authority 유지 |
| `IPC-001` | TypeScript `contracts.ts` 분리, barrel 호환 유지 |
| `IPC-002` | `client.ts` 내부를 bounded context clients로 분리 |
| `FE-APP-001` | epoch guard, mutation tail, operation identity 등 pure primitive 추출 |
| `FE-APP-002` | library/import/conversation controller 추출 |
| `FE-APP-003` | generation/chat stream controller 추출 |
| `FE-APP-004` | memory/provider/discovery controller 추출 |
| `FE-CHAT-001` | virtual scroll/anchor/lifecycle 추출 |
| `FE-CHAT-002` | composer/fullscreen/message action/utility drawer 분리 |
| `FE-CHAT-003` | portable runtime/interaction room 분리 |
| `FE-ORCH-001` | orchestration controller state/use case 분리 |
| `FE-ORCH-002` | OrchestrationStudio section components 분리 |
| `FE-PROV-001` | ProviderSettings section components 분리 |
| `FE-CSS-001` | `app.css`를 app/chat/orchestration/provider/shared stylesheet로 이동 |

**Gate E:** frontend typecheck/test green, Svelte reactivity와 focus/scroll/accessibility behavior 유지, IPC version 불변.

### Phase F — 최종 강제 규칙

| Task | 내용 |
|---|---|
| `ENF-001` | source-size baseline v2: 언어별 limits, 테스트 포함, baseline 증가 금지 |
| `ENF-002` | public API diff와 forbidden dependency 검사 강화 |
| `ENF-003` | AI context map drift 검사와 targeted command 출력 |
| `ENF-004` | 문서/Task manifest 완료 상태와 잔여 hotspot 보고서 |

**Gate F:** 모든 필수 GitHub check green, P0 facade 목표 달성, context budget 검사 통과.

---

## 8. P0 파일별 상세 실행법

## 8.1 `crates/core/src/app.rs`

### 현재 문제

`Core` 조립, import, provider, generation, model sync, runtime, credential lifetime, target resolution, helper와 테스트가 한 파일에 공존한다. 이미 `app/generation_events.rs`, `generation_workflow.rs`, `model_sync.rs`, `runtime_*` child module이 있으므로 이 패턴을 확대한다.

### 안전한 분해 순서

1. **TEST-CORE-001:** inline test와 test provider를 `app/tests/`로 이동한다.
2. **CORE-GEN-001:** 데이터 타입과 pure identity/validation부터 이동한다.
   - `GenerationOperationContext`
   - operation nonce/fingerprint
   - credential carrier와 binding validation
   - admission key/limits 관련 코드
3. **CORE-GEN-002:** provider target resolution과 request/prompt preparation을 이동한다.
4. **CORE-GEN-003:** async dispatch와 checkpoint/recovery를 이동한다.
5. **CORE-PROV-001:** provider CRUD/catalog/model sync 경로를 이동한다.
6. **CORE-IMPORT-001:** pending import map과 inspect/commit/discard를 이동한다.
7. **CORE-APP-001 마무리:** `app.rs`에는 Core/CoreInner, constructor/open, shutdown, child service delegation만 남긴다.

### 반드시 유지할 것

- `Core` public method signature
- `ConnectionBoundCredential`의 non-Clone/non-Serialize와 redacted Debug
- zeroize와 dispatch lease drop 순서
- generation admission limit
- durable attempt 이전 dispatch 금지
- event bus/stream ordering
- shutdown grace와 watchdog semantics

### 권장 중간 size ratchet

| 단계 | `app.rs` 상한 |
|---|---:|
| tests 이동 후 | 650KB 이하 |
| generation identity/credential 이동 후 | 520KB 이하 |
| generation dispatch 이동 후 | 350KB 이하 |
| provider/import 이동 후 | 180KB 이하 |
| 최종 facade | 40KB 이하 |

각 단계에서 실제 감소치만 baseline에 반영한다.

---

## 8.2 `crates/storage/src/discovery_repository.rs`

### 현재 문제

repository API type, bounded validation, deterministic contract codec, SQL query, row hydration, state transition, approval, provider graph commit, credential execution, startup recovery가 한 파일에 있다. child module `contract_codec`, `errors`, `repository_io`, `tests/`가 이미 존재하므로 facade 패턴을 확장한다.

### 분해 순서

1. `types.rs`: repository input/output와 public result만 이동
2. `validation.rs`: identifier, bounded JSON, transition validation
3. `row_mapping.rs`: DB row ↔ domain/view 변환
4. `queries.rs`: side effect 없는 read/list query
5. `transition_store.rs`: transition, event, evidence, candidate persistence
6. `approval_store.rs`: approval lifecycle
7. `commit_store.rs`: provider graph publication과 commit bookkeeping
8. `credential_execution.rs`: native credential authority/attestation
9. `recovery.rs`: unfinished operation scan, interruption, unknown outcome
10. `discovery_repository.rs`: `impl Storage` public facade와 명시적 export만 유지

### transaction 규칙

- 한 transition을 저장하는 SQL sequence는 한 함수/모듈이 소유한다.
- helper가 필요하면 동일 `Transaction` reference를 받는다.
- commit bookkeeping과 provider graph publication을 다른 transaction으로 나누지 않는다.
- recovery query의 bounded user list와 complete internal scan을 혼동하지 않는다.

### 테스트 분류

```text
tests/
  begin.rs
  validation.rs
  evidence.rs
  candidates.rs
  transitions.rs
  approvals.rs
  commit.rs
  credentials.rs
  interruption.rs
  unknown_outcome.rs
  recovery.rs
```

---

## 8.3 `crates/storage/src/database.rs`

### 현재 문제

Storage open, SQLite connection configuration, migration registry, schema verification, startup checks, health/stats, compatibility/cutover test support가 혼재한다.

### 분해 순서

1. `connection.rs`: connection acquisition와 pool/locking 경계
2. `pragmas.rs`: SQLite pragma 적용과 확인
3. `bootstrap.rs`: data root, DB open, startup sequence
4. `migration_registry.rs`: migration 목록과 metadata
5. `migration_runner.rs`: 적용 순서와 transaction
6. `migration_verification.rs`: schema integrity와 frozen baseline 확인
7. `schema.rs`: schema version/read-only metadata
8. `health.rs`, `stats.rs`: 제품-facing 상태
9. 기존 `asset_delivery`, `connection_metrics`, `private_path` 유지

### 금지사항

- migration SQL 문자열을 이동하면서 내용 변경
- migration 0001~0011 수정
- startup recovery 순서 변경
- pragma 값을 정리 목적으로 변경
- Storage connection lifetime 변경

---

## 8.4 `crates/storage/src/interaction_repository.rs`

### 목표 경계

- conversation/branch/message
- display projection
- interaction proposal/effect/history
- checkpoint
- derived event outbox
- recovery

### 분해 원칙

- read model과 write transaction을 같은 거대 “repository helpers” 파일로 다시 모으지 않는다.
- message mutation과 generation attempt durable closure의 transaction 경계를 보존한다.
- source branch/head/revision optimistic concurrency check를 한곳에서 소유한다.
- effect history는 이미 존재하는 child module을 유지하고 facade에서 명시적으로 호출한다.

### 완료 기준

- facade는 의미 기반 Storage method만 노출
- persistence row type을 Core가 직접 조립하지 않음
- recovery와 user-facing history query가 분리됨
- tests가 proposal/effect/checkpoint/outbox/recovery별로 독립 실행 가능

---

## 8.5 `crates/storage/src/orchestration.rs`

현재 child module은 `builtins.rs`, `module_authority.rs`, `tests.rs` 정도만 존재한다. 다음 bounded context로 분리한다.

- prompt documents/presets
- variables
- transforms
- memory
- knowledge
- modules/module authority
- applied runtime plans
- built-in records

`types.rs`를 무제한 dump 파일로 만들지 않는다. 특정 context에만 쓰는 row/write type은 해당 module 안에 둔다.

---

## 8.6 `crates/core/src/provider_discovery.rs`

현재 child directory에는 `schema_fixture.rs`만 있다. Storage 분해가 끝난 뒤 다음 순서로 이동한다.

1. `types.rs`, `views.rs`
2. `begin.rs`, `action.rs`
3. `known_provider.rs`, `deterministic.rs`
4. `assistant.rs`
5. `approval.rs`, `credential.rs`
6. `commit.rs`, `recovery.rs`

Core는 discovery state machine을 제품 use case로 조정하되 SQL/row를 소유하지 않는다. remote assistant와 deterministic discovery의 evidence trust 수준을 합치지 않는다.

---

## 8.7 Frontend P0

### `app-controller.ts`

루트 controller를 제거하지 않는다. 먼저 다음 순서로 위임한다.

1. pure helpers: error normalization, compatibility, guards
2. operation identity와 epoch guard
3. library/import/conversation
4. generation/chat stream
5. memory supervisor/retry
6. provider settings/discovery

각 child controller는 다음 중 하나만 소유한다.

- 특정 feature의 async operation과 epoch
- 해당 feature state slice update
- 해당 feature의 side-effect subscription

여러 controller가 같은 epoch나 subscription을 동시에 소유하면 안 된다. root controller는 composition과 cross-feature orchestration만 유지한다.

### `ChatPane.svelte`

분해 순서:

1. scroll/virtual measurement/anchor를 `chat-scroll.svelte.ts`로 이동
2. composer state와 submit/focus를 분리
3. message actions를 분리
4. utility swipe/drawer를 분리
5. portable runtime과 interaction room UI를 분리
6. 최종 `ChatPane.svelte`는 state 연결과 layout만 유지

Svelte 5 rune state를 이동할 때 closure capture와 `$derived` dependency를 바꾸지 않는다. DOM measurement는 component mount/unmount lifecycle을 보존한다.

### `OrchestrationStudio.svelte`

한 번에 section을 전부 추출하지 않는다.

1. navigation과 selected section
2. prompt documents
3. memory/knowledge
4. transforms
5. modules/lifecycle
6. interactions/runtime preview
7. shared editor primitives

각 section은 자체 props/event contract를 명시하고 Core/IPC client를 직접 새로 생성하지 않는다. controller가 use case를 소유한다.

---

## 9. 테스트 전략

### 9.1 테스트 계층

| 계층 | 목적 |
|---|---|
| pure unit | validation, mapping, identity, deterministic ordering |
| repository unit/integration | SQL transaction, row hydration, recovery |
| Core vertical slice | Storage+provider+Core use case와 durable behavior |
| Shell/IPC contract | DTO, command registry, capability 일치 |
| Frontend controller | epoch, stale result, retry, stream reconciliation |
| Component | rendering, focus, scroll anchor, accessibility |
| Cross-platform CI | compile/behavior regression |

### 9.2 리팩터링 characterization test

추출 전에 다음을 고정한다.

- public output/result/error code
- serialized JSON/DTO shape
- hash/fingerprint
- SQL-visible durable phase
- event sequence
- cancellation result
- retry/resume behavior
- UI focus/scroll position

### 9.3 테스트 파일 규칙

- 테스트 파일 이름은 behavior를 나타낸다.
- giant `test_support.rs`에 모든 fixture를 넣지 않는다.
- fixture builder는 bounded context별로 둔다.
- mutable global state를 만들지 않는다.
- test provider는 필요한 behavior만 구현한다.
- assertion을 helper 안에 숨겨 실패 원인을 흐리지 않는다.

### 9.4 targeted validation matrix

| 변경 영역 | 최소 targeted 검사 |
|---|---|
| Core | `cargo test -p lorepia-core` + 해당 dependency crate test |
| Storage | `cargo test -p lorepia-storage` + 관련 Core vertical slice |
| Orchestration | `cargo test -p lorepia-orchestration` + golden |
| Providers | `cargo test -p lorepia-providers` + Core routing tests |
| Shell API | shell-api crate tests + IPC registry check |
| Tauri | 해당 manifest/package test + generated command check |
| Frontend controller/component | `npm run check --prefix apps/lorepia` + 지정 Vitest 파일 |
| Scripts | 해당 Python unit test |

package 이름이 실제 manifest와 다르면 에이전트는 먼저 `Cargo.toml`에서 확인해 정확한 명령을 보고한다.

### 9.5 병합 전 전체 검사

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

npm run check --prefix apps/lorepia
npm run test --prefix apps/lorepia

python3 scripts/generate_ipc_commands.py --check
python3 scripts/check_source_architecture.py --base-ref <merge-base>
```

GitHub에서는 다음 보호 브랜치 필수 check가 모두 통과해야 한다.

- Frontend
- Android dependency integrity
- Rust (ubuntu-latest)
- Rust (macos-latest)
- Rust (windows-latest)
- Dependency review
- RustSec audit
- Cargo deny
- CodeQL (JavaScript/TypeScript)
- iOS simulator

---

## 10. Source-size와 AI context 강제 규칙

### 10.1 `source-size-baseline` v2

최종 검사기는 다음을 지원해야 한다.

- language/file-kind별 limit
- production/test/generated 분류
- 기존 baseline의 증가 금지
- baseline entry 제거 시 재추가 금지 또는 explicit approval
- parent file 감소량과 새 child module 총량 보고
- 파일 수만 늘려 우회하는 행위 탐지용 directory aggregate report

권장 설정 예시:

```json
{
  "version": 2,
  "limits": {
    "rust_production": { "bytes": 40960, "lines": 900 },
    "typescript_production": { "bytes": 40960, "lines": 900 },
    "svelte_component": { "bytes": 30720, "lines": 600 },
    "test_source": { "bytes": 61440, "lines": 1200 },
    "facade": { "bytes": 30720, "lines": 500 }
  }
}
```

초기에는 기존 baseline을 유지하고 새 파일부터 적용한다. P0 parent가 목표에 도달하면 해당 baseline entry를 삭제한다.

### 10.2 AI context map

`config/ai-context-map.json`은 feature별 다음 정보를 가진다.

```json
{
  "generation": {
    "entrypoints": [
      "crates/core/src/app/generation/mod.rs",
      "crates/chat/src/generation.rs"
    ],
    "storage": [
      "crates/storage/src/generation_attempt.rs"
    ],
    "tests": [
      "crates/core/tests/chat_vertical_slice.rs"
    ],
    "commands": [
      "cargo test -p lorepia-core",
      "cargo test -p lorepia-chat"
    ],
    "max_context_bytes": 250000
  }
}
```

검사 스크립트는 존재하지 않는 경로, 중복 경로, context byte 초과를 보고한다. context map은 보안 allowlist가 아니라 **읽기 시작점**이다. 변경 영향 분석 결과 필요한 파일은 추가로 읽을 수 있지만 이유를 보고해야 한다.

---

## 11. PR와 커밋 규칙

### 11.1 브랜치

```text
refactor/<task-id-lowercase>-<short-scope>
```

예:

```text
refactor/stor-disc-001-contract-validation
refactor/core-gen-001-operation-identity
refactor/fe-chat-001-scroll-lifecycle
```

### 11.2 권장 커밋 순서

1. `test: capture <behavior>` — 필요한 경우만
2. `refactor: extract <bounded-context>` — 기계적 이동
3. `refactor: delegate <facade>` — 호출 경로 연결
4. `test: split <test-area>` — 테스트 위치 조정
5. `chore: lower source-size baseline` — 실제 감소치 반영
6. `docs: update <code-map>` — ownership가 바뀐 경우

한 커밋에서 rename, formatting, logic change를 섞지 않는다.

### 11.3 PR 본문 필수 항목

```markdown
## Task
- ID:
- Baseline/merge-base:

## Scope

## Non-goals

## Symbol move map
| Before | After | Semantic change |

## Invariants checked

## Tests

## Size delta
| File | Before | After |

## Public API / schema / IPC impact

## Risks and rollback
```

---

## 12. 위험 등록부

| 위험 | 징후 | 방지책 |
|---|---|---|
| public visibility 폭증 | `pub(crate)`/`pub`가 대량 추가됨 | child privacy와 facade 명시 export 사용 |
| transaction 분할 | Core가 SQL 단계 순서를 조립함 | atomic repository method 유지 |
| secret lifetime 변화 | credential clone/temporary String 증가 | credential module 별도 검토, drop test 유지 |
| async cancellation 변화 | test hang, partial state 차이 | cancel/timeout characterization test 선행 |
| lock order 변화 | Windows만 hang, flaky shutdown | lock acquisition map 기록, 한 Task에서 변경 금지 |
| 결정론 손상 | golden/hash 변경 | stable collection/serialization 유지 |
| Svelte reactivity 손상 | UI stale, focus/scroll regression | state owner 단일화, component test 분리 |
| IPC drift | command not allowed/registered | generator check와 capability check 필수 |
| 과도한 micro-file | 파일 왕복 증가 | 100줄 이하 파일은 명확한 불변조건 없으면 합침 |
| 새로운 god `types.rs` | type 파일이 1,000줄 초과 | context-owned type을 해당 module에 배치 |
| baseline gaming | parent는 줄었지만 child aggregate 폭증 | directory aggregate report와 review |
| 테스트 의미 변화 | assertion 수 감소 | test count/name/coverage behavior 비교 |

---

## 13. Task 완료 보고 형식

AI는 최종 응답에 다음을 포함한다.

```markdown
## Completed
- Task ID:
- Commit/branch:

## Moved symbols

## Behavioral proof

## Validation run
- command: result

## Size changes

## Remaining risks

## Follow-up Task candidates
```

“테스트가 통과했다”만 쓰지 않는다. 어떤 불변조건이 어떤 테스트로 보호되는지 설명한다.

---

## 14. 전체 완료 정의

프로젝트 리팩터링은 다음을 모두 충족해야 완료로 본다.

1. 모든 P0 parent file이 facade 목표 이하이거나, 남은 예외가 근거가 있는 ADR로 승인되어 있다.
2. 신규 운영 파일이 언어별 hard target을 넘지 않는다.
3. 테스트 파일도 size ratchet에 포함된다.
4. Core/Shell에서 새 `Stored*` persistence row 재노출이 없다.
5. Storage transaction과 recovery ownership이 유지된다.
6. credential lifetime과 redaction/zeroize 검증이 유지된다.
7. orchestration golden과 deterministic hash가 의도치 않게 변경되지 않는다.
8. IPC command/DTO/API version이 리팩터링 때문에 변경되지 않는다.
9. 모든 feature에 작은 code map과 targeted test 명령이 있다.
10. AI context map의 기본 bundle이 250KB 이하다.
11. 전체 보호 브랜치 CI가 green이다.
12. 기존 source-size baseline entry가 명확히 감소했고 새 giant baseline이 생기지 않았다.
13. 일반적인 generation, provider, chat UI, orchestration 변경이 거대 facade 전체를 읽지 않고 수행 가능하다.

---

## 부록 A. AI에게 Task를 맡길 때 쓰는 표준 프롬프트

```text
너는 LorePia 리팩터링 계획서의 Task <TASK-ID>만 수행한다.

Repository: Dokpamo/LorePia
Start from: latest green main
Normative documents:
- docs/refactoring/lorepia-ai-first-refactoring-master-plan.md
- config/refactoring/task-manifest.yaml
- nearest AGENTS.md
- relevant ADRs

반드시 먼저 수행할 것:
1. 현재 main commit과 merge-base를 기록한다.
2. Task manifest의 read_first/allowed_paths/forbidden_changes를 읽는다.
3. 수정 전에 symbol move map, 관련 테스트, 불변조건, 예상 size delta를 보고한다.
4. 기존 targeted test를 실행해 baseline을 확인한다.

제약:
- 이 Task 밖의 기능 개선을 하지 않는다.
- public API, DB schema, serialized DTO, IPC command 이름을 바꾸지 않는다.
- credential lifetime, transaction boundary, deterministic ordering을 바꾸지 않는다.
- 새 dependency를 추가하지 않는다.
- source-size baseline을 상향하지 않는다.
- 무관한 formatting/rename을 하지 않는다.
- 한 Task가 예상 범위를 벗어나면 임의 확장하지 말고 중단 사유를 보고한다.

구현 방식:
- 먼저 characterization test가 필요한지 판단한다.
- 코드를 이름 변경 없이 child module로 이동한다.
- facade가 기존 entry point를 유지하게 한다.
- 최소 visibility만 사용한다.
- targeted tests -> lint/typecheck -> full validation 순서로 실행한다.

최종 보고:
- moved symbols
- changed files
- behavior proof
- validation commands/results
- before/after size
- public API/schema/IPC impact
- rollback method
- 남은 위험

Task 완료 후 다음 Task를 시작하지 않는다.
```

---

## 부록 B. Extraction map 템플릿

```markdown
# Extraction Map: <TASK-ID>

## Parent file responsibility before

## Target bounded context

## Symbols
| Symbol | Kind | Current dependencies | Destination | Visibility |

## Callers

## Tests

## Invariants

## Move order

## Expected parent size reduction

## Excluded cleanup
```

---

## 부록 C. 작은 module 문서 템플릿

```markdown
# <Module Name>

## Owns

## Does not own

## Entry points

## State / transaction owner

## Security and determinism invariants

## Dependencies

## Tests

## Common modifications
```

---

## 부록 D. 롤백 원칙

- extraction PR은 기존 facade를 유지하므로 child implementation을 원래 module로 되돌릴 수 있어야 한다.
- schema나 durable data가 바뀌지 않는 Task는 코드 revert만으로 롤백 가능해야 한다.
- baseline 감소 커밋은 implementation revert와 함께 되돌린다. baseline만 단독 상향하지 않는다.
- generated registry를 건드린 PR은 manifest와 generated output을 함께 롤백한다.
- UI component extraction은 props/event facade를 유지해 원래 template로 되돌릴 수 있어야 한다.
- 리팩터링 도중 발견한 별도 버그는 현재 PR에서 고치지 않고 issue/Task로 기록한다.

---

## 부록 E. 첫 실행 권장 Task

첫 실제 작업은 `GOV-002` 또는 `TEST-CORE-001`이다. 구현 위험이 가장 낮고 이후 AI context 비용을 바로 낮춘다.

`TEST-CORE-001`의 핵심 제약:

```text
목표:
crates/core/src/app.rs의 운영 동작과 public API를 변경하지 않고,
inline test와 test-only provider/helper를 crates/core/src/app/tests/로 이동한다.

금지:
- production logic 수정
- public signature 변경
- assertion 변경
- test behavior 변경
- source-size baseline 상향

검증:
cargo fmt --all --check
cargo clippy -p lorepia-core --all-targets -- -D warnings
cargo test -p lorepia-core
python3 scripts/check_source_architecture.py --base-ref <merge-base>
```
