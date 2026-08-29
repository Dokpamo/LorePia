export interface PortableRuntimeStateScopeInput {
    character_id: string;
    character_content_revision_id: string | null;
    conversation_id: string;
    branch_id: string;
}

export interface PortableRuntimeStatePayloadValueDto {
    options: Record<string, string>;
    chatVars: Record<string, unknown>;
    state: Record<string, unknown>;
    messageOverrides: Record<string, string>;
    background: string;
    auxiliarySelection: unknown;
}

export interface PortableRuntimeStatePayloadDto {
    schema_version: number;
    value: PortableRuntimeStatePayloadValueDto;
}

export interface PortableRuntimeStateRecordDto {
    scope: PortableRuntimeStateScopeInput;
    scope_epoch: number;
    revision: number;
    payload: PortableRuntimeStatePayloadDto;
    created_at: string;
    updated_at: string;
}

export interface GetPortableRuntimeStateInput {
    scope: PortableRuntimeStateScopeInput;
}

export interface GetPortableRuntimeStateDto {
    scope_epoch: number;
    record: PortableRuntimeStateRecordDto | null;
}

export interface PutPortableRuntimeStateInput {
    scope: PortableRuntimeStateScopeInput;
    expected_scope_epoch: number;
    expected_revision: number | null;
    payload: PortableRuntimeStatePayloadDto;
}

export type PutPortableRuntimeStateResultDto =
    | {
          status: 'saved';
          record: PortableRuntimeStateRecordDto;
          evicted_rows: number;
          evicted_bytes: number;
      }
    | {
          status: 'revision_conflict';
          current: PortableRuntimeStateRecordDto | null;
      }
    | {
          status: 'scope_invalidated';
          current_scope_epoch: number;
      };
