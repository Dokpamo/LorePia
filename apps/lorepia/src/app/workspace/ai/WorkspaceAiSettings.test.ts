import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { INITIAL_APP_STATE, LorepiaAppController } from '../../app-controller';
import { t } from '../../../lib/i18n';
import type { LorepiaClient, ModelRouteDto, GenerationPresetDto } from '../../../lib/ipc/contracts';
import type { ChoiceRequest } from '../../../ui/workspace/settings-choice';
import WorkspaceAiSettings from './WorkspaceAiSettings.svelte';

const mocks = vi.hoisted(() => ({ choice: vi.fn() }));
vi.mock('../../../ui/workspace/choice-sheet.svelte', () => ({
    useChoiceSheet: () => ({ open: mocks.choice }),
}));
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    mocks.choice.mockReset();
});

function setup(legacy = false) {
    const appState = structuredClone(INITIAL_APP_STATE);
    appState.providers.phase = 'ready';
    const workspace = appState.providers.workspace;
    workspace.settings = {
        ...workspace.settings,
        selected_provider_profile_id: legacy ? 'legacy-profile' : null,
        selected_model_route_id: legacy ? null : 'route-a',
        selected_generation_preset_id: legacy ? null : 'preset-a2',
        preserve_partial_generations: false,
    };
    workspace.routes = ['route-a', 'route-b'].map((id) => ({ id, model_id: id }) as ModelRouteDto);
    workspace.presets = [
        { id: 'preset-a1', model_route_id: 'route-a', display_name: 'First preset' },
        { id: 'preset-a2', model_route_id: 'route-a', display_name: 'Saved preset' },
        { id: 'preset-b', model_route_id: 'route-b', display_name: 'Other preset' },
    ] as GenerationPresetDto[];
    const controller = new LorepiaAppController({} as LorepiaClient);
    const onclose = vi.fn();
    const view = render(WorkspaceAiSettings, { appState, controller, onclose });
    return { appState, controller, onclose, view };
}

async function select(label: 'workspaceAi.model' | 'workspaceAi.preset', value: string) {
    const button = screen.getAllByRole('button', { name: t(label) })[0];
    if (!button) throw new Error('Missing AI choice');
    await fireEvent.click(button);
    const request = mocks.choice.mock.calls.at(-1)?.[0] as ChoiceRequest;
    request.onselect(value);
    await tick();
}
const saveButton = () => screen.getByRole('button', { name: t('workspaceAi.save') });
const back = () => fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
const partialSwitch = () => screen.getByRole('switch', { name: t('workspaceAi.partial') });

it('reselects a saved model without replacing its non-first preset or blocking back', async () => {
    const { onclose } = setup();
    await select('workspaceAi.model', 'route-a');
    await select('workspaceAi.preset', 'preset-a2');
    expect(screen.getByText('Saved preset')).toBeInTheDocument();
    expect(saveButton()).toBeDisabled();
    await back();
    expect(onclose).toHaveBeenCalledOnce();
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
});

it.each(['model', 'preset', 'switch'] as const)(
    'clears dirty state after reverting %s',
    async (field) => {
        const { onclose } = setup();
        if (field === 'model') await select('workspaceAi.model', 'route-b');
        else if (field === 'preset') await select('workspaceAi.preset', 'preset-a1');
        else await fireEvent.click(partialSwitch());
        expect(saveButton()).toBeEnabled();
        if (field === 'model') await select('workspaceAi.model', 'route-a');
        else if (field === 'preset') await select('workspaceAi.preset', 'preset-a2');
        else await fireEvent.click(partialSwitch());
        expect(saveButton()).toBeDisabled();
        await back();
        expect(onclose).toHaveBeenCalledOnce();
        expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    },
);

