export const SUPPORTED_SHELL_API_VERSION = 3;

export const SUPPORTED_CORE_API_VERSION = 10;

export const SUPPORTED_CHAT_EVENT_VERSION = 4;

export type LoadingPhase = 'idle' | 'loading' | 'ready' | 'error';

export interface FieldErrorDto {
    field: string;
    message_key: string;
}

export interface ShellErrorDto {
    code: string;
    message_key: string;
    recoverable: boolean;
    operation_id: string | null;
    field_errors: FieldErrorDto[];
}

export type PromptBlockRoleHint =
    'system' | 'developer' | 'user' | 'assistant' | 'provider_default';

export type PromptBlockOverflowPolicy =
    | 'reject'
    | 'drop_block'
    | 'trim_head'
    | 'trim_tail'
    | 'keep_latest_items'
    | 'summarize'
    | 'reduce_knowledge_entries';

export type PromptBlockKind =
    | 'static_instruction'
    | 'character_identity'
    | 'character_description'
    | 'character_personality'
    | 'scenario'
    | 'user_persona'
    | 'dialogue_examples'
    | 'world_knowledge'
    | 'retrieved_memory'
    | 'conversation_summary'
    | 'history_slice'
    | 'latest_user_turn'
    | 'author_note'
    | 'post_history_instruction'
    | 'assistant_prefill'
    | 'group_context';

export type OrchestrationConditionExprDto =
    | { op: 'true' }
    | { op: 'false' }
    | {
          op: 'equals' | 'not_equals';
          variable: OrchestrationVariableRefDto;
          value: OrchestrationVariableValueDto;
      }
    | { op: 'greater_than'; variable: OrchestrationVariableRefDto; value: number }
    | { op: 'contains'; variable: OrchestrationVariableRefDto; value: string }
    | { op: 'exists'; variable: OrchestrationVariableRefDto }
    | { op: 'model_supports'; capability: string }
    | { op: 'all' | 'any'; expressions: OrchestrationConditionExprDto[] }
    | { op: 'not'; expression: OrchestrationConditionExprDto };

export interface SafePromptTemplateDto {
    parts: SafePromptTemplatePartDto[];
    max_output_chars: number;
}

export type SafePromptTemplatePartDto =
    | { kind: 'text'; value: string }
    | { kind: 'variable'; variable: OrchestrationVariableRefDto }
    | {
          kind: 'built_in';
          value:
              | 'character_name'
              | 'user_name'
              | 'persona_name'
              | 'persona_description'
              | 'current_date'
              | 'current_time';
      }
    | { kind: 'slot'; name: string }
    | { kind: 'join'; variable: OrchestrationVariableRefDto; separator: string }
    | {
          kind: 'conditional';
          condition: OrchestrationConditionExprDto;
          then_template: SafePromptTemplateDto;
          else_template: SafePromptTemplateDto | null;
      };

export interface PromptTokenPolicyDto {
    priority: number;
    min_tokens: number | null;
    max_tokens: number | null;
    reserve_tokens: number | null;
}

export type CreatorControlValue = boolean | number | string | string[];

export interface SafeRegexDto {
    pattern: string;
    case_insensitive: boolean;
}

export type OrchestrationModuleScope =
    'app' | 'user' | 'persona' | 'character' | 'conversation' | 'branch';

export type OrchestrationVariableScope =
    | 'app'
    | 'user'
    | 'persona'
    | 'character'
    | 'conversation'
    | 'branch'
    | 'session'
    | 'turn'
    | 'module';

export type OrchestrationVariableValueDto =
    | { type: 'bool'; value: boolean }
    | { type: 'integer'; value: number }
    | { type: 'decimal'; value: number }
    | { type: 'text'; value: string }
    | { type: 'enum'; value: string }
    | { type: 'string_list'; value: string[] };

export interface OrchestrationVariableRefDto {
    scope: OrchestrationVariableScope;
    namespace: string | null;
    id: string;
}

export interface OrchestrationVariableMapDto {
    values: {
        variable: OrchestrationVariableRefDto;
        value: OrchestrationVariableValueDto;
    }[];
}

export interface RevisionedDto<Value> {
    value: Value;
    revision: number;
    created_at: string;
    updated_at: string;
    deleted_at: string | null;
}

export type JsonValue =
    null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
