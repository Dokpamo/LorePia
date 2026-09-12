# c999c43 후속 자원 효율 재점검

2026-09-12~13, `codex/final-resource-efficiency-20260912`, PR #54.
기준은 c999c43이며 사용자가 수정에 동의한 Pro 재점검을 Astra 세 에이전트와
대조·구현했다. 보고서의 제안을 그대로 적용하지 않고 코드·재현·회귀 검사로
확인했다. 아래 수치는 합성 입력의 작업량이며 기기 응답 시간이나 RSS를
의미하지 않는다.

| 지적 | 반영한 수정과 근거 |
| --- | --- |
| RECHECK-00 Windows 테스트 실패 | 열린 verified handle의 쓰기 공유 거부를 OS 오류 32로 검증하고, handle 종료 후 변조·재개방 거부도 검증한다. 운영 코드의 공유 권한은 변경하지 않았다. |
| RECHECK-01 Markdown 노드 제한 우회 | 블록·목록 항목·인용 줄·텍스트·인라인 요소를 합친 4096 render-unit 제한. 초과한 블록부터 원문을 단일 literal로 표시한다. 네 종류의 60KB 입력과 실제 Svelte 요소/텍스트 노드를 검사했다. |
| RECHECK-02 스트리밍 전체 재파싱 | 각 메시지에서 확정된 빈 줄 이전 블록만 재사용하고 미완성 문법은 다시 해석한다. 60KB/500회 입력에서 생성 블록 ≤2548, UTF-8 길이 계산 입력 60KB. 모든 접두 입력·Unicode·수정·축약·상한 전환의 전체 파서 동등성을 검사했다. |
| RECHECK-03 코드 513번째 줄 손실 | 실제 닫는 fence만 소비한다. 열린/닫힌 511~514줄 코드의 모든 내용이 유지된다. |
| RECHECK-04 30+128 중복 로딩 | 최근 30개를 먼저 표시하고 같은 snapshot의 이전 98개만 보충한다. 정상 경로의 본문 전송은 158→128개. 본문·상태·체크포인트·표시 내용이 바뀌거나 anchor가 사라지면 최신 128개로 다시 읽는다. |
| RECHECK-05 과도한 계보 재검증 | 로컬 messages 변경만 계보 epoch에 반영한다. 10만 메시지에서 조회/체크포인트/무관한 쓰기 20회 교대 시 전체 검증 1회. anchor·membership은 ID를 복제하지 않는 정렬 위치 인덱스로 검색한다. 외부 변경·rollback·DDL·지원하지 않는 테이블은 검증을 생략하지 않는다. |
| RECHECK-06 정적 카드 반복 빌드 | 매크로/변환 규칙이 없는 정적 카드의 무관한 입력 변화는 빌드 전에 제외한다. 카드 3개의 normalization·display·asset resolve·sanitize·srcdoc 추가 작업이 모두 0회였다. 권한/profile/client/본문 변경과 동적 문법은 재검증한다. |

30+98 경로를 교차 검토하면서 추가 비용도 수정했다. 선택 페이지에 assistant가
있으면 그보다 최신 부분만 확인하며, head 페이지는 추가 SQL이 없다. 그 외에는
ID 128개씩 탐색해 첫 assistant에서 중단한다. 10만 계보의 30개 prefix 경로가
전체 계보를 방문하지 않는 것을 SQLite 작업량 상한으로 검증했다.

증분 파서에는 별도 메모리 회귀도 발견해 수정했다. 확정 블록의 작은 문자열이
과거 누적 입력 버퍼를 붙잡는 현상을 실제 GC 후 힙으로 재현했다. 새로 확정된
문자열만 UTF-16 값을 보존해 분리한 뒤, 1,800문단 합성 입력의 소유 힙 중앙값은
169.19 MB에서 0.94 MB로 줄었다. 이는 이번 증분 구현의 수정 전/후 비교이며,
기존 전체 파싱보다 메모리가 180배 작다는 뜻은 아니다. 같은 전체 파싱 대조군은
0.73 MB였다. V8/Node 24에서 측정했으며 WebKit의 힙은 측정하지 않았다.

