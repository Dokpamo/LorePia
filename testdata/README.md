# LorePia test fixtures

이 디렉터리의 일반 import/security fixture는 LorePia 테스트를 위해 만든 합성 데이터다.
실사용자 데이터, 실제 credential, 제3자 캐릭터 카드나 제3자 프로젝트 소스 파일을
복사하지 않는다. 포맷 호환성은 공개된 포맷 구조만 참고하고 내용은 clean-room으로 만든다.

현재 legacy fixture 생성 스크립트는 보존돼 있지 않으므로 아래 커밋 바이트와 SHA-256을
authoritative fixture로 취급한다. fixture를 다시 만들면 생성 스크립트를 함께 추가하고
이 표의 hash를 갱신해야 한다.

| 파일 | 목적 | SHA-256 |
| --- | --- | --- |
| `archives/absolute-path.zip` | 절대경로 entry 거부 | `f3f6081677cced532be66a0d265c1218c8124ff2ab2786511aea6ad0ebe319cb` |
| `archives/case-collision.zip` | 대소문자 경로 충돌 거부 | `48a59a3b1464eda1828aae1939decd99dc3e50a220bc0df18a86dffef1ff4fe5` |
| `archives/high-ratio.zip` | 압축비 제한 | `bfaa5a9aae030a4bf209a8d0b775c9b74cf2fd6b7475126dac7504a7d974f7aa` |
| `archives/mime-mismatch.zip` | 확장자/서명 불일치 거부 | `98d46f57aed87485c73a411d1b38c9a306f9ba08628a238b212d236092e60a12` |
| `archives/traversal.zip` | 상위 경로 탈출 거부 | `c9dd532fffeb9ae0686fad536c1a013bb6b760324e130c03f62f4585e352f961` |
| `archives/unicode-collision.zip` | 정규화 경로 충돌 거부 | `bcfa8b92d5a58919833fff457cfe3ae6d3bd246447e1f60d9e84bb002cf3cf5c` |
| `cards/minimal-v3.json` | 최소 chara_card_v3 | `c3044fb2fde68e7d0a7673e6623890d8c2eacaa96422a9b6c22f77b8afe9dea2` |
| `packages/minimal.charx` | asset 없는 최소 CHARX | `055203ed208eaff2a227346944f081ae324cba26e0b8d8c619321bb6879eb561` |
| `packages/with-avatar.charx` | 합성 PNG가 있는 CHARX | `2c528a64fbf36a011e29c1a692cd13568b83f76e764ea03487393c28a2e666de` |
| `tauri-upgrade/recovery-compatibility-v1-vectors.json` | 합성 cutover/credential recovery vector | `6ac79521f61a1e051ea613caf1b3605a69fcb89644af81a4f742e201136b3194` |

`tauri-upgrade/native-schema-11`은 예외적으로 외부 frozen reference의 정확한 tag/commit을
실행해 생성한 호환성 증거다. 외부 제품 소스를 workspace에 복사하지 않았고 seed는 위의
합성 `with-avatar.charx`다. 출처 commit, adapter/lockfile/artifact hash, 재현 범위와
`NOT_RUN` 플랫폼 gate는 `native-schema-11/provenance.json`을 기준으로 한다.

저장소 전체의 배포 라이선스는 아직 소유자가 결정하지 않았다. 따라서 이 문서는 새로운
MIT/Apache 또는 fixture 재배포 라이선스를 임의로 부여하지 않는다. 라이선스가 확정되면
root license 파일과 함께 fixture 적용 범위를 명시해야 한다.
