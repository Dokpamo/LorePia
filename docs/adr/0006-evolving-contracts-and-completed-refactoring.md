# ADR 0006: 진화하는 계약과 완료된 리팩터링 기록

- 상태: 채택 — 2026-09-08 코드 리뷰의 전체 수정 요청에 따른 변경
- Task: REVIEW-FIX-20260908

## 결정

계층, 보안, 영속성, 결정론과 실행 코드의 계약 일치는 상시 검사한다. 완료된
리팩터링 작업의 문서·소스 크기와 당시 API 목록은 이후 기능을 영구 동결하는
근거로 사용하지 않는다.

1. Core/Storage 공개 API 및 Cargo dependency/feature의 checked-in manifest는
   현재 검토할 계약이다. 기능 변경에서 코드와 manifest를 함께 변경할 수 있다.
   누락·오래된 계약은 계속 실패한다. Stored* 재노출, 새 wildcard 예외,
   Domain/Orchestration I/O 및 workspace 계층 위반은 manifest 수정으로
   허용되지 않는다. 방향 검사는 현재 알려진 workspace crate 계층에 적용하며,
   새 crate는 소유 계층과 의존 방향을 별도로 검토해 검사 모델에 반영한다.
   새로운 의존성 자체를 이번 수정에서 추가하지는 않는다.
2. 기존 facade의 분류를 유지한 채 코드만 다른 파일로 이전할 수 있다. 옛 파일과
   추적하던 하위 source가 실제로 사라졌을 때만 그 inventory 항목을 폐기할 수
   있다. 존재하는 파일을 다른 종류로 바꾸어 cap을 회피할 수 없으며 source/test
   size cap 증가와 giant-file 예외 추가는 계속 금지한다.
3. 완료된 59개 리팩터링 작업의 context map, task manifest, completion ledger,
   baseline report와 summary는 config/refactoring/archive.json의 ancestor commit에
   고정한다. CI는 파일 무결성·완료 상태와 해당 commit의 context 크기를 검사한다.
   완료 후의 주석·기능·파일 이동은 과거 context 예산을 다시 소모하지 않는다.
4. check_ai_context_map.py --current는 현재 파일의 strict budget 측정을 제공한다.
   report_refactoring_baseline.py --print는 현재 측정값을 제공한다. archive가 있으면
   기본 report 명령도 stdout에 현재 보고서를 출력하며 과거 snapshot을 덮어쓰지
   않는다. 별도 --output/--summary-output으로 새 보고서를 저장할 수 있다.

## 검증

동작/API/의존성 변경이 manifest 없이 이루어지면 실패하고, 명시적인 manifest
업데이트는 허용한다. manifest가 일치해도 금지된 계층/I/O dependency는 실패한다.
살아 있는 facade 분류 제거, 크기 cap 증가, archived evidence 변경과 미완료
campaign archive도 실패한다. 현재 소스에 설명을 추가하거나 파일을 폐기해도
완료된 campaign의 무결성과 context 검사는 통과한다.

## 적용 범위

ADR 0001의 의존 방향과 source-size 비증가 원칙, ADR 0002~0005의 transaction,
credential, IPC와 자산 검증은 유지한다. 과거 master plan과 storage API audit에
기록된 '공개 API/의존성 inventory 감소만 허용' 규칙은 해당 리팩터링 campaign에
한정한다. 보안 또는 데이터 계약 변경을 코드 이동으로 위장해서는 안 된다.
