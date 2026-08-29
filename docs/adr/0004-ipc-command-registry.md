# ADR 0004: IPC command registry

- 상태: 승인
- 날짜: 2026-08-29

## 문제

문자열 command가 UI 곳곳에 흩어지고 Rust handler와 Tauri capability가 따로 변경되면,
오타뿐 아니라 release에서 의도하지 않은 권한 노출이 생긴다.

## 결정

frontend command/event 이름은 `apps/lorepia/src/lib/ipc/commands.ts`의 상수 registry를
단일 기준으로 사용한다. payload/result는 `contracts.ts` 및 bounded contract module에
둔다. Rust는 명시적인 invoke handler 목록을 유지하고 build script가 permission을
생성한다. development와 release capability는 별도 allowlist이며 새 command를 자동으로
release에 추가하지 않는다.

## 대안

- 각 호출부에 문자열 literal 사용: 국소 변경은 쉽지만 감사와 rename이 불안정하다.
- 모든 command를 하나의 wildcard capability로 허용: 유지보수는 쉽지만 피해 반경이 크다.

## 보안 영향

등록, handler, generated permission, release capability 네 지점의 불일치를 CI가 탐지할
수 있다. renderer 입력은 여전히 Rust DTO의 strict deserialization과 Core 검증을 거친다.

## 성능 영향

상수 lookup과 DTO 변환 비용만 있으며 IPC round trip에 비해 무시할 수준이다.

## 마이그레이션 전략

기존 client API는 유지하면서 command literal만 registry로 옮긴다. 큰 contract 파일은
도메인별 child module로 분리하고 re-export로 호환성을 유지한다.

## 롤백 전략

client facade를 유지한 채 registry module을 합칠 수 있다. 명시적 capability와 handler
allowlist는 롤백하지 않는다.
