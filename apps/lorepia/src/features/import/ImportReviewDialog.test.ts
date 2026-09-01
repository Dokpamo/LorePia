import { cleanup, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
import { t } from '../../lib/i18n';
import { resetPortableRegexRuleFailuresForTests } from '../chat/portable-regex';
import ImportReviewDialog from './ImportReviewDialog.svelte';

afterEach(() => {
    cleanup();
    resetPortableRegexRuleFailuresForTests();
});

describe('ImportReviewDialog dynamic content review', () => {
    it('compiles regex rules before commit and reports only the failed rule as disabled', async () => {
        const state = {
            import_flow: {
                phase: 'ready',
                error: null,
                inspection: {
                    inspection_id: 'inspection-1',
                    kind: 'charx_package',
                    display_name: 'Dynamic card',
                    description: 'Review fixture',
                    source_sha256: 'ab'.repeat(32),
                    source_size: 1_024,
                    estimated_stored_size: 2_048,
                    asset_count: 0,
                    dynamic_content: {
                        runtime_script_count: 1,
                        elevated_runtime_script_count: 1,
                        required_runtime_capabilities: ['runtime:callbacks', 'model:primary'],
                        runtime_capabilities_declared: true,
                        regex_rule_count: 2,
                        enabled_regex_rule_count: 2,
                        model_calls_possible: true,
                        custom_markup_present: true,
                        regex_rules: [
                            {
                                id: 'valid',
                                name: 'Valid',
                                phase: 'display',
                                runtime_index: 0,
                                pattern: '(?<=a)b',
                                flags: 'u',
                            },
                            {
                                id: 'invalid',
                                name: 'Invalid',
                                phase: 'display',
                                runtime_index: 1,
                                pattern: '(',
                                flags: '',
                            },
                        ],
                    },
                    representative_image: null,
                    warnings: [],
                    blocked_reasons: [],
                    unsupported_optional_fields: [],
                    allowed: true,
                },
            },
        } as unknown as LorepiaAppState;
        const controller = {
            commitImport: vi.fn(),
            discardImport: vi.fn(),
        } as unknown as LorepiaAppController;

        render(ImportReviewDialog, { state, controller });

        expect(screen.getByText('Lua 런타임 스크립트 1개')).toBeInTheDocument();
        expect(screen.getByText('고급 권한을 요청하는 Lua 스크립트 1개')).toBeInTheDocument();
        expect(screen.getByText('선택한 모델을 추가로 호출할 수 있음')).toBeInTheDocument();
        await waitFor(() => {
            expect(
                screen.getByText('유효하지 않거나 제한시간을 넘긴 규칙 1개는 비활성화됩니다.'),
            ).toBeInTheDocument();
            expect(screen.getByRole('button', { name: '안전 모드로 가져오기' })).toBeEnabled();
        });
        expect(
            screen.getByText(
                '가져온 뒤에는 일반 콘텐츠만 열리며, 동적 기능은 대화 설정에서 권한별로 승인해야 합니다.',
            ),
        ).toBeInTheDocument();
    });

    it('labels a Risu module destination and quarantine boundary before commit', () => {
        const longDescription = 'Risu module description '.repeat(80);
        const state = {
            import_flow: {
                phase: 'ready',
                error: null,
                inspection: {
                    inspection_id: 'inspection-risu',
                    kind: 'risu_module',
                    display_name: 'Additional asset module',
                    description: longDescription,
                    source_sha256: 'cd'.repeat(32),
                    source_size: 195_314_199,
                    estimated_stored_size: 197_427_600,
                    asset_count: 3_172,
                    dynamic_content: {
                        runtime_script_count: 0,
                        elevated_runtime_script_count: 0,
                        required_runtime_capabilities: [],
                        runtime_capabilities_declared: false,
                        regex_rule_count: 0,
                        enabled_regex_rule_count: 0,
                        model_calls_possible: false,
                        custom_markup_present: false,
                        regex_rules: [],
                    },
                    representative_image: null,
                    warnings: [],
                    blocked_reasons: [],
                    unsupported_optional_fields: [],
                    allowed: true,
                },
            },
        } as unknown as LorepiaAppState;
        const controller = {
            commitImport: vi.fn(),
            discardImport: vi.fn(),
        } as unknown as LorepiaAppController;

        const { container } = render(ImportReviewDialog, { state, controller });

        expect(screen.getByText(t('import.kind.risu_module'))).toBeInTheDocument();
        expect(screen.getByText(t('import.risu.title'))).toBeInTheDocument();
        expect(screen.getByText(t('import.risu.safety'))).toBeInTheDocument();
        expect(screen.getByRole('button', { name: t('import.commit.content') })).toBeEnabled();
        const description = container.querySelector('.review-description');
        expect(description?.textContent.endsWith('…')).toBe(true);
        expect(description?.textContent.length).toBeLessThan(longDescription.length);
        expect(container.querySelector('.modal-body')).toBeInTheDocument();
        expect(container.querySelector('.modal-card > .modal-actions')).toBeInTheDocument();
    });
});
