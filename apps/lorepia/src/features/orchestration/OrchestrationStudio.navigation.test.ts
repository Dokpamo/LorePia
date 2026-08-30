import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { LorepiaClient } from '../../lib/ipc/contracts';
import OrchestrationStudio from './OrchestrationStudio.svelte';
import { ContentPackageController } from './content-package-controller';
import { contentPackageState } from './tests/content-package-fixtures';
import { appState, controller, orchestrationState } from './tests/fixtures';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe('OrchestrationStudio', () => {
    it('keeps the orchestration studio available when provider loading fails', () => {
        const failedProviderState = appState();
        failedProviderState.providers.phase = 'error';
        failedProviderState.providers.error = 'synthetic provider failure';

        render(OrchestrationStudio, {
            section: 'prompt',
            appState: failedProviderState,
            orchestrationState: orchestrationState(),
            controller: controller(),
        });

        expect(screen.getByRole('region', { name: '프롬프트' })).toBeInTheDocument();
        expect(screen.queryByRole('heading', { name: '프롬프트 제작실' })).not.toBeInTheDocument();
    });

    it('groups create destinations and pushes each section into one selected subtool page', async () => {
        const onOpenSection = vi.fn();
        const rendered = render(OrchestrationStudio, {
            section: null,
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            onOpenSection,
        });

        expect(screen.getByRole('heading', { name: '창작 스튜디오' })).toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '새로고침' })).not.toBeInTheDocument();

        const feature = screen.getByRole('button', { name: /프롬프트 설계/ });
        expect(feature).toHaveClass('setting-row', 'studio-destination-row');
        const destinationList = rendered.container.querySelector<HTMLElement>(
            '.setting-list.studio-destination-list',
        );
        if (destinationList === null) throw new Error('studio destination list is missing');
        expect(destinationList).toContainElement(feature);
        expect(within(destinationList).getAllByRole('button')).toHaveLength(4);
        expect(destinationList.querySelectorAll('.setting-icon')).toHaveLength(4);
        expect(destinationList.querySelector('.setting-chevron')).not.toBeInTheDocument();

        await fireEvent.click(feature);
        expect(onOpenSection).toHaveBeenCalledWith('prompt');

        const sectionCases = [
            {
                section: 'prompt' as const,
                count: 6,
                destination: /프롬프트 블록/,
                outerTitle: '프롬프트',
                innerTitle: '프롬프트 블록',
                assertSelected: () => {
                    expect(
                        screen.getByRole('searchbox', { name: '블록 검색' }),
                    ).toBeInTheDocument();
                    expect(screen.queryByLabelText('사용자 표시 이름')).not.toBeInTheDocument();
                },
            },
            {
                section: 'memory' as const,
                count: 4,
                destination: /세계관 지식 시뮬레이터/,
                outerTitle: '기억과 지식',
                innerTitle: '세계관 지식 시뮬레이터',
                assertSelected: () => {
                    expect(screen.getByLabelText('검사할 문장')).toBeInTheDocument();
                    expect(screen.queryByLabelText('규칙 ID')).not.toBeInTheDocument();
                },
            },
            {
                section: 'content' as const,
                count: 2,
                destination: /LorePia 패키지/,
                outerTitle: '콘텐츠 모듈',
                innerTitle: 'LorePia 패키지 선택 가져오기',
                assertSelected: () => {
                    expect(
                        screen.getByRole('toolbar', { name: 'LorePia 패키지 작업' }),
                    ).toBeInTheDocument();
                    expect(
                        screen.queryByRole('heading', { name: '콘텐츠 모듈 활성화·롤백' }),
                    ).not.toBeInTheDocument();
                },
            },
            {
                section: 'diagnostics' as const,
                count: 3,
                destination: /메시지 표시 변환/,
                outerTitle: '진단',
                innerTitle: '메시지 표시 변환 진단',
                assertSelected: () => {
                    expect(
                        screen.getByText('현재 분기에 저장된 표시 변환 진단이 없습니다.'),
                    ).toBeInTheDocument();
                    expect(
                        screen.queryByRole('heading', {
                            name: '현재 방의 지식·기억 선택 근거',
                        }),
                    ).not.toBeInTheDocument();
                },
            },
        ];

        for (const sectionCase of sectionCases) {
            cleanup();
            const sectionView = render(OrchestrationStudio, {
                section: sectionCase.section,
                detailPage: null,
                appState: appState(),
                orchestrationState: orchestrationState(),
                controller: controller(),
                contentPackageState: contentPackageState(),
                contentPackageController: new ContentPackageController({} as LorepiaClient),
            });
            const subtools = screen.getByRole('list', { name: '세부 도구' });
            expect(subtools).toHaveClass('setting-list', 'studio-detail-list');
            const rows = within(subtools).getAllByRole('button');
            expect(rows).toHaveLength(sectionCase.count);
            expect(rows.every((row) => row.classList.contains('studio-detail-row'))).toBe(true);
            expect(subtools.querySelector('.setting-chevron')).not.toBeInTheDocument();

            await fireEvent.click(
                within(subtools).getByRole('button', { name: sectionCase.destination }),
            );

            expect(screen.queryByRole('list', { name: '세부 도구' })).not.toBeInTheDocument();
            expect(
                sectionView.container.querySelectorAll('.studio-home.detail-index'),
            ).toHaveLength(0);
            expect(sectionView.container.querySelectorAll('.studio-panel')).toHaveLength(1);
            expect(
                screen.queryByRole('heading', { name: sectionCase.outerTitle }),
            ).not.toBeInTheDocument();
            const detailRegion = screen.getByRole('region', { name: sectionCase.innerTitle });
            expect(
                within(detailRegion).queryByRole('heading', { name: sectionCase.innerTitle }),
            ).not.toBeInTheDocument();
            sectionCase.assertSelected();
        }
    });
});
