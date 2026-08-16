# 릴리즈 절차

## 현재 상태

| 항목 | 상태 |
| --- | --- |
| 번들 생성 | 준비됨 — `tauri.release.conf.json`이 `bundle.active`를 켠다 |
| macOS hardened runtime + entitlements | 준비됨 — `tauri.macos.conf.json`, `Entitlements.plist` |
| CI 워크플로 | 준비됨 — `.github/workflows/release.yml` |
| 코드 서명 | **시크릿 미설정** — 아래 절차 필요 |
| 자동 업데이트 | **비활성** — 아래 결정 필요 |

## 코드 서명

릴리즈 워크플로가 아래 시크릿을 읽는다. 설정하지 않으면 서명 없는 번들이 나온다.

### macOS

| 시크릿 | 값 |
| --- | --- |
| `APPLE_CERTIFICATE` | Developer ID Application 인증서 `.p12`를 base64로 인코딩 |
| `APPLE_CERTIFICATE_PASSWORD` | 위 `.p12`의 비밀번호 |
| `APPLE_SIGNING_IDENTITY` | 예: `Developer ID Application: 이름 (TEAMID)` |
| `APPLE_ID` | 노터라이제이션용 Apple ID |
| `APPLE_PASSWORD` | 해당 Apple ID의 앱 암호 |
| `APPLE_TEAM_ID` | 10자 팀 식별자 |

`APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID`가 모두 있으면 Tauri가 번들 후 노터라이제이션까지 수행한다.

### Windows

| 시크릿 | 값 |
| --- | --- |
| `WINDOWS_CERTIFICATE` | 코드 서명 인증서 `.pfx`를 base64로 인코딩 |
| `WINDOWS_CERTIFICATE_PASSWORD` | 위 `.pfx`의 비밀번호 |

> 인증서와 비밀번호는 저장소에 커밋하지 않는다. GitHub Actions 시크릿으로만 넣는다.

## 자동 업데이트 — 결정 대기

의도적으로 꺼둔 상태다(`createUpdaterArtifacts: false`). 켜려면 두 가지를 먼저 정해야 한다.

1. **업데이트 매니페스트를 어디에 호스팅할 것인가** — GitHub Releases 정적 파일, 자체 서버 등.
2. **업데이트 서명 키페어를 어떻게 보관할 것인가** — `tauri signer generate`로 만들고,
   개인 키는 시크릿으로, 공개 키는 설정에 넣는다. **개인 키를 잃으면 기존 설치본에
   업데이트를 내보낼 수 없다.**

두 가지가 정해지면 필요한 변경은 다음과 같다.

- `apps/lorepia/src-tauri/Cargo.toml`에 `tauri-plugin-updater` 추가
- `tauri.release.conf.json`의 `createUpdaterArtifacts`를 `true`로
- `plugins.updater`에 `endpoints`와 `pubkey` 설정
- `capabilities/main-release.json`에 업데이터 권한 부여
- 릴리즈 워크플로에 `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 추가

그 전까지는 사용자가 새 버전을 직접 받아 설치하는 방식이다.

## 릴리즈 실행

1. `apps/lorepia/package.json`의 `version`을 올린다 (`tauri.conf.json`이 이 값을 참조한다).
2. 두 검사 스위트가 통과하는지 확인한다 (`CONTRIBUTING.md` 참고).
3. `vX.Y.Z` 태그를 푸시하면 릴리즈 워크플로가 플랫폼별 번들을 만들어 아티팩트로 올린다.
