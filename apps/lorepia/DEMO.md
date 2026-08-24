# LorePia 디자인 데모

실제 계정, 자격 증명, 로컬 데이터베이스를 건드리지 않고 각 화면의 디자인과 상호작용을 확인하는 인메모리 데모입니다.

## Tauri 앱으로 실행

```sh
npm run tauri:demo
```

이 명령은 `preview.html`과 데모 전용 클라이언트를 실제 Tauri 창에 연결합니다. 일반 `npm run tauri -- dev`는 계속 실제 로컬 데이터 모드로 실행됩니다.

## 포함된 화면 데이터

- 캐릭터 4명과 캐릭터별 인사말
- 대화, 분기, 날짜가 포함된 채팅 메시지
- 페르소나 목록과 추가·수정·삭제 흐름
- 공급자 연결, 모델 경로, 생성 프리셋과 현재 설정값
- 프롬프트, 작업·메모리 프로필, 지식책, 변환 규칙, 상호작용 규칙, 콘텐츠 모듈

데모 세션의 변경은 메모리에만 남으며 앱을 다시 실행하면 `src/preview/demo-data.ts`의 초기 상태로 돌아갑니다.

## 관련 파일

- `src/preview/demo-data.ts`: 데모 데이터 원본
- `src/preview/mock-client.ts`: 화면 동작을 연결하는 상태형 인메모리 클라이언트
- `src/preview/main.ts`: 데모 진입점과 최초 캐릭터·대화 선택
- `src-tauri/tauri.demo.conf.json`: Tauri 데모 실행 설정