it('keeps a switch draft local, guards back, and saves through the existing controller', async () => {
    const { controller, onclose, appState, view } = setup(true);
    const target = vi.spyOn(controller, 'selectProviderGenerationTarget').mockResolvedValue(true);
    const save = vi
        .spyOn(controller, 'setPreservePartialGenerations')
        .mockImplementation(async (preserve) => {
            await view.rerender({
                appState: {
                    ...appState,
                    providers: {
                        ...appState.providers,
                        workspace: {
                            ...appState.providers.workspace,
                            settings: {
                                ...appState.providers.workspace.settings,
                                preserve_partial_generations: preserve,
                            },
                        },
                    },
                },
                controller,
                onclose,
            });
            return true;
        });
    const control = partialSwitch();
    expect(control.tagName).toBe('BUTTON');
    expect(control).toHaveAttribute('type', 'button');
    control.focus();
    expect(control).toHaveFocus();
    // Native buttons translate Space/Enter activation to a click with detail 0.
    await fireEvent.click(control, { detail: 0 });
    expect(control).toHaveAttribute('aria-checked', 'true');
    expect(mocks.choice).not.toHaveBeenCalled();
    expect(save).not.toHaveBeenCalled();
    await back();
    expect(screen.getByRole('alertdialog')).toBeInTheDocument();
    expect(onclose).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.keepEditing') }));
    expect(partialSwitch()).toHaveAttribute('aria-checked', 'true');
    await fireEvent.click(saveButton());
    expect(save).toHaveBeenCalledWith(true);
    expect(target).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.getByRole('dialog')).toHaveAttribute('aria-busy', 'false'));
    expect(saveButton()).toBeDisabled();
    await back();
    expect(onclose).toHaveBeenCalledOnce();
});

it('does not clear a legacy profile when the empty model is reselected or a draft is reverted', async () => {
    const { onclose } = setup(true);
    await select('workspaceAi.model', '');
    expect(saveButton()).toBeDisabled();
    await select('workspaceAi.model', 'route-a');
    expect(saveButton()).toBeEnabled();
    await select('workspaceAi.model', '');
    expect(saveButton()).toBeDisabled();
    await back();
    expect(onclose).toHaveBeenCalledOnce();
});

it('retains a failed switch save for retry and discards only after confirmation', async () => {
    const { controller, onclose } = setup();
    vi.spyOn(controller, 'setPreservePartialGenerations').mockResolvedValue(false);
    await fireEvent.click(partialSwitch());
    await fireEvent.click(saveButton());
    expect(saveButton()).toBeEnabled();
    expect(partialSwitch()).toHaveAttribute('aria-checked', 'true');
    await back();
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.discardChanges') }));
    expect(onclose).toHaveBeenCalledOnce();
});

it('groups advanced links separately and retains the basic draft beneath advanced navigation', async () => {
    const { controller, onclose } = setup();
    vi.spyOn(controller, 'loadProviderDiagnostics').mockResolvedValue(null);
    const group = screen
        .getByRole('heading', { name: t('workspaceAi.advanced') })
        .closest('section');
    if (!group) throw new Error('Missing advanced settings group');
    for (const key of [
        'settings.page.discovery.provider',
        'settings.section.catalog.title',
        'settings.page.discovery.sync_job',
        'workspaceAi.capabilities',
    ] as const) {
        expect(within(group).getByRole('button', { name: t(key) })).toBeInTheDocument();
    }
    expect(
        within(group).queryByRole('button', { name: t('workspaceAi.connections') }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: t('workspaceAi.preview') })).toHaveClass('secondary');
    expect(saveButton()).not.toHaveClass('secondary');
    await fireEvent.click(partialSwitch());
    await fireEvent.click(
        within(group).getByRole('button', { name: t('settings.section.catalog.title') }),
    );
    await back();
    expect(partialSwitch()).toHaveAttribute('aria-checked', 'true');
    expect(saveButton()).toBeEnabled();
    await back();
    expect(screen.getByRole('alertdialog')).toBeInTheDocument();
    expect(onclose).not.toHaveBeenCalled();
});

it('follows a newly saved legacy target without clearing it when a preserve-only draft is saved', async () => {
    const { controller, appState, view, onclose } = setup();
    const target = vi.spyOn(controller, 'selectProviderGenerationTarget').mockResolvedValue(true);
    const save = vi.spyOn(controller, 'setPreservePartialGenerations').mockResolvedValue(true);
    await fireEvent.click(partialSwitch());
    await view.rerender({
        appState: {
            ...appState,
            providers: {
                ...appState.providers,
                workspace: {
                    ...appState.providers.workspace,
                    settings: {
                        ...appState.providers.workspace.settings,
                        selected_provider_profile_id: 'new-legacy-profile',
                        selected_model_route_id: null,
                        selected_generation_preset_id: null,
                    },
                },
            },
        },
        controller,
        onclose,
    });
    expect(partialSwitch()).toHaveAttribute('aria-checked', 'true');
    await fireEvent.click(saveButton());
    expect(save).toHaveBeenCalledWith(true);
    expect(target).not.toHaveBeenCalled();
});
