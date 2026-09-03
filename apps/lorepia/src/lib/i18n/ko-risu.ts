export const koRisu = {
    'chat.runtime.permissions.select_all': '호환 권한 모두 선택',
    'orchestration.package.capability.approve': '{capability} 기능 승인',
    'orchestration.package.capability.interactions': '선언형 상호작용',
    'orchestration.package.capability.portable_runtime': '격리된 Risu Lua·토글 런타임',
    'orchestration.package.capability.transforms': '안전한 텍스트 변환',
    'orchestration.risu.action.busy': '연결 중…',
    'orchestration.risu.action.generation': '프리셋 호환 설정 저장',
    'orchestration.risu.action.memory': '장기기억 설정 저장',
    'orchestration.risu.description.generation':
        '가져온 모델 힌트와 샘플링값 중 선택한 라우트가 지원하는 값만 적용합니다.',
    'orchestration.risu.description.memory':
        '가져온 요약 템플릿과 스케줄을 실제 대화 프롬프트에 연결합니다. 임베딩 모델은 차원값까지 명시해야 하므로 이 단계에서는 안전하게 비활성 상태를 유지합니다.',
    'orchestration.risu.error.generation': 'Risu 설정 적용에 실패했습니다.',
    'orchestration.risu.error.memory': '하이파 설정 적용에 실패했습니다.',
    'orchestration.risu.error.memory_metadata': '가져온 하이파 설정에 필요한 연결 정보가 없습니다.',
    'orchestration.risu.error.memory_selection':
        '요약 모델 라우트와 적용할 대화 프롬프트를 선택해 주세요.',
    'orchestration.risu.error.parameters':
        '이 라우트가 허용하는 파라미터와 맞지 않아 적용하지 못했습니다.',
    'orchestration.risu.error.prompt_binding':
        '생성 프리셋은 만들었지만 현재 프롬프트 연결을 저장하지 못했습니다.',
    'orchestration.risu.error.room_binding':
        '기억 프로필은 연결했지만 현재 방 전환을 저장하지 못했습니다.',
    'orchestration.risu.error.route_required': '모델 라우트를 선택해 주세요.',
    'orchestration.risu.error.source_api':
        '이 빌드에서는 가져온 프리셋 목록이나 편집 API를 사용할 수 없습니다.',
    'orchestration.risu.error.source_empty': '호환 설정을 적용할 프롬프트 프리셋이 없습니다.',
    'orchestration.risu.error.source_load': '프롬프트 프리셋을 불러오지 못했습니다.',
    'orchestration.risu.error.source_not_compatible':
        '선택한 프리셋에는 LorePia가 보존한 Risu 호환 설정이 없습니다.',
    'orchestration.risu.error.summary_draft': '기억 요약 작업 프로필 초안을 만들지 못했습니다.',
    'orchestration.risu.error.summary_preset': '기억 요약용 생성 프리셋을 만들지 못했습니다.',
    'orchestration.risu.error.summary_task': '기억 요약 작업 프로필을 저장하지 못했습니다.',
    'orchestration.risu.error.target_api':
        '현재 Core가 대화 프롬프트 연결 API를 제공하지 않습니다.',
    'orchestration.risu.eyebrow': 'RISU 호환',
    'orchestration.risu.generation_name': '{name} · Risu 호환',
    'orchestration.risu.label': 'Risu 호환 설정',
    'orchestration.risu.memory_name': '{name} · 장기기억',
    'orchestration.risu.model_hint': '원본 모델 힌트: {model}',
    'orchestration.risu.provider_missing':
        '먼저 설정의 공급자 화면에서 모델 라우트를 하나 만들어 주세요.',
    'orchestration.risu.route.generation': '대화 모델 라우트',
    'orchestration.risu.route.memory': '기억 요약 모델 라우트',
    'orchestration.risu.select': '선택',
    'orchestration.risu.success.generation':
        '모델 라우트와 지원되는 Risu 생성값을 현재 프롬프트에 연결했습니다.',
    'orchestration.risu.success.generation_standalone':
        '지원되는 Risu 생성값을 프리셋에 저장했습니다. 대화방에서 이 프리셋을 선택하면 적용됩니다.',
    'orchestration.risu.success.memory':
        '하이파 요약 템플릿·주기·호출 제한을 연결하고 현재 방의 장기기억을 켰습니다.',
    'orchestration.risu.success.memory_standalone':
        '하이파 요약 템플릿·주기·호출 제한을 대상 프리셋에 저장했습니다. 대화방에서 해당 프리셋을 선택하면 적용됩니다.',
    'orchestration.risu.source.label': 'Risu 호환 원본 프리셋',
    'orchestration.risu.source.loading': '가져온 프리셋을 불러오는 중입니다.',
    'orchestration.risu.summary_name': '{name} · 기억 요약',
    'orchestration.risu.target_prompt': '장기기억을 적용할 대화 프롬프트',
    'orchestration.risu.title.generation': '프리셋 모델 설정 연결',
    'orchestration.risu.title.memory': '하이파 장기기억 연결',
    'orchestration.variables.truncated': '처음 {count}개 변수만 표시합니다.',
} as const;
