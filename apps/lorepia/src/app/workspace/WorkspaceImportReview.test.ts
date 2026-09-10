import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
import WorkspaceImportReview from './WorkspaceImportReview.svelte';

afterEach(cleanup);

it('keeps blocked content uncommittable and discards staged import from the back action', async () => {
    const commitImport = vi.fn();
    const discardImport = vi.fn();
    const controller = { commitImport, discardImport } as unknown as LorepiaAppController;
    const state = {
        import_flow: {
            phase: 'ready',
            error: null,
            resource_override_active: false,
            inspection: {
                inspection_id: 'review',
                kind: 'charx_package',
                display_name: 'Blocked card',
                description: 'Review fixture',
                source_size: 1024,
                estimated_stored_size: 2048,
                asset_count: 0,
                allowed: false,
                blocked_reasons: ['Blocked archive'],
                warnings: [],
                unsupported_optional_fields: [],
                dynamic_content: {
                    runtime_script_count: 0,
                    regex_rule_count: 0,
                    custom_markup_present: false,
                    regex_rules: [],
                },
            },
        },
    } as unknown as LorepiaAppState;
    render(WorkspaceImportReview, { state, controller });
    expect(screen.getByRole('dialog', { name: t('import.title') })).toBeInTheDocument();
    expect(screen.getByText('Blocked archive')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: t('import.commit') })).toBeDisabled();
    expect(commitImport).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: t('import.dialog.close') }));
    expect(discardImport).toHaveBeenCalledOnce();
});

it('only offers the explicit native resource retry when the controller reports it available', async () => {
    const beginImport = vi.fn();
    const controller = { beginImport, discardImport: vi.fn() } as unknown as LorepiaAppController;
    const state = {
        import_flow: {
            phase: 'error',
            error: t('import.error.resource_limit'),
            inspection: null,
            resource_override_active: false,
            resource_override_available: false,
        },
    } as unknown as LorepiaAppState;
    const view = render(WorkspaceImportReview, { state, controller });
    expect(
        screen.queryByRole('button', { name: t('import.resource.retry') }),
    ).not.toBeInTheDocument();
    await view.rerender({
        state: {
            ...state,
            import_flow: { ...state.import_flow, resource_override_available: true },
        },
        controller,
    });
    expect(screen.getByText(t('import.resource.retry_description'))).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: t('import.resource.retry') }));
    expect(beginImport).toHaveBeenCalledOnce();
});