Storage는 본문과 표시 sidecar·진단을 같은 읽기 transaction에서 얻고 잠금을
해제한 뒤 해시를 검증한다. renderer의 snapshot token은 Storage 인스턴스와
전체 DB 변경 상태에 묶인 opaque hash다. 이 토큰과 계보 전용 epoch의 역할을
분리해 체크포인트를 놓치지 않는다. 새 migration, 명령, capability나 의존성
버전 변경은 없다. 기존 rusqlite의 hooks feature와 두 페이지 공개 타입의
변경은 명시적 계약 inventory에 반영했다.

## 검증 기록

macOS ARM64, Rust 1.96.0, Node 24.18.1에서 전체 Rust 1,981개 테스트를
통과했다(기존 ignored 14개, 실패 0개). 프런트 전체 162개 파일/880개 테스트,
Lua/regex sandbox·GC 메모리 회귀 검사, production build와
format·lint·TypeScript·Svelte 검사도 통과했다. 전체 workspace Clippy의
경고 금지 검사와 rustfmt, Python 102개 테스트 및 IPC·architecture·context·
i18n·archive·workflow 검사를 통과했다. 수정한 계보/hook/페이지 표적은
각각 5/2/10개, Shell의 페이지/snapshot 표적 9개를 통과했다. 플랫폼 CI의
최종 결과는 PR #54 본문의 validation과 checks에 기록한다. Windows 공유
동작은 실제 Windows CI 결과로 판단하며 checks가 원본 기록이다.

전체 Rust 실행 중 direct-reqwest 대조군의 정확히 1개 TCP 가정도 교정했다.
Hyper의 idle 반영과 다음 연결 시작은 경쟁할 수 있어, 완료된 body만으로
정확한 TCP 개수를 보장할 수 없다. 통합 검사는 20회 요청에서 실제 연결
재사용과 요청 수·credential 분리를 확인하며, 정확한 transport 생성/정책 key는
기존 pool 단위 검사가 확인한다. 제품 전송 정책이나 코드는 변경하지 않았다.

RustSec CI에는 검사 도구 설치 문제도 있었다. 고정 action의 기본 설치가
버전/lockfile을 고정하지 않아 문서 파일이 빠진 jiff 0.2.36을 선택했다.
같은 cargo-audit 0.22.2를 배포 lockfile로 미리 설치하도록 고쳤다. 앱의
Cargo.lock이나 감사 제외 목록은 변경하지 않았다. 기존 action이 PATH의
도구를 재사용하는 동작은 [고정 action 코드](https://github.com/rustsec/audit-check/blob/69366f33c96575abad1ee0dba8212993eecbe998/src/main.ts)와
실행 번들에서 확인했다. `--locked`의 의미는 [Cargo 설치 문서](https://doc.rust-lang.org/cargo/commands/cargo-install.html)를 따른다.

## 보장 범위

- 계보의 첫 검증은 전체 ID를 읽고 정렬한다. 8 MiB는 유지되는 캐시의 상한으로,
  cold 임시 메모리나 모든 DB 데이터의 상한이 아니다. head 변경/외부 쓰기/
  메시지 변경과 캐시 교체 후에는 다시 검증할 수 있다.
- Markdown은 이전 입력 비교와 미완성 긴 suffix의 재파싱이 남는다. 모든 입력을
  새로 도착한 글자 수만큼만 처리한다고 보장하지 않는다. 4096은 의미 있는
  렌더 요소/텍스트의 제한이며 프레임워크 comment나 전체 브라우저 heap 한도는 아니다.
- 변환 규칙이나 매크로가 있는 Portable 문서는 보수적으로 동적 경로를 유지한다.
- 기록이 변하면 추가 조회 비용을 내고 최신 문맥을 다시 얻는다. snapshot proof가
  없는 구형 client는 기존 최신 128개 fallback을 유지한다.
- 원본 이미지 손실 압축, 내구성 완화, credential/DNS/파일 검증 삭제는 포함하지 않는다.

상세 계약: [history](resource-recheck-history.md),
[Markdown](resource-recheck-markdown.md), [Storage](resource-recheck-storage.md),
[Portable](resource-recheck-portable.md), [작업 범위](resource-recheck-task.md).
