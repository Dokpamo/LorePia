import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { LorepiaClient } from '../../lib/ipc/contracts';
import OrchestrationStudio from './OrchestrationStudio.svelte';
import { ContentPackageController } from './content-package-controller';
import {
    completedContentPackageState,
    contentPackageSelectionState,
    contentPackageState,
    restartedCompletedExportState,
} from './tests/content-package-fixtures';
import { appState, controller, orchestrationState } from './tests/fixtures';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe('OrchestrationStudio', () => {
    it('renders only the safe selective package review and disables quarantined components', () => {
        render(OrchestrationStudio, {
            section: 'content',
            detailPage: 'packages',
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: contentPackageState(),
            contentPackageController: new ContentPackageController({} as LorepiaClient),
        });

        expect(
            screen.getByRole('region', { name: 'LorePia 패키지 선택 가져오기' }),
        ).toBeInTheDocument();
        expect(
            screen.getByRole('heading', { name: /<img src=x onerror=alert\(1\)>/ }),
        ).toBeInTheDocument();
        expect(document.querySelector('img')).toBeNull();
        expect(screen.getByLabelText(/component-safe/)).toBeEnabled();
        expect(screen.getByLabelText(/component-quarantined/)).toBeDisabled();
        expect(screen.getByText('실행 가능한 변환은 비활성 격리됨')).toBeInTheDocument();
        expect(document.body.textContent).not.toContain('/Users/');
        expect(document.body.textContent).not.toContain('raw-script-body');
    });

    it('exports only the just-completed package and renders safe delivery evidence', async () => {
        const packageController = new ContentPackageController({} as LorepiaClient);
        const exportCompletedPackage = vi
            .spyOn(packageController, 'exportCompletedPackage')
            .mockResolvedValue(true);
        const packageState = completedContentPackageState();
        packageState.export_receipt = {
            kind: 'lorepia_package',
            source_id: 'import-1',
            sha256: '9'.repeat(64),
            size_bytes: 8192,
            file_name: 'package.synthetic.lorepia.zip',
        };
        render(OrchestrationStudio, {
            section: 'content',
            detailPage: 'packages',
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: packageController,
        });
        expect(screen.getByRole('heading', { name: '가져오기 완료' })).toBeInTheDocument();
        expect(screen.getByRole('heading', { name: '최근 패키지 내보내기' })).toBeInTheDocument();
        expect(screen.getByText('파일명 package.synthetic.lorepia.zip')).toBeVisible();
        expect(screen.getByText('크기 8192바이트')).toBeVisible();
        expect(screen.getByText('9'.repeat(64))).toBeVisible();
        const exportButton = screen.getByRole('button', { name: '완료된 패키지 내보내기' });
        await fireEvent.click(exportButton);
        expect(exportCompletedPackage).toHaveBeenCalledOnce();
        expect(document.body.textContent).not.toContain('/Users/');
        expect(document.body.textContent).not.toContain('raw bytes');

        cleanup();
        packageState.exporting_import_id = 'import-1';
        render(OrchestrationStudio, {
            section: 'content',
            detailPage: 'packages',
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: packageController,
        });
        expect(screen.getByRole('button', { name: '내보내는 중…' })).toBeDisabled();
        expect(screen.getByRole('status')).toHaveTextContent(
            '운영체제 저장 위치를 선택하고 있습니다.',
        );
    });

    it('renders the restart-safe completed package catalog in backend order and exports a row', async () => {
        const packageController = new ContentPackageController({} as LorepiaClient);
        const exportFromCatalog = vi
            .spyOn(packageController, 'exportCompletedPackageFromCatalog')
            .mockResolvedValue(true);
        const reload = vi
            .spyOn(packageController, 'loadCompletedPackageExports')
            .mockResolvedValue(true);
        const packageState = restartedCompletedExportState();
        render(OrchestrationStudio, {
            section: 'content',
            detailPage: 'packages',
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: packageController,
        });
        const catalog = screen.getByRole('list', { name: '완료된 패키지 내보내기 목록' });
        const rows = within(catalog).getAllByRole('listitem');
        const [newerRow, olderRow] = rows;
        if (newerRow === undefined || olderRow === undefined) {
            throw new Error('synthetic completed export rows are missing');
        }
        expect(newerRow).toHaveTextContent('newer.lorepia.zip');
        expect(olderRow).toHaveTextContent('older.lorepia.zip');
        expect(within(newerRow).getByText('8'.repeat(64))).toBeVisible();
        expect(within(olderRow).getByText('크기 4096바이트')).toBeVisible();
        expect(screen.queryByRole('heading', { name: '가져오기 완료' })).toBeNull();

        await fireEvent.click(
            screen.getByRole('button', { name: 'older.lorepia.zip 완료 패키지 내보내기' }),
        );
        expect(exportFromCatalog).toHaveBeenCalledWith('import-1');
        await fireEvent.click(screen.getByRole('button', { name: '목록 새로고침' }));
        expect(reload).toHaveBeenCalledOnce();
    });

    it('bounds a corrupt oversized completed package catalog before rendering actions', () => {
        const packageState = restartedCompletedExportState();
        packageState.completed_package_exports = Array.from({ length: 101 }, (_, index) => ({
            kind: 'lorepia_package' as const,
            source_id: `import-${String(index)}`,
            sha256: 'a'.repeat(64),
            size_bytes: index + 1,
            suggested_file_name: `package-${String(index)}.lorepia.zip`,
        }));
        render(OrchestrationStudio, {
            section: 'content',
            detailPage: 'packages',
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: new ContentPackageController({} as LorepiaClient),
        });
        expect(screen.getByText('package-99.lorepia.zip')).toBeVisible();
        expect(screen.queryByText('package-100.lorepia.zip')).toBeNull();
        expect(screen.getByText(/처음 100개 완료 패키지만 표시합니다/)).toBeVisible();
        expect(screen.getAllByRole('button', { name: /완료 패키지 내보내기$/ })).toHaveLength(100);
    });

    it('shows the exact target review and keeps approval disabled until every update target is confirmed', async () => {
        const packageController = new ContentPackageController({} as LorepiaClient);
        const toggleConfirmation = vi
            .spyOn(packageController, 'toggleUpdateTargetConfirmation')
            .mockReturnValue(true);
        const packageState = contentPackageSelectionState();
        render(OrchestrationStudio, {
            section: 'content',
            detailPage: 'packages',
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: packageController,
        });
        expect(screen.getByRole('heading', { name: '대상 쓰기 검토' })).toBeInTheDocument();
        expect(screen.getByText('1'.repeat(64))).toBeInTheDocument();
        expect(screen.getAllByText('2'.repeat(64))).toHaveLength(2);
        expect(screen.getByText('3'.repeat(64))).toBeInTheDocument();
        expect(screen.getByText('prompt-revision-7')).toBeInTheDocument();
        expect(screen.getByText(/기대 상태 CAS\s*8/)).toBeInTheDocument();
        expect(screen.getByText(/component-safe · 전체 문서 인덱스 0/)).toBeInTheDocument();
        expect(screen.getByText(/새 대상 생성 — 별도 업데이트 확인 불필요/)).toBeInTheDocument();
        const updateConfirmation = screen.getByLabelText('prompt-existing 기존 대상 업데이트 확인');
        expect(updateConfirmation).toBeEnabled();
        expect(
            screen.getByRole('button', { name: '표시된 근거와 기능 명시적 승인' }),
        ).toBeDisabled();
        await fireEvent.click(updateConfirmation);
        expect(toggleConfirmation).toHaveBeenCalledWith('component-safe', 0);

        cleanup();
        packageState.confirmed_update_targets = [
            {
                source_component_id: 'component-safe',
                component_document_ordinal: 0,
                target_object_id: 'prompt-existing',
                expected_target_revision_id: 'prompt-revision-7',
                expected_target_state_revision: 8,
            },
        ];
        render(OrchestrationStudio, {
            section: 'content',
            detailPage: 'packages',
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: packageController,
        });
        expect(
            screen.getByRole('button', { name: '표시된 근거와 기능 명시적 승인' }),
        ).toBeEnabled();
    });

    it('bounds target-review rendering and does not expose an unconfirmed hidden update', () => {
        const packageState = contentPackageSelectionState();
        const selectionReview = packageState.selection;
        if (selectionReview === null) throw new Error('synthetic package selection is missing');
        selectionReview.target_review.documents = Array.from({ length: 201 }, (_, index) => ({
            source_component_id: 'component-safe',
            component_document_ordinal: index,
            document_index: index,
            document_kind: 'prompt_preset' as const,
            target_object_id: `prompt-target-${String(index)}`,
            disposition: 'update' as const,
            expected_target_revision_id: `prompt-revision-${String(index)}`,
            expected_target_state_revision: index + 1,
            source_component_sha256: '2'.repeat(64),
            document_sha256: '3'.repeat(64),
        }));
        render(OrchestrationStudio, {
            section: 'content',
            detailPage: 'packages',
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: new ContentPackageController({} as LorepiaClient),
        });
        expect(screen.getByText(/처음 200개 대상 문서만 표시합니다/)).toBeInTheDocument();
        expect(screen.getByText('prompt-target-199')).toBeInTheDocument();
        expect(screen.queryByText('prompt-target-200')).not.toBeInTheDocument();
        expect(
            screen.getByRole('button', { name: '표시된 근거와 기능 명시적 승인' }),
        ).toBeDisabled();
    });
});
