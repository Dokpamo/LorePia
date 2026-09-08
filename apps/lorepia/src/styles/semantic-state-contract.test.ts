import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import ChatErrorRegion from '../features/chat/ChatErrorRegion.svelte';
import { styleRules } from '../tests/css-rules';
import appCss from './app-css';

afterEach(cleanup);

describe('semantic feedback', () => {
    it('defines complete semantic feedback palettes for every theme', () => {
        const palettes = styleRules(appCss).filter(
            (rule) => rule.declarations['--status-error-fg'],
        );
        expect(palettes.length).toBeGreaterThanOrEqual(2);
        for (const palette of palettes) {
            for (const state of ['error', 'warning', 'success', 'info']) {
                for (const part of ['fg', 'bg', 'border'])
                    expect(
                        palette.declarations[`--status-${state}-${part}`],
                        palette.selector,
                    ).toBeTruthy();
            }
        }
    });

    it('announces independent errors and dispatches the matching dismiss action', async () => {
        const onDismissChat = vi.fn();
        const onDismissRuntime = vi.fn();
        const onDismissInteraction = vi.fn();
        const result = render(ChatErrorRegion, {
            chatError: 'Chat failed',
            runtimeError: 'Runtime failed',
            interactionError: 'Interaction failed',
            onDismissChat,
            onDismissRuntime,
            onDismissInteraction,
        });
        expect(screen.getAllByRole('alert')).toHaveLength(3);
        const runtimeAlert = screen.getByText('Runtime failed').closest('[role=alert]');
        if (!(runtimeAlert instanceof HTMLElement)) throw new Error('Missing runtime alert');
        const dismissRuntime = within(runtimeAlert).getByRole('button');
        expect(dismissRuntime).toHaveAccessibleName();
        await fireEvent.click(dismissRuntime);
        expect(onDismissRuntime).toHaveBeenCalledOnce();
        expect(onDismissChat).not.toHaveBeenCalled();
        expect(onDismissInteraction).not.toHaveBeenCalled();
        await result.rerender({
            chatError: 'Chat failed',
            runtimeError: null,
            interactionError: null,
            onDismissChat,
            onDismissRuntime,
            onDismissInteraction,
        });
        expect(screen.getAllByRole('alert')).toHaveLength(1);
        expect(screen.getByRole('alert')).toHaveTextContent('Chat failed');
    });
});
