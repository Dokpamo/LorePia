# ADR 0005: Asset delivery verification

- 상태: 승인
- 날짜: 2026-08-29

## 문제

renderer에 파일 경로나 임의 URL을 주면 로컬 파일 권한이 넓어진다. 반대로 Range 요청마다
64MiB CAS 객체 전체를 다시 hash한 뒤 일부만 반환하면 오디오·영상 seek 비용이 요청 수에
비례해 커진다. 경로를 검사한 뒤 별도 open하는 방식에는 교체 경쟁도 남는다.

## 결정

renderer는 `lorepia-asset://sha256/<canonical-lowercase-digest>`만 받는다. native protocol은
DB의 승인 descriptor와 고정 media allowlist를 확인하고 다음 규칙으로 전달한다.

1. cache miss에서 digest로 계산한 exact CAS 경로를 no-follow 방식으로 연다. Unix 계열은
   directory handle chain과 `openat`; Windows는 reparse-point 자체를 열어 거부한다.
2. 같은 file handle에서 regular-file/size/single-link 조건, 전체 SHA-256과 media signature를
   검증한다.
3. hash 시작 전에 file identity를 봉인하고 hash·signature 뒤와 cache 삽입 직전에 다시
   비교한다.
4. 검증된 handle은 최대 16개, 30초의 process-local LRU lease에만 보관한다. cache miss
   전체 검증은 분당 32회로 제한해 순환 eviction hash 증폭을 막는다.
5. cache hit마다 file identity를 range read 전후에 비교하고, 달라지면 응답하지 않는다.
6. 요청한 range만 같은 handle에서 seek/read한다. protocol range는 최대 1MiB이며 전체
   renderer asset은 64MiB, 이미지는 16MiB로 제한한다.
7. 동시에 유지할 응답 수와 body memory는 native admission gate로 제한한다.

경로가 lease 중 교체되어도 열린 verified handle의 기존 bytes만 제공하며, lease 만료 뒤에는
새 경로를 다시 전체 검증한다. 같은 inode의 변경과 hard-link alias는 fail-closed한다.

## 대안

- 요청마다 전체 파일 hash: 단순하고 보수적이지만 연속 seek의 총 읽기량이 과도하다.
- 검증 후 파일을 닫고 range용으로 재open: cache는 쉽지만 검증 대상과 전달 대상이 달라질 수 있다.
- 파일 bytes 전체를 메모리에 cache: seek는 빠르지만 메모리 DoS 면적이 커진다.

## 보안 영향

absolute path, logical package path와 raw bytes는 invoke DTO를 통과하지 않는다. 짧은 lease로
재검증 간격이 생기지만, 열린 handle과 전후 identity 검사가 교체·변조된 bytes의 전달을
막는다. cache lock poisoning과 identity 변화는 정상 데이터로 복구하지 않는다.

## 성능 영향

한 lease 안의 반복 Range 요청은 전체 hash를 한 번만 수행하고 이후에는 요청 범위만 읽는다.
동일 cache의 file seek는 직렬화되지만 범위가 1MiB로 제한되어 있고 open handle 수가
고정된다. 실제 WebView 연속 seek 처리량은 플랫폼 benchmark로 계속 측정한다.

## 마이그레이션 전략

기존 digest URL과 shell DTO는 유지한다. protocol backend만 full-body 후 slice에서
descriptor-first, exact-range read로 바꾸므로 frontend 계약 변경은 없다.

근거 테스트는 `verified_asset_cache`의 반복 range/expiry/mutation, storage의 hash 횟수와
tamper 검출, Tauri protocol의 exact range/HEAD/admission/URI parser 테스트다.

## 롤백 전략

문제가 생기면 lease TTL을 0으로 바꿔 요청마다 재검증하도록 되돌릴 수 있다. opaque digest,
no-follow open, 동일 handle 검증·read, 크기 제한과 admission gate는 유지한다.
