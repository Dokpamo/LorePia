import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import UiNotice from '../ui/workspace/UiNotice.svelte';
import { t } from '../lib/i18n';

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe('current semantic feedback', () => {
    it('keeps a retryable failure announced and actionable until it is dismissed', async () => {
        vi.useFakeTimers();
        const retry = vi.fn();
        const ondismiss = vi.fn();
        render(UiNotice, { notice: { id: 1, text: 'Failed to copy', retry }, ondismiss });
        expect(screen.getByRole('status')).toHaveTextContent('Failed to copy');
        await vi.advanceTimersByTimeAsync(10000);
        expect(ondismiss).not.toHaveBeenCalled();
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.retryReply') }));
        expect(retry).toHaveBeenCalledOnce();
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.dismissNotice') }));
        expect(ondismiss).toHaveBeenCalledOnce();
    });
});
