import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import { createPreviewClient } from '../../preview/mock-client';
import { openWorkspaceChat } from '../../tests/workspace-chat';
import { chatDisplayPreferences } from '../../lib/chat-display';

beforeEach(() => {
    chatDisplayPreferences.set({});
    vi.stubGlobal(
        'ResizeObserver',
        class {
            observe = vi.fn();
            unobserve = vi.fn();
            disconnect = vi.fn();
        },
    );
});
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    chatDisplayPreferences.set({});
    localStorage.clear();
});

it('opens settings as an expandable sheet and retains a dirty choice when continuing editing', async () => {
    const { container } = await openWorkspaceChat();
    const trigger = screen.getByRole('button', { name: t('uiPreview.roomSettings') });
    await fireEvent.click(trigger);
    const sheet = screen.getByRole('dialog', { name: t('uiPreview.roomSettings') });
    expect(sheet).toHaveClass('ui-choice-sheet');
    expect(sheet.querySelector('.ui-page-header')).toBeNull();
    await fireEvent.click(within(sheet).getByRole('button', { name: t('uiPreview.expandSheet') }));
    expect(sheet).toHaveAttribute('data-sheet-expanded', 'true');
    const radio = within(sheet).getAllByRole('radio')[0];
    if (!radio) throw new Error('Missing mode');
    await fireEvent.click(radio);
    await fireEvent.click(within(sheet).getByRole('button', { name: t('uiPreview.closeChoices') }));
    await waitFor(() => expect(container.querySelector('.ui-confirm')).not.toBeNull());
    expect(sheet.style.getPropertyValue('--ui-choice-drag')).toBe('100%');
    const keep = container.querySelector<HTMLButtonElement>(
        '.ui-confirm-actions button:not(.ui-discard)',
    );
    if (!keep) throw new Error('Missing continue action');
    await fireEvent.click(keep);
    await waitFor(() => expect(sheet.style.getPropertyValue('--ui-choice-drag')).toBe('0px'));
    expect(radio).toBeChecked();
    await fireEvent.click(within(sheet).getByRole('button', { name: t('uiPreview.save') }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(trigger).toHaveFocus();
});

it('manages grants in settings and shows approved card UI in chat without opening the right page', async () => {
    const client = createPreviewClient();
    const original = client.getCharacterRenderProfile?.bind(client);
    if (!original) throw new Error('Missing render profile API');
    vi.spyOn(client, 'getCharacterRenderProfile').mockImplementation(async (...args) => ({
        ...(await original(...args)),
        runtime_scripts: [],
        display_transforms: [],
        runtime_script_count: 0,
        background_markup:
            '<div style="position:fixed;right:16px;top:12px">{{button::Card settings::settings}}</div>',
        required_runtime_capabilities: ['chat:read', 'ui:write'],
        runtime_capabilities_declared: true,
    }));
    const { container } = await openWorkspaceChat(client);
    expect(container.querySelector('.ui-card-floating')).toBeNull();
    expect(screen.queryByText(t('chat.runtime.enableDisplay'))).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.roomSettings') }));
    const sheet = screen.getByRole('dialog', { name: t('uiPreview.roomSettings') });
    await fireEvent.click(
        await within(sheet).findByRole('button', { name: t('chat.runtime.enableDisplay') }),
    );
    await within(sheet).findByRole('button', { name: t('workspaceRuntime.revoke') });
    await fireEvent.click(within(sheet).getByRole('button', { name: t('uiPreview.closeChoices') }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    await waitFor(() =>
        expect(
            container.querySelector<HTMLIFrameElement>('.ui-chat .ui-card-floating iframe')?.srcdoc,
        ).toContain('Card settings'),
    );
    expect(container.querySelector('.ui-creator iframe')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: t('chat.runtime.openRoom') }));
    await waitFor(() => expect(container.querySelector('.ui-chat .ui-card-floating')).toBeNull());
    expect(container.querySelector('.ui-creator iframe')).not.toBeNull();
    expect(screen.queryByText(t('chat.runtime.permissions.approve_selected'))).toBeNull();
});
