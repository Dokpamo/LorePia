import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi, type MockInstance } from 'vitest';

import type {
    CharacterRenderProfileDto,
    LorepiaClient,
    MessageDto,
} from '../../../lib/ipc/contracts';
import { t } from '../../../lib/i18n';
import { LorepiaAppController } from '../../../app/app-controller';
import {
    INITIAL_ORCHESTRATION_STATE,
    OrchestrationController,
} from '../../orchestration/orchestration-controller';
import '../../../styles/app.css';
import ChatPane from '../ChatPane.svelte';
import { PortableCharacterRuntime, type PortableRuntimeOptions } from '../portable-runtime';
import { chatReadyState } from './chat-pane-state-builder';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

interface RenderedChat {
    controller: LorepiaAppController;
    sendMessage: MockInstance<LorepiaAppController['sendMessage']>;
}

function renderChatWithSettings(
    appState = chatReadyState(),
    client?: LorepiaClient,
): RenderedChat & { orchestrationController: OrchestrationController } {
    const controller = new LorepiaAppController({} as LorepiaClient);
    const sendMessage = vi.spyOn(controller, 'sendMessage').mockResolvedValue(true);
    const orchestrationController = new OrchestrationController({} as LorepiaClient);
    render(ChatPane, {
        appState,
        controller,
        client,
        orchestrationState: {
            ...structuredClone(INITIAL_ORCHESTRATION_STATE),
            phase: 'ready',
        },
        orchestrationController,
    });
    return { controller, sendMessage, orchestrationController };
}

function portableRuntimeStub(close: () => void = vi.fn()): PortableCharacterRuntime {
    return {
        toggles: [],
        generationVariables: {},
        variables: {},
        backgroundMarkup: '',
        auxiliarySelection: null,
        optionValue: vi.fn().mockReturnValue(''),
        setOption: vi.fn().mockResolvedValue(undefined),
        setAuxiliarySelection: vi.fn(),
        setMessages: vi.fn(),
        refreshDisplay: vi.fn().mockResolvedValue(undefined),
        prepareInput: vi.fn((text: string) => Promise.resolve({ text, shouldSend: true })),
        afterOutput: vi.fn().mockResolvedValue(undefined),
        handleAction: vi.fn().mockResolvedValue(undefined),
        cancelActiveModelCall: vi.fn().mockResolvedValue('not_found'),
        displayText: (message: MessageDto) => message.content,
        effectiveText: (message: MessageDto) => message.content,
        close,
    } as unknown as PortableCharacterRuntime;
}

