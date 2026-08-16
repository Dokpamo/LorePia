import eslint from '@eslint/js';
import { defineConfig } from 'eslint/config';
import prettier from 'eslint-config-prettier';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import tseslint from 'typescript-eslint';

export default defineConfig(
    {
        ignores: ['dist/**', 'node_modules/**', 'src-tauri/**'],
    },
    eslint.configs.recommended,
    ...tseslint.configs.strictTypeChecked,
    ...tseslint.configs.stylisticTypeChecked,
    ...svelte.configs['flat/recommended'],
    {
        languageOptions: {
            globals: {
                ...globals.browser,
                ...globals.es2022,
            },
            parserOptions: {
                extraFileExtensions: ['.svelte'],
                parser: tseslint.parser,
                projectService: {
                    allowDefaultProject: ['eslint.config.js', 'svelte.config.js'],
                },
            },
        },
        rules: {
            '@typescript-eslint/consistent-type-imports': [
                'error',
                { fixStyle: 'inline-type-imports' },
            ],
            '@typescript-eslint/no-confusing-void-expression': 'off',
            '@typescript-eslint/no-misused-promises': [
                'error',
                { checksVoidReturn: { arguments: false, attributes: false } },
            ],
        },
    },
    {
        /*
         * Korean literals must live in `src/lib/i18n/ko.ts`, not inline.
         *
         * `files` shrinks as each feature area is migrated; a file leaves this
         * list only when it holds no inline Korean at all. That makes migration
         * progress measurable with `npm run lint` and stops a migrated file
         * from regressing.
         */
        files: ['src/**/*.ts', 'src/**/*.svelte'],
        ignores: [
            'src/lib/i18n/**',
            // Preview fixtures are sample content, not shipped UI copy.
            'src/preview/**',
            'src/app/App.test.ts',
            'src/app/app-controller.providers.test.ts',
            'src/app/app-controller.test.ts',
            'src/features/assets/TrustedAsset.test.ts',
            'src/features/chat/ChatPane.svelte',
            'src/features/chat/ChatPane.test.ts',
            'src/features/chat/GenerationAttemptApprovals.test.ts',
            'src/features/chat/chat-stream.test.ts',
            'src/features/chat/composer.test.ts',
            'src/features/chat/generation-attempt-approval-controller.test.ts',
            'src/features/chat/interaction-room-controller.test.ts',
            'src/features/chat/markdown.test.ts',
            'src/features/chat/markdown.ts',
            'src/features/conversations/ConversationPane.test.ts',
            'src/features/library/LibraryPane.test.ts',
            'src/features/orchestration/ContentModuleLifecyclePanel.svelte',
            'src/features/orchestration/ContentModuleLifecyclePanel.test.ts',
            'src/features/orchestration/CreatorDocumentEditors.svelte',
            'src/features/orchestration/CreatorDocumentEditors.test.ts',
            'src/features/orchestration/OrchestrationStudio.svelte',
            'src/features/orchestration/OrchestrationUI.test.ts',
            'src/features/orchestration/PromptPresetHistory.svelte',
            'src/features/orchestration/PromptPresetHistory.test.ts',
            'src/features/orchestration/content-package-controller.test.ts',
            'src/features/orchestration/creator-document-controller.test.ts',
            'src/features/orchestration/module-lifecycle-controller.test.ts',
            'src/features/orchestration/orchestration-controller.test.ts',
            'src/features/orchestration/prompt-preset-history-controller.test.ts',
            'src/features/personas/PersonaPanel.test.ts',
            'src/features/personas/persona-controller.test.ts',
            'src/features/providers/CapabilityPanel.svelte',
            'src/features/providers/DiscoveryPanel.svelte',
            'src/features/providers/ProviderConfiguration.test.ts',
            'src/features/providers/ProviderCrudPanel.svelte',
            'src/features/providers/ProviderSettings.svelte',
            'src/features/providers/ProviderSettings.test.ts',
            'src/features/providers/ProviderWorkflows.test.ts',
            'src/lib/i18n/ko.ts',
            'src/lib/ipc/client.test.ts',
        ],
        rules: {
            'no-restricted-syntax': [
                'error',
                {
                    selector: 'Literal[value=/[\\uAC00-\\uD7A3]/]',
                    message:
                        'Move this Korean text into src/lib/i18n/ko.ts and read it with t()/$tr().',
                },
                {
                    selector: 'TemplateElement[value.raw=/[\\uAC00-\\uD7A3]/]',
                    message:
                        'Move this Korean text into src/lib/i18n/ko.ts and read it with t()/$tr().',
                },
            ],
        },
    },
    prettier,
);
