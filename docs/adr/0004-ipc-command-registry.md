# ADR 0004: IPC command registry

- 상태: 승인
- 날짜: 2026-08-29

## 문제

문자열 command가 UI 곳곳에 흩어지고 Rust handler와 Tauri capability가 따로 변경되면,
오타뿐 아니라 release에서 의도하지 않은 권한 노출이 생긴다.

## 결정

기계적인 app command 이름은 `config/ipc-commands.json`을 단일 기준으로 사용한다.
표준 라이브러리 Python 생성기가 이 manifest에서 Rust build registry와 frontend TypeScript
registry를 재현 가능하게 만든다. 생성물은 저장소에 커밋하고 CI에서 `--check`로 drift를
차단한다. release dependency gate도 같은 `--check`를 실행한다. build script는 manifest를
직접 읽어 생성된 `APP_COMMANDS`와 순서까지 대조하므로 stale 생성물은 Tauri permission을
만들기 전에 로컬 빌드에서도 실패한다. payload/result 타입은 `contracts.ts` 및 bounded
contract module에 직접 정의한다.

`tauri::generate_handler!`와 development/release capability allowlist는 생성하지 않는다.
두 표면은 실제 handler 존재와 renderer 권한 부여를 뜻하므로 보안 검토를 거친 명시적인
수동 승인으로 유지한다. build script의 exact-set 검사는 생성된 registry, invoke handler,
두 capability와 Tauri generated permission이 모두 일치해야만 성공한다. 특히 manifest에
command를 추가해도 release capability가 자동으로 넓어지지 않는다.

generator의 `--check`는 Tauri build 전에 permission TOML의 정확한 집합·canonical 내용·Git
추적 여부를 확인한다. 또한 기본/dev 설정은 `main-development`, release 설정은
`main-release`만 선택해야 한다. Tauri는 capability 목록이 없거나 비어 있으면 모든 capability
파일을 자동 포함하므로 기본 설정에도 development ID를 명시한다. 현재 exact-set 정책은
dev-only command를 지원하지 않으며 필요해지면 별도 결정으로 capability 집합 의미를 바꾼다.

## 대안

- 각 호출부에 문자열 literal 사용: 국소 변경은 쉽지만 감사와 rename이 불안정하다.
- 모든 command를 하나의 wildcard capability로 허용: 유지보수는 쉽지만 피해 반경이 크다.

## 보안 영향

manifest와 두 registry의 drift, permission의 누락·변조·미추적, 환경별 capability 선택 오류는
generator의 `--check`가 build 전에 탐지한다. 등록, handler, generated permission, release
capability의 불일치는 기존 build exact-set 검사도 다시 탐지한다. renderer 입력은 여전히
Rust DTO의 strict deserialization과 Core 검증을 거친다.

## 성능 영향

상수 lookup과 DTO 변환 비용만 있으며 IPC round trip에 비해 무시할 수준이다.

## 마이그레이션 전략

기존 client API는 유지하면서 command literal만 manifest로 옮긴다. command를 추가할 때는
manifest 수정과 생성기 실행 뒤에도 Rust handler, `tauri::generate_handler!`, DTO/client,
development/release capability를 각각 검토한다. 타입 전체 codegen은 Rust와 TypeScript의
검증 경계를 흐리므로 이 결정의 범위에 포함하지 않는다. 큰 contract 파일은 도메인별
child module로 분리하고 re-export로 호환성을 유지한다.

## 롤백 전략

client facade를 유지한 채 generated registry를 수동 상수로 되돌릴 수 있다. 명시적
capability와 handler allowlist, build exact-set 검사는 롤백하지 않는다.