describe('ChatPane composer', () => {
    it('labels an unconfirmed runtime model cancellation as retryable', async () => {
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-1',
            assets: [],
            background_markup: '',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [],
            display_transforms: [],
            runtime_scripts: [
                {
                    id: 'script-1',
                    name: 'Runtime',
                    event: 'load',
                    language: 'lua',
                    source: '-- no-op',
                    elevated_access: false,
                },
            ],
            required_runtime_capabilities: [],
            runtime_capabilities_declared: false,
            runtime_knowledge: [],
            runtime_script_count: 1,
        };
        const runtime = portableRuntimeStub();
        const cancelActiveModelCall = vi
            .spyOn(runtime, 'cancelActiveModelCall')
            .mockResolvedValue('unconfirmed');
        let reportModelCall: NonNullable<PortableRuntimeOptions['onModelCallStatus']> = () =>
            undefined;
        const createRuntime = vi
            .spyOn(PortableCharacterRuntime, 'create')
            .mockImplementation((options) => {
                reportModelCall = options.onModelCallStatus ?? (() => undefined);
                return Promise.resolve(runtime);
            });
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
        } as unknown as LorepiaClient;
        const { controller, orchestrationController } = renderChatWithSettings(
            chatReadyState(),
            client,
        );

        await fireEvent.click(screen.getByRole('button', { name: t('quick.toggle') }));
        await fireEvent.click(
            await screen.findByRole('button', {
                name: t('chat.runtime.permissions.approve_selected'),
            }),
        );
        await waitFor(() => expect(createRuntime).toHaveBeenCalledOnce());
        reportModelCall({
            requestId: '00000000-0000-4000-8000-000000000020',
            target: 'primary',
            characterName: 'Character',
            startedAt: Date.now(),
        });
        await tick();

        const stop = document.querySelector<HTMLButtonElement>(
            '.portable-runtime-model-call button',
        );
        if (stop === null) throw new Error('runtime model stop action is missing');
        await fireEvent.click(stop);
        await waitFor(() =>
            expect(screen.getByText(t('chat.runtime.cancel.unconfirmed'))).toBeInTheDocument(),
        );
        expect(cancelActiveModelCall).toHaveBeenCalledOnce();

        controller.destroy();
        orchestrationController.destroy();
    });

    it('discards a pending runtime approval when the selected character changes', async () => {
        const profileA: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-a',
            assets: [],
            background_markup: '<div>PROFILE-A</div>',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [],
            display_transforms: [],
            runtime_scripts: [],
            required_runtime_capabilities: [],
            runtime_capabilities_declared: false,
            runtime_knowledge: [],
            runtime_script_count: 0,
        };
        const profileB: CharacterRenderProfileDto = {
            ...profileA,
            character_id: 'character-2',
            character_content_revision_id: 'revision-b',
            background_markup: '<div>PROFILE-B</div>',
            required_runtime_capabilities: ['runtime:callbacks', 'elevated'],
            runtime_capabilities_declared: true,
            runtime_scripts: [
                {
                    id: 'script-b',
                    name: 'Runtime B',
                    event: 'load',
                    language: 'lua',
                    source: '-- must remain blocked',
                    elevated_access: true,
                },
            ],
            runtime_script_count: 1,
        };
        const getCharacterRenderProfile = vi.fn((characterId: string) =>
            Promise.resolve(characterId === 'character-1' ? profileA : profileB),
        );
        const client = { getCharacterRenderProfile } as unknown as LorepiaClient;
        const controller = new LorepiaAppController({} as LorepiaClient);
        const orchestrationController = new OrchestrationController({} as LorepiaClient);
        const appStateA = chatReadyState();
        const rendered = render(ChatPane, {
            appState: appStateA,
            controller,
            client,
            orchestrationState: {
                ...structuredClone(INITIAL_ORCHESTRATION_STATE),
                phase: 'ready',
            },
            orchestrationController,
        });

        await fireEvent.click(screen.getByRole('button', { name: '대화 설정' }));
        const approve = await screen.findByRole('button', {
            name: '선택한 기능만 이번 세션에서 허용',
        });
        let digestPending = false;
        let finishDigest = (value: ArrayBuffer): void => {
            throw new Error(
                `runtime grant digest did not start (${String(value.byteLength)} bytes)`,
            );
        };
        vi.spyOn(globalThis.crypto.subtle, 'digest').mockImplementation(
            () =>
                new Promise<ArrayBuffer>((resolve) => {
                    digestPending = true;
                    finishDigest = resolve;
                }),
        );
        await fireEvent.click(approve);
        await waitFor(() => expect(digestPending).toBe(true));

        const appStateB = chatReadyState();
        const conversationStateB = appStateB.conversation_state;
        if (
            appStateB.selected_character === null ||
            appStateB.selected_conversation === null ||
            conversationStateB === null
        ) {
            throw new Error('chat fixture is incomplete');
        }
        appStateB.selected_character = {
            ...appStateB.selected_character,
            id: 'character-2',
            name: '마루',
        };
        appStateB.selected_conversation = {
            ...appStateB.selected_conversation,
            id: 'conversation-2',
            character_id: 'character-2',
        };
        appStateB.conversation_state = {
            ...conversationStateB,
            conversation_id: 'conversation-2',
        };
        await rendered.rerender({ appState: appStateB });
        await screen.findByRole('checkbox', { name: '고급 카드 권한' });

        finishDigest(new Uint8Array(32).buffer);
        await tick();
        await waitFor(() =>
            expect(
                screen.getByRole('button', {
                    name: '선택한 기능만 이번 세션에서 허용',
                }),
            ).toBeInTheDocument(),
        );
        expect(screen.queryByRole('button', { name: '캐릭터 기능 권한 해제' })).toBeNull();
        expect(document.querySelector('.portable-runtime-background .portable-frame')).toBeNull();

        controller.destroy();
        orchestrationController.destroy();
    });

    it('does not expose chat text or indices to card UI when chat read is denied', async () => {
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-1',
            assets: [],
            background_markup:
                '<button card-btn="safe{{lastcharmessage}}{{chat_index}}{{lastmessageid}}">Run</button>',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [],
            display_transforms: [
                {
                    pattern: '^(SECRET-CHAT-CONTENT)$',
                    replacement: '<button card-btn="$1">Continue</button>',
                    flags: '',
                },
            ],
            runtime_scripts: [],
            required_runtime_capabilities: [],
            runtime_capabilities_declared: false,
            runtime_knowledge: [],
            runtime_script_count: 0,
        };
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'message-secret',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: 'SECRET-CHAT-CONTENT',
                status: 'complete',
                generation_id: null,
                created_at: '2026-08-02T00:00:00Z',
            },
        ];
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
            resolveAssetDelivery: vi.fn(),
        } as unknown as LorepiaClient;
        const { controller, orchestrationController } = renderChatWithSettings(appState, client);

        await fireEvent.click(screen.getByRole('button', { name: '대화 설정' }));
        const chatRead = await screen.findByRole('checkbox', { name: '현재 대화 읽기' });
        expect(chatRead).not.toBeChecked();
        await fireEvent.click(
            screen.getByRole('button', { name: '선택한 기능만 이번 세션에서 허용' }),
        );

        await waitFor(() => {
            const frame = document.querySelector<HTMLIFrameElement>(
                '.portable-runtime-background .portable-frame',
            );
            expect(frame?.srcdoc).toContain('data-portable-action="safe"');
        });
        const frame = document.querySelector<HTMLIFrameElement>(
            '.portable-runtime-background .portable-frame',
        );
        expect(frame?.srcdoc).not.toContain('SECRET-CHAT-CONTENT');
        expect(frame?.srcdoc).not.toContain('data-portable-action="safe10"');
        expect(document.querySelectorAll('.portable-frame')).toHaveLength(1);
        expect(screen.getByText('SECRET-CHAT-CONTENT')).toBeInTheDocument();
        controller.destroy();
        orchestrationController.destroy();
    });

    it('applies imported message transforms only for an active reviewed grant', async () => {
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-1',
            assets: [],
            background_markup: '',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [
                {
                    pattern: '^RAW-CARD-OUTPUT$',
                    replacement: 'REVIEWED-CARD-OUTPUT',
                    flags: '',
                },
            ],
            display_transforms: [
                {
                    pattern: '^REVIEWED-CARD-OUTPUT$',
                    replacement: '<div data-reviewed="true">APPROVED-CARD-DISPLAY</div>',
                    flags: '',
                },
            ],
            runtime_scripts: [],
            required_runtime_capabilities: ['chat:read', 'profile:read', 'ui:write'],
            runtime_capabilities_declared: true,
            runtime_knowledge: [],
            runtime_script_count: 0,
        };
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'message-card-output',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: 'RAW-CARD-OUTPUT',
                status: 'complete',
                generation_id: 'generation-card-output',
                created_at: '2026-08-02T00:00:00Z',
            },
        ];
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
            resolveAssetDelivery: vi.fn(),
        } as unknown as LorepiaClient;
        const { controller, orchestrationController } = renderChatWithSettings(appState, client);

        await fireEvent.click(screen.getByRole('button', { name: t('quick.toggle') }));
        const approve = await screen.findByRole('button', {
            name: t('chat.runtime.permissions.approve_selected'),
        });
        expect(screen.getByText('RAW-CARD-OUTPUT')).toBeInTheDocument();
        expect(document.querySelector('.portable-frame')).toBeNull();

        // The default grant deliberately withholds chat/profile read access.
        await fireEvent.click(approve);
        await waitFor(() =>
            expect(document.querySelector('.portable-runtime-revoke')).not.toBeNull(),
        );
        expect(screen.getByText('RAW-CARD-OUTPUT')).toBeInTheDocument();
        expect(document.querySelector('.portable-frame')).toBeNull();

        const deniedGrantRevoke = document.querySelector<HTMLButtonElement>(
            '.portable-runtime-revoke',
        );
        if (deniedGrantRevoke === null) throw new Error('runtime revoke action is missing');
        await fireEvent.click(deniedGrantRevoke);
        const chatRead = await screen.findByRole('checkbox', {
            name: t('chat.runtime.capability.chat_read'),
        });
        const profileRead = screen.getByRole('checkbox', {
            name: t('chat.runtime.capability.profile_read'),
        });
        await fireEvent.click(chatRead);
        await fireEvent.click(profileRead);
        await fireEvent.click(
            screen.getByRole('button', {
                name: t('chat.runtime.permissions.approve_selected'),
            }),
        );

        await waitFor(() => {
            const frame = document.querySelector<HTMLIFrameElement>('.portable-frame');
            expect(frame?.srcdoc).toContain('APPROVED-CARD-DISPLAY');
        });
        expect(screen.queryByText('RAW-CARD-OUTPUT')).toBeNull();

        const reviewedGrantRevoke = document.querySelector<HTMLButtonElement>(
            '.portable-runtime-revoke',
        );
        if (reviewedGrantRevoke === null) throw new Error('runtime revoke action is missing');
        await fireEvent.click(reviewedGrantRevoke);
        await waitFor(() => expect(document.querySelector('.portable-frame')).toBeNull());
        expect(screen.getByText('RAW-CARD-OUTPUT')).toBeInTheDocument();

        controller.destroy();
        orchestrationController.destroy();
    });

    it('keeps imported transforms inert for an explicit empty capability declaration', async () => {
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-explicit-empty',
            assets: [],
            background_markup: '',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [
                {
                    pattern: '^EXPLICIT-EMPTY-OUTPUT$',
                    replacement: '<div>SHOULD-NOT-RENDER</div>',
                    flags: '',
                },
            ],
            display_transforms: [],
            runtime_scripts: [],
            required_runtime_capabilities: [],
            runtime_capabilities_declared: true,
            runtime_knowledge: [],
            runtime_script_count: 0,
        };
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'message-explicit-empty',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: 'EXPLICIT-EMPTY-OUTPUT',
                status: 'complete',
                generation_id: null,
                created_at: '2026-08-02T00:00:00Z',
            },
        ];
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
            resolveAssetDelivery: vi.fn(),
        } as unknown as LorepiaClient;
        const { controller, orchestrationController } = renderChatWithSettings(appState, client);

        await fireEvent.click(screen.getByRole('button', { name: t('quick.toggle') }));
        const approve = await screen.findByRole('button', {
            name: t('chat.runtime.permissions.approve_selected'),
        });
        expect(
            screen.queryByRole('checkbox', { name: t('chat.runtime.capability.chat_read') }),
        ).toBeNull();
        await fireEvent.click(approve);
        await waitFor(() =>
            expect(document.querySelector('.portable-runtime-revoke')).not.toBeNull(),
        );

        expect(screen.getByText('EXPLICIT-EMPTY-OUTPUT')).toBeInTheDocument();
        expect(screen.queryByText('SHOULD-NOT-RENDER')).toBeNull();
        expect(document.querySelector('.portable-frame')).toBeNull();

        controller.destroy();
        orchestrationController.destroy();
    });

    it('keeps ordinary chat available while imported runtime code remains unapproved', async () => {
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-1',
            assets: [],
            background_markup: '',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [],
            display_transforms: [],
            runtime_scripts: [
                {
                    id: 'script-1',
                    name: 'Runtime',
                    event: 'load',
                    language: 'lua',
                    source: '-- must remain inert',
                    elevated_access: false,
                },
            ],
            required_runtime_capabilities: [],
            runtime_capabilities_declared: false,
            runtime_knowledge: [],
            runtime_script_count: 1,
        };
        const createRuntime = vi.spyOn(PortableCharacterRuntime, 'create');
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
        } as unknown as LorepiaClient;
        const { controller, sendMessage, orchestrationController } = renderChatWithSettings(
            chatReadyState(),
            client,
        );

        await fireEvent.click(screen.getByRole('button', { name: '대화 설정' }));
        await screen.findByRole('button', {
            name: '선택한 기능만 이번 세션에서 허용',
        });
        expect(
            screen.queryByRole('checkbox', { name: t('chat.runtime.capability.model_primary') }),
        ).toBeNull();
        expect(
            screen.queryByRole('checkbox', { name: t('chat.runtime.capability.model_auxiliary') }),
        ).toBeNull();
        expect(
            screen.getByRole('checkbox', { name: t('chat.runtime.capability.callbacks') }),
        ).toBeChecked();
        expect(
            screen.getByRole('checkbox', { name: t('chat.runtime.capability.ui_write') }),
        ).toBeChecked();
        const composer = screen.getByRole('textbox', { name: '메시지' });
        await fireEvent.input(composer, { target: { value: '안전 모드 대화' } });
        await fireEvent.click(screen.getByRole('button', { name: '메시지 보내기' }));

        await waitFor(() => expect(sendMessage).toHaveBeenCalledWith('안전 모드 대화'));
        expect(createRuntime).not.toHaveBeenCalled();
        controller.destroy();
        orchestrationController.destroy();
    });

    it('sends customized character runtime values without persisting them as room settings', async () => {
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-1',
            assets: [],
            background_markup: '',
            toggle_schema: 'music=배경음악=toggle',
            initial_variables: { music: '0' },
            output_transforms: [],
            display_transforms: [],
            runtime_scripts: [
                {
                    id: 'script-1',
                    name: 'Runtime',
                    event: 'load',
                    language: 'lua',
                    source: '-- no-op',
                    elevated_access: false,
                },
            ],
            required_runtime_capabilities: [],
            runtime_capabilities_declared: false,
            runtime_knowledge: [],
            runtime_script_count: 1,
        };
        let music = '0';
        const setOption = vi.fn((_key: string, value: string) => {
            music = value;
            return Promise.resolve();
        });
        const runtime = {
            toggles: [{ key: 'music', label: '배경음악', kind: 'toggle', choices: [] }],
            get generationVariables() {
                return { music };
            },
            get variables() {
                return { music };
            },
            backgroundMarkup: '',
            auxiliarySelection: null,
            optionValue: (key: string) => (key === 'music' ? music : ''),
            setOption,
            setAuxiliarySelection: vi.fn(),
            setMessages: vi.fn(),
            refreshDisplay: vi.fn().mockResolvedValue(undefined),
            prepareInput: vi.fn((text: string) => Promise.resolve({ text, shouldSend: true })),
            afterOutput: vi.fn().mockResolvedValue(undefined),
            handleAction: vi.fn().mockResolvedValue(undefined),
            displayText: (message: MessageDto) => message.content,
            effectiveText: (message: MessageDto) => message.content,
            close: vi.fn(),
        } as unknown as PortableCharacterRuntime;
        const createRuntime = vi
            .spyOn(PortableCharacterRuntime, 'create')
            .mockResolvedValue(runtime);
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
        } as unknown as LorepiaClient;
        const { controller, sendMessage, orchestrationController } = renderChatWithSettings(
            chatReadyState(),
            client,
        );

        await fireEvent.click(screen.getByRole('button', { name: '대화 설정' }));
        expect(createRuntime).not.toHaveBeenCalled();
        expect(screen.queryByRole('switch', { name: '배경음악' })).not.toBeInTheDocument();
        await fireEvent.click(
            await screen.findByRole('button', {
                name: t('chat.runtime.permissions.approve_selected'),
            }),
        );
        const musicToggle = await screen.findByRole('switch', { name: '배경음악' });
        await fireEvent.click(musicToggle);
        await waitFor(() => expect(setOption).toHaveBeenCalledWith('music', '1'));

        const composer = screen.getByRole('textbox', { name: '메시지' });
        await fireEvent.input(composer, { target: { value: '카드 옵션 테스트' } });
        await fireEvent.click(screen.getByRole('button', { name: '메시지 보내기' }));

        await waitFor(() =>
            expect(sendMessage).toHaveBeenCalledWith('카드 옵션 테스트', {
                values: [
                    {
                        variable: { scope: 'character', namespace: null, id: 'music' },
                        value: { type: 'text', value: '1' },
                    },
                ],
            }),
        );
        controller.destroy();
        orchestrationController.destroy();
    });

    it.each([
        [
            'recreates the portable runtime after a refreshed same-branch rewind',
            {
                mutationCommitted: true,
                messagesRefreshed: true,
                scopeKey: 'conversation-1:branch-1',
            },
        ],
        [
            'recreates the portable runtime when rewind committed but readback failed',
            {
                mutationCommitted: true,
                messagesRefreshed: false,
                scopeKey: 'conversation-1:branch-1',
            },
        ],
    ] as const)('%s', async (_label, removalResult) => {
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'assistant-1',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: 'rewind target',
                status: 'complete',
                generation_id: 'generation-1',
                created_at: '2026-08-29T00:00:00Z',
            },
        ];
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-1',
            assets: [],
            background_markup: '',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [],
            display_transforms: [],
            runtime_scripts: [
                {
                    id: 'script-1',
                    name: 'Runtime',
                    event: 'load',
                    language: 'lua',
                    source: '-- no-op',
                    elevated_access: false,
                },
            ],
            required_runtime_capabilities: [],
            runtime_capabilities_declared: false,
            runtime_knowledge: [],
            runtime_script_count: 1,
        };
        const firstRuntimeClose = vi.fn();
        const firstRuntime = portableRuntimeStub(firstRuntimeClose);
        const secondRuntime = portableRuntimeStub();
        const createRuntime = vi
            .spyOn(PortableCharacterRuntime, 'create')
            .mockResolvedValueOnce(firstRuntime)
            .mockResolvedValueOnce(secondRuntime);
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
        } as unknown as LorepiaClient;
        const { controller, orchestrationController } = renderChatWithSettings(appState, client);
        const removeMessage = vi
            .spyOn(controller, 'removeMessage')
            .mockResolvedValue(removalResult);

        await fireEvent.click(screen.getByRole('button', { name: t('quick.toggle') }));
        await fireEvent.click(
            await screen.findByRole('button', {
                name: t('chat.runtime.permissions.approve_selected'),
            }),
        );
        await waitFor(() => expect(createRuntime).toHaveBeenCalledTimes(1));

        await fireEvent.click(
            screen.getByRole('button', { name: t('chat.message.remove_from_here') }),
        );
        await fireEvent.click(
            screen.getByRole('button', { name: t('chat.message.remove_confirm') }),
        );

        await waitFor(() => expect(removeMessage).toHaveBeenCalledWith('assistant-1'));
        await waitFor(() => expect(createRuntime).toHaveBeenCalledTimes(2));
        expect(firstRuntimeClose).toHaveBeenCalledOnce();
        controller.destroy();
        orchestrationController.destroy();
    });

    it('does not reset the newly selected room runtime after a stale rewind receipt', async () => {
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'assistant-1',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: 'rewind target',
                status: 'complete',
                generation_id: 'generation-1',
                created_at: '2026-08-29T00:00:00Z',
            },
        ];
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-1',
            assets: [],
            background_markup: '',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [],
            display_transforms: [],
            runtime_scripts: [
                {
                    id: 'script-1',
                    name: 'Runtime',
                    event: 'load',
                    language: 'lua',
                    source: '-- no-op',
                    elevated_access: false,
                },
            ],
            required_runtime_capabilities: [],
            runtime_capabilities_declared: false,
            runtime_knowledge: [],
            runtime_script_count: 1,
        };
        const firstRuntimeClose = vi.fn();
        const secondRuntimeClose = vi.fn();
        const createRuntime = vi
            .spyOn(PortableCharacterRuntime, 'create')
            .mockResolvedValueOnce(portableRuntimeStub(firstRuntimeClose))
            .mockResolvedValueOnce(portableRuntimeStub(secondRuntimeClose));
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
        } as unknown as LorepiaClient;
        const controller = new LorepiaAppController({} as LorepiaClient);
        const orchestrationController = new OrchestrationController({} as LorepiaClient);
        const rendered = render(ChatPane, {
            appState,
            controller,
            client,
            orchestrationState: {
                ...structuredClone(INITIAL_ORCHESTRATION_STATE),
                phase: 'ready',
            },
            orchestrationController,
        });
        const removalReceipt: { resolve: (() => void) | null } = { resolve: null };
        const removeMessage = vi.spyOn(controller, 'removeMessage').mockImplementation(
            () =>
                new Promise((resolve) => {
                    removalReceipt.resolve = () =>
                        resolve({
                            mutationCommitted: true,
                            messagesRefreshed: false,
                            scopeKey: 'conversation-1:branch-1',
                        });
                }),
        );

        await fireEvent.click(screen.getByRole('button', { name: t('quick.toggle') }));
        await fireEvent.click(
            await screen.findByRole('button', {
                name: t('chat.runtime.permissions.approve_selected'),
            }),
        );
        await waitFor(() => expect(createRuntime).toHaveBeenCalledTimes(1));

        await fireEvent.click(
            screen.getByRole('button', { name: t('chat.message.remove_from_here') }),
        );
        await fireEvent.click(
            screen.getByRole('button', { name: t('chat.message.remove_confirm') }),
        );
        await waitFor(() => expect(removeMessage).toHaveBeenCalledWith('assistant-1'));

        const nextAppState = structuredClone(appState);
        if (
            nextAppState.selected_conversation === null ||
            nextAppState.conversation_state === null
        ) {
            throw new Error('chat fixture is incomplete');
        }
        nextAppState.selected_conversation.id = 'conversation-2';
        nextAppState.conversation_state = {
            ...nextAppState.conversation_state,
            conversation_id: 'conversation-2',
            active_branch_id: 'branch-2',
        };
        nextAppState.messages.items = [];
        await rendered.rerender({ appState: nextAppState });
        await fireEvent.click(
            await screen.findByRole('button', {
                name: t('chat.runtime.permissions.approve_selected'),
            }),
        );
        await waitFor(() => expect(createRuntime).toHaveBeenCalledTimes(2));
        expect(firstRuntimeClose).toHaveBeenCalledOnce();

        const resolveRemoval = removalReceipt.resolve;
        if (resolveRemoval === null) throw new Error('removal receipt was not requested');
        resolveRemoval();
        await tick();

        expect(createRuntime).toHaveBeenCalledTimes(2);
        expect(secondRuntimeClose).not.toHaveBeenCalled();
        controller.destroy();
        orchestrationController.destroy();
    });
});
