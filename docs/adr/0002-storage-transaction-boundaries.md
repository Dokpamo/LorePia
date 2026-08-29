# ADR 0002: Storage 트랜잭션 경계

- 상태: 승인
- 날짜: 2026-08-29

## 문제

SQLite 기록과 CAS 파일 승격이 서로 다른 시점에 실패하면 DB만 남거나 파일만 남는
부분 완료 상태가 생긴다. Core가 SQL 순서를 직접 조립하면 복구 불변조건도 분산된다.

## 결정

SQL transaction, CAS publish/fsync, import journal, cutover와 시작 시 복구는 `storage`가
소유한다. Core는 의미 있는 use-case를 호출하고 결과를 조정하지만 Connection이나 경로를
받지 않는다. 다단계 작업은 durable intent/phase를 먼저 기록하고, 재시작 복구가
idempotent하게 완료하거나 격리할 수 있어야 한다.

근거 테스트는 import cutpoint, schema cutover, generation closure, credential operation
recovery와 `testdata/tauri-upgrade` fixture다.

## 대안

- Core에서 여러 storage 메서드를 임의 순서로 호출: 경계가 보이지만 crash consistency가 약하다.
- 모든 파일을 SQLite blob으로 저장: 원자성은 단순해지지만 대용량 미디어 전달 비용이 커진다.

## 보안 영향

승인 전 staging 경로와 승인 후 CAS 경로가 섞이지 않으며, 불완전한 상태는 정상 데이터로
노출되지 않는다. 데이터 무결성 불일치는 fail-closed 오류가 된다.

## 성능 영향

fsync와 hash 비용이 추가된다. durable commit 구간에서만 지불하고 read path는 별도
bounded cache/streaming 정책으로 최적화한다.

## 마이그레이션 전략

기존 공개 row/commit 타입은 먼저 분류하고 Core 호출자를 의미 기반 repository API로
옮긴 뒤 visibility를 줄인다. 현황은 `docs/architecture/storage-public-api-audit.md`에 둔다.

## 롤백 전략

각 migration과 cutover는 이전 generation을 보존한 상태에서만 활성화한다. 데이터가
이미 기록된 뒤에는 코드만 되돌리지 않고 호환성 검증 및 명시적인 recovery 절차를 쓴다.
