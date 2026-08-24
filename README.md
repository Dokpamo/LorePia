# LorePia

로컬 우선 캐릭터 대화 데스크톱 앱. 대화 기록, 캐릭터 자산, 제공자 크리덴셜이 모두 사용자 기기에 남고,
프롬프트 구성은 결정론적으로 재현된다.

## 무엇이 들어 있나

- **결정론적 프롬프트 오케스트레이션** — 기억·지식 선택이 고정소수점 랭킹과 안정적 tie-break로
  정렬되므로 같은 입력은 항상 같은 프롬프트 플랜을 만든다. 플랜은 골든 픽스처로 고정돼 있다.
- **분기 대화** — 메시지 단위 분기, 재생성, 편집. 기억 레코드는 분기 계보를 인식해 선택된다.
- **제공자 중립 어댑터** — OpenAI Responses / OpenAI 호환 Chat / Anthropic Messages /
  Gemini generateContent / OpenRouter / Ollama.
- **콘텐츠 모듈 생명주기** — 패키지 임포트, 검토, 활성화·비활성화·롤백을 개정 단위로 관리.

## 저장소 구조

의존 방향은 항상 아래에서 위로만 흐른다.

| 경로 | 역할 |
| --- | --- |
| `crates/domain` | 플랫폼 독립 타입. 다른 crate에 의존하지 않는다. |
| `crates/orchestration` | 결정론적 프롬프트·기억·지식·모듈 구성. I/O 없음. |
| `crates/content` | 캐릭터 카드와 CHARX 패키지의 안전한 검사·정규화. |
| `crates/storage` | SQLite 영속성과 마이그레이션. |
| `crates/providers` | HTTP 전송, URL 정책, 제공자 어댑터, 디스커버리. |
| `crates/chat` | 생성 스트림 소비와 이벤트 투영. |
| `crates/core` | 위 계층을 묶는 애플리케이션 API. |
| `crates/shell-api` | 셸 바인딩용 DTO·검증 경계. 크리덴셜과 호스트 경로는 넘지 않고, 대화 내용은 필요한 사용 사례의 타입화된 DTO로만 전달한다. |
| `plugins/lorepia-platform` | OS 키체인 크리덴셜 봉투와 플랫폼 고유 동작. |
| `apps/lorepia` | Svelte 5 프론트엔드와 Tauri 2 셸. |

## 개발 환경

- Rust — `rust-toolchain.toml`이 **1.96.0**을 고정한다. 별도 설치 불필요.
- Node — `.node-version`이 **24.18.1**을 지정한다.
- 플랫폼 요구사항은 [Tauri 2 사전 준비](https://tauri.app/start/prerequisites/)를 따른다.

```bash
npm ci --prefix apps/lorepia
```

## 명령

앱 실행:

```bash
npm run tauri dev --prefix apps/lorepia
```

운영 데이터와 분리된 UI 테스트 데이터 데모:

```bash
npm run demo --prefix apps/lorepia
```

데이터 구성과 초기화 방법은 [apps/lorepia/TEST_DATA.md](apps/lorepia/TEST_DATA.md)에 정리돼 있다.

Rust 검사:

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

프론트엔드 검사:

```bash
npm run check --prefix apps/lorepia && npm run test --prefix apps/lorepia
```

배포 번들 (`bundle.active`가 켜진 릴리즈 설정 사용):

```bash
npm run tauri build --prefix apps/lorepia -- --config tauri.release.conf.json
```

## 데이터가 저장되는 곳

앱은 플랫폼 표준 앱 데이터 디렉터리 아래 하나의 데이터 루트를 쓴다. 그 안에 SQLite 데이터베이스,
승인된 에셋, 스테이징·복구 디렉터리가 들어간다. 제공자 크리덴셜은 데이터베이스에 저장되지 않고
**OS 키체인**에 버전드 봉투로 보관되며, 데이터베이스에는 논리 참조만 남는다.

## 보안 모델

이 앱은 신뢰할 수 없는 콘텐츠(캐릭터 카드, 제공자 응답, 패키지)를 다루므로 경계를 명시적으로 강제한다.

- **크리덴셜** — OS 키체인 + 권한 스코프 봉투 + `zeroize`. 제공자 응답에 크리덴셜이 반사되면
  생성이 중단된다.
- **네트워크** — `crates/providers/src/url_policy.rs`가 URL을 정규화하고 DNS 응답을 재확인한다.
  공개 모드는 https + 전역 라우팅 주소만, 로컬 모드는 루프백만 허용한다. 사설망 origin은
  사용자가 정확한 주소 집합을 고정해 승인해야 한다.
- **웹뷰** — 엄격한 CSP, `assetProtocol` 비활성, 승인된 에셋 전용 커스텀 프로토콜.
- **IPC** — Tauri capability가 명시적 커맨드 allowlist를 강제하며 개발용과 릴리즈용이 분리돼 있다.
  `build.rs`가 등록된 커맨드와 capability 부여가 정확히 일치하는지 빌드 시점에 검증한다.
- **임포트** — 아카이브 경로 탈출, 심볼릭 링크, 항목 충돌, 압축비 폭탄, MIME 불일치를 차단한다.

## 스키마 마이그레이션

`crates/storage/migrations/`가 번호순 마이그레이션을 담는다. `SCHEMA_VERSION`은
`crates/storage/src/database.rs`에 있다. 마이그레이션 0001–0011은 스키마 11 컷오버의 리플레이
기준선으로 **동결**돼 있으므로 편집하지 않는다. 스키마 변경은 항상 새 마이그레이션을 추가한다.
