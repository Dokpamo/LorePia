export type ConversationMode = 'chat' | 'story';

export type MessageRole = 'system' | 'user' | 'assistant';

export type MessageStatus = 'pending' | 'complete' | 'cancelled' | 'failed';

export interface ConversationDto {
    id: string;
    character_id: string;
    title: string;
    created_at: string;
    updated_at: string;
}

export interface ConversationStateDto {
    conversation_id: string;
    active_branch_id: string;
    selected_mode: ConversationMode;
    updated_at: string;
}

export interface ConversationBranchDto {
    id: string;
    conversation_id: string;
    title: string | null;
    fork_message_id: string | null;
    head_message_id: string | null;
    created_at: string;
    updated_at: string;
}

export interface MessageDto {
    id: string;
    conversation_id: string;
    parent_id: string | null;
    role: MessageRole;
    content: string;
    status: MessageStatus;
    generation_id: string | null;
    created_at: string;
    /** Present only after Rust verified the immutable DisplayOnly sidecar. */
    display_projection?: MessageDisplayProjectionDto;
}

export type MessageTransformStage = 'provider_output_canonical' | 'display_only';

export type MessageTransformDisposition =
    | 'applied'
    | 'no_match'
    | 'disabled'
    | 'pending_import_approval'
    | 'resolved_prompt_disabled'
    | 'condition_false'
    | 'failed'
    | 'limit_rejected'
    | 'pipeline_rejected';

/** Content-free, generation-linked transform evidence safe for expert UI. */
export interface MessageTransformDiagnosticDto {
    set_revision_id: string | null;
    rule_id: string | null;
    stage: MessageTransformStage;
    disposition: MessageTransformDisposition;
    code: string | null;
    before_sha256: string;
    after_sha256: string | null;
    recorded_at: string;
}

export interface MessageDisplayProjectionDto {
    canonical_content_sha256: string;
    display_content_sha256: string;
    diagnostics_sha256: string;
    diagnostics: MessageTransformDiagnosticDto[];
}
