# 릴리스 절차

## 현재 정책

| 항목 | 현재 상태 |
| --- | --- |
| 수동 후보 번들 | 준비됨 — `workflow_dispatch`로만 생성 |
| 후보 체크섬 | 준비됨 — `SHA256SUMS` 생성 |
| 후보 SBOM | 준비됨 — 플랫폼별 SPDX JSON 생성 |
| 후보 provenance/SBOM attestation | 준비됨 — GitHub OIDC attestation 생성 |
| 공식 `v*` 릴리스 | **차단됨** — 보호된 서명 작업이 아직 없음 |
| macOS/Windows 코드 서명 | **미구현** |
| 자동 업데이트 | **비활성** |

수동 실행 결과는 이름에 `UNSIGNED-candidate`가 붙고 14일 뒤 삭제되는 개발·검증용
아티팩트다. 공식 릴리스나 최종 사용자 배포물로 취급하지 않는다.

## 후보 번들 생성

GitHub Actions의 `Release` 워크플로를 수동 실행한다. 워크플로는 먼저 다음 공급망
검사를 통과해야 한다.

- `cargo deny check`
- 프로덕션 npm 의존성의 High/Critical advisory 차단
- SHA로 고정된 third-party Actions와 최소 권한
- checkout credential persistence 비활성화

그 다음 Linux, macOS, Windows의 unsigned 후보를 만들고 각 번들 디렉터리에 다음
증거를 함께 둔다.

- `SHA256SUMS`
- `lorepia-<runner>.spdx.json`
- GitHub artifact provenance attestation
- GitHub SBOM attestation

attestation은 빌드 출처와 digest를 증명하지만 운영체제 코드 서명을 대신하지 않는다.

## 공식 릴리스가 차단되는 이유

`v*` 태그 push는 현재 `Protected signing required` 작업에서 의도적으로 실패한다.
서명 secret이 없는 상태에서 unsigned 파일이 공식 릴리스로 게시되는 것을 막기 위한
fail-closed 정책이다. 태그를 만드는 것만으로 번들이 배포된다고 가정하면 안 된다.

공식 릴리스를 열기 전에 별도의 보호된 signing job을 구현해야 한다.

1. secret 없는 build job이 후보, 체크섬, SBOM을 생성한다.
2. GitHub Environment 승인 뒤 signing job이 정확한 후보 digest를 다시 확인한다.
3. macOS Developer ID 서명과 notarization을 수행하고 결과를 검증한다.
4. Windows Authenticode 서명을 수행하고 결과를 검증한다.
5. 서명된 파일의 새 체크섬과 provenance를 생성한다.
6. 모든 플랫폼 검증이 성공한 경우에만 GitHub Release를 게시한다.

인증서, 암호, notarization credential, updater private key는 저장소나 build job에
넣지 않는다.

## 로컬 검증

```bash
python3 scripts/check_github_workflow_security.py
python3 scripts/test_write_release_checksums.py
cargo deny check
npm audit --omit=dev --audit-level=high --prefix apps/lorepia
```

## 자동 업데이트 — 결정 대기

`createUpdaterArtifacts`는 의도적으로 꺼져 있다. 다음 두 가지가 결정되기 전에는
사용자가 새 버전을 직접 받아 설치한다.

1. 업데이트 매니페스트의 신뢰할 수 있는 호스팅 위치
2. 오프라인 복구 절차를 포함한 updater 서명 키 보관 방식

활성화할 때는 updater plugin과 최소 capability를 추가하고, 공개 키만 앱 설정에
포함하며 private key는 보호된 signing environment에만 둔다.
