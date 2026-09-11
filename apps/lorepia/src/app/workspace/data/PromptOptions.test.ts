import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import PromptOptions from './PromptOptions.svelte';
import type { SettingsServices } from '../../../features/providers/settings/settings-services';
import { orchestrationState } from '../../../features/orchestration/tests/fixtures';
import { t } from '../../../lib/i18n';

import type { ChoiceRequest } from '../../../ui/workspace/settings-choice';
const { open } = vi.hoisted(() => ({
    open: vi.fn<(request: ChoiceRequest, opener: HTMLButtonElement) => void>(),
}));
vi.mock('../../../ui/workspace/choice-sheet.svelte', () => ({ useChoiceSheet: () => ({ open }) }));
afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});
it('edits imported control values through the existing choice sheet and saves the room', async () => {
    const view = orchestrationState();
    const base = view.workspace.creator_controls[0];
    if (!base) throw new Error('Expected control');
    view.workspace.creator_controls.push({
        ...base,
        id: 'enabled',
        label: 'World events',
        kind: 'toggle',
        choices: [],
        value: false,
    });
    view.dirty_room_config = true;
    const stage = vi.fn();
    const save = vi.fn().mockResolvedValue(true);
    const services = {
        orchestrationState: view,
        orchestrationController: { stageCreatorControl: stage, saveRoomConfig: save },
    } as unknown as SettingsServices;
    render(PromptOptions, { services });
    expect(screen.queryByRole('checkbox')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: `${t('importSetup.options')} 2` }));
    await fireEvent.click(screen.getByRole('checkbox', { name: 'World events' }));
    expect(stage).toHaveBeenCalledWith('enabled', true);
    await fireEvent.click(screen.getByRole('button', { name: base.label }));
    const request = open.mock.calls[0]?.[0];
    expect(request?.options.map((item) => item.value)).toEqual(base.choices);
    const choice = base.choices[1];
    if (!request || !choice) throw new Error('Expected choice request');
    request.onselect(choice);
    expect(stage).toHaveBeenCalledWith('tone', choice);
    await fireEvent.click(screen.getByRole('button', { name: t('settingsUi.save') }));
    expect(save).toHaveBeenCalledOnce();
});
