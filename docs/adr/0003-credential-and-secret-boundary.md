# ADR 0003: Credential과 secret 경계

- 상태: 승인
- 날짜: 2026-08-29

## 문제

provider credential이 renderer, 로그, SQLite row 또는 일반 설정 export에 들어가면 XSS,
진단 파일, 백업을 통해 장기 secret이 노출될 수 있다.

## 결정

renderer와 Core는 credential 값 대신 논리적 reference와 binding hash만 다룬다. 실제 값의
저장·조회·삭제는 native platform credential adapter가 수행하고, 작업별 짧은 lease만
provider dispatch에 전달한다. 로그와 durable operation에는 metadata/status만 기록한다.
vault가 잠기거나 binding이 달라지면 자동 대체하지 않고 fail-closed한다.

대화와 캐릭터 DB 자체는 현재 암호화 vault가 아니다. 운영체제 파일 보호와 private
permission은 적용하지만 선택적 콘텐츠 암호화는 별도 제품 설계 과제로 남긴다.

## 대안

- API key를 SQLite에 암호화 없이 저장: 구현은 단순하지만 데이터 루트 유출에 취약하다.
- 하나의 앱 전역 secret slot 사용: 교체는 쉽지만 connection/origin 혼동 위험이 크다.

## 보안 영향

raw secret의 직렬화 면적과 renderer 도달 가능성이 줄어든다. 반면 잠금 해제된 동일 사용자
세션과 평문 콘텐츠 DB에 대한 위협은 별도 프라이버시 고지와 vault 설계가 필요하다.

## 성능 영향

플랫폼 vault 접근과 binding 검증이 dispatch마다 발생할 수 있다. lease는 작업 범위와
수명을 제한하며 장기 plaintext cache를 만들지 않는다.

## 마이그레이션 전략

legacy credential은 durable operation과 recovery evidence를 거쳐 native binding으로
이동한다. 실패 또는 결과 불명은 재사용하지 않고 quarantine/recovery 대상으로 둔다.

## 롤백 전략

새 binding이 검증되기 전에 legacy slot을 삭제하지 않는다. 되돌릴 때도 raw secret을
renderer나 DB로 복사하지 않고 platform adapter의 호환 경로만 사용한다.
