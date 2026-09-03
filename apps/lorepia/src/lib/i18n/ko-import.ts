export const koImport = {
    'import.assets': '에셋',
    'import.assets.count': '{count}개',
    'import.blocked': '가져올 수 없음',
    'import.cancel': '취소',
    'import.close': '닫기',
    'import.commit': '서재에 추가',
    'import.commit.content': '콘텐츠로 가져오기',
    'import.commit.safe': '안전 모드로 가져오기',
    'import.description.empty': '설명이 없습니다.',
    'import.dialog.close': '가져오기 검토 닫기',
    'import.dynamic.capabilities': '카드가 선언한 권한: {capabilities}',
    'import.dynamic.capabilities.legacy':
        '이전 형식 카드로, 실행 전에 필요한 권한을 사용자가 직접 선택해야 합니다.',
    'import.dynamic.capabilities.none': '호스트 권한 없음',
    'import.dynamic.elevated': '고급 권한을 요청하는 Lua 스크립트 {count}개',
    'import.dynamic.lua': 'Lua 런타임 스크립트 {count}개',
    'import.dynamic.markup': '사용자 정의 HTML/CSS 표시',
    'import.dynamic.model': '선택한 모델을 추가로 호출할 수 있음',
    'import.dynamic.network': '직접 네트워크·파일·Tauri 접근은 허용되지 않습니다.',
    'import.dynamic.regex': '정규식 변환 {count}개',
    'import.dynamic.safe_mode':
        '가져온 뒤에는 일반 콘텐츠만 열리며, 동적 기능은 대화 설정에서 권한별로 승인해야 합니다.',
    'import.dynamic.title': '동적 기능과 요청 권한',
    'import.estimated_size': '예상 저장 크기',
    'import.error.storage_unavailable':
        '저장 공간이 부족하거나 사용할 수 없습니다. 대용량 파일은 가져오는 동안 원본 크기의 약 3배 이상 여유 공간이 필요할 수 있습니다.',
    'import.error.resource_limit':
        '표준 가져오기 자원 한도를 넘었습니다. 파일을 실행하지 않았으며 아직 서재에 추가하지 않았습니다.',
    'import.inspecting': '로컬 파일을 안전하게 검사하는 중입니다.',
    'import.kind': '형식',
    'import.kind.png': 'PNG 카드',
    'import.kind.imported_memory': '메모리 프리셋',
    'import.kind.imported_module': '콘텐츠 모듈',
    'import.kind.imported_preset': '프롬프트 프리셋',
    'import.notice.added': '{name}을(를) 서재에 추가했습니다.',
    'import.notice.content_added':
        '{name}을(를) 콘텐츠로 가져왔습니다. 문서 {documents}개 · 에셋 {assets}개',
    'import.notice.review': '{name} 가져오기를 검토해 주세요.',
    'import.regex.checking': '정규식 규칙을 격리된 Worker에서 검사하는 중입니다.',
    'import.regex.disabled': '유효하지 않거나 제한시간을 넘긴 규칙 {count}개는 비활성화됩니다.',
    'import.regex.unavailable':
        '검사를 완료하지 못한 규칙 {count}개는 실행 시 안전하게 건너뜁니다.',
    'import.regex.valid': '실행 가능한 정규식 규칙의 컴파일 검사를 통과했습니다.',
    'import.resource.approved_limits':
        '사용자 승인으로 원본·단일 항목은 최대 16 GiB, 총 압축 해제량은 최대 32 GiB까지 검사합니다.',
    'import.resource.inspecting':
        '대용량 모드로 복사하고 검사하는 중입니다. 파일 크기에 따라 오래 걸릴 수 있습니다.',
    'import.resource.retry': '대용량 모드로 다시 선택',
    'import.resource.retry_description':
        '필요한 파일이 맞다면 대용량 모드로 다시 선택할 수 있습니다. 저장공간이 원본 크기의 약 3배 이상 필요할 수 있습니다.',
    'import.resource.security_boundary':
        '경로 조작·심볼릭 링크·손상된 구조·파일 서명 검사는 완화하지 않습니다.',
    'import.resource.storage_warning':
        '검사 중 앱 전용 임시 복사본이 생기며, 공간이 부족하면 아무것도 가져오지 않고 중단합니다.',
    'import.resource.title': '대용량 자원 승인',
    'import.compatibility.destination':
        '모듈·프리셋·지식·에셋은 콘텐츠 저장소에 보관되며 캐릭터 서재 항목으로 만들지 않습니다.',
    'import.compatibility.safety':
        'Lua는 격리된 런타임에서 권한을 승인한 뒤에만 실행합니다. 사용자 HTML/CSS와 직접 네트워크·파일 접근은 실행하지 않으며, 모델·API 설정은 확인 후 연결합니다.',
    'import.compatibility.title': '호환 콘텐츠 가져오기',
    'import.compatibility.update':
        '같은 콘텐츠를 다시 가져오면 현재 항목을 최신 호환 변환본으로 갱신합니다. 기존 내용은 이전 리비전으로 보존됩니다.',
    'import.source_size': '원본 크기',
    'import.title': '가져오기 검토',
    'import.unsupported_fields': '아직 지원하지 않는 선택 필드',
    'import.warnings': '확인할 내용',
} as const;
