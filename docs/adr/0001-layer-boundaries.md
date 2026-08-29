# ADR 0001: 계층 경계와 의존 방향

- 상태: 승인
- 날짜: 2026-08-29

## 문제

도메인 규칙, SQLite/CAS 구현, provider HTTP, Tauri 명령, Svelte UI가 서로의 구현
타입을 직접 참조하면 테스트와 보안 검토의 범위가 계속 넓어진다.

## 결정

의존 방향은 `domain -> orchestration -> core -> shell-api -> native shell/UI`로 유지한다.
`storage`와 `providers`는 Core가 사용하는 adapter이며 domain/orchestration에서 역참조하지
않는다. UI는 `shell-api` DTO와 명시적인 IPC 계약만 사용한다. 이 규칙은
`scripts/check_source_architecture.py`와 Cargo manifest 검사로 ratchet한다.

## 대안

- UI가 storage row를 직접 사용: 변경은 빠르지만 영속화 형식이 제품 API가 된다.
- 모든 기능을 Tauri crate에 배치: 조립은 단순하지만 headless 테스트와 플랫폼 분리가 깨진다.

## 보안 영향

경로, raw credential, DB connection과 native handle이 renderer DTO로 새지 않는 경계가
명확해진다. 새 command는 별도 IPC 계약과 capability 검토를 거쳐야 한다.

## 성능 영향

계층 변환 비용은 작은 구조체 복사 정도다. 네트워크·DB·파일 I/O보다 작으며, 병목은
각 adapter 내부에서 측정한다.

## 마이그레이션 전략

큰 파일은 수정되는 bounded context부터 child module로 옮기고 기존 public API는 즉시
깨지 않는다. 파일 크기 기준선은 감소만 허용한다.

## 롤백 전략

새 module의 re-export를 유지한 채 구현만 이전 module로 되돌릴 수 있다. 의존 방향
검사는 롤백하지 않는다.
