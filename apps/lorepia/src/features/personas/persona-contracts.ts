/** Safe webview contract for local personas and conversation-scoped selection. */

export interface PersonaDocumentDto {
    id: string;
    name: string;
    description: string;
}

export interface PersonaDto {
    value: PersonaDocumentDto;
    revision: number;
    revision_id: string;
    created_at: string;
    updated_at: string;
}

export interface PersonaPageCursorDto {
    catalog_revision: string;
    updated_at: string;
    persona_id: string;
}

export type PersonaListPageDto =
    | {
          kind: 'page';
          catalog_revision: string;
          items: PersonaDto[];
          next_cursor: PersonaPageCursorDto | null;
      }
    | {
          kind: 'restart_required';
          current_catalog_revision: string;
      };

export interface SelectedPersonaSnapshotDto {
    value: PersonaDocumentDto;
    revision: number;
    revision_id: string;
    snapshot_created_at: string;
}

export interface ConversationPersonaSelectionDto {
    conversation_id: string;
    state_revision: number | null;
    selected_persona: SelectedPersonaSnapshotDto | null;
    updated_at: string | null;
    cleared_at: string | null;
}

export interface PersonaDeletionReceiptDto {
    persona_id: string;
    revision: number;
    deleted_at: string;
}

export interface CreatePersonaInput {
    name: string;
    description: string;
}

export interface UpdatePersonaInput {
    persona_id: string;
    expected_revision: number;
    name: string;
    description: string;
}

export interface GetPersonaInput {
    persona_id: string;
}

export interface ListPersonasInput {
    limit: number;
}

export interface ListPersonaPageInput {
    limit: number;
    after: PersonaPageCursorDto | null;
}

export interface DeletePersonaInput {
    persona_id: string;
    expected_revision: number;
}

export interface GetConversationPersonaSelectionInput {
    conversation_id: string;
}

export interface SelectConversationPersonaInput {
    conversation_id: string;
    persona_id: string;
    expected_state_revision: number | null;
}

export interface ClearConversationPersonaInput {
    conversation_id: string;
    expected_state_revision: number;
}

export interface PersonaClientApi {
    createPersona(input: CreatePersonaInput): Promise<PersonaDto>;
    updatePersona(input: UpdatePersonaInput): Promise<PersonaDto>;
    getPersona(input: GetPersonaInput): Promise<PersonaDto>;
    listPersonas(input: ListPersonasInput): Promise<PersonaDto[]>;
    listPersonaPage(input: ListPersonaPageInput): Promise<PersonaListPageDto>;
    deletePersona(input: DeletePersonaInput): Promise<PersonaDeletionReceiptDto>;
    getConversationPersonaSelection(
        input: GetConversationPersonaSelectionInput,
    ): Promise<ConversationPersonaSelectionDto>;
    selectConversationPersona(
        input: SelectConversationPersonaInput,
    ): Promise<ConversationPersonaSelectionDto>;
    clearConversationPersona(
        input: ClearConversationPersonaInput,
    ): Promise<ConversationPersonaSelectionDto>;
}
