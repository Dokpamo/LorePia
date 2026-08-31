import type {
    ClearConversationPersonaInput,
    ConversationPersonaSelectionDto,
    CreatePersonaInput,
    DeletePersonaInput,
    GetConversationPersonaSelectionInput,
    GetPersonaInput,
    ListPersonaPageInput,
    ListPersonasInput,
    PersonaDeletionReceiptDto,
    PersonaDto,
    PersonaListPageDto,
    SelectConversationPersonaInput,
    UpdatePersonaInput,
} from '../../../features/personas/persona-contracts';

import type {
    AssetDeliveryDto,
    ResolveAssetDeliveryInput,
    MemorySupervisorStatusDto,
    BootstrapDto,
    CharacterDto,
    CharacterGreetingCatalogDto,
    CharacterRenderProfileDto,
} from '../contracts';

import { isMemorySupervisorStatus } from '../client-payload-guards';

import { LOREPIA_COMMANDS, LOREPIA_EVENTS } from '../commands';

import { ClientTransportBase } from './transport';

export abstract class LibraryClient extends ClientTransportBase {
    bootstrapSnapshot(): Promise<BootstrapDto> {
        return this.call(LOREPIA_COMMANDS.bootstrap);
    }

    getMemorySupervisorStatus(): Promise<MemorySupervisorStatusDto> {
        return this.call(LOREPIA_COMMANDS.getMemorySupervisorStatus);
    }

    subscribeMemorySupervisorStatus(
        onStatus: (status: MemorySupervisorStatusDto) => void,
    ): Promise<() => void> {
        return this.transport.listen(LOREPIA_EVENTS.memorySupervisorStatus, (payload) => {
            if (isMemorySupervisorStatus(payload)) onStatus(payload);
        });
    }

    listCharacters(): Promise<CharacterDto[]> {
        return this.call(LOREPIA_COMMANDS.listCharacters);
    }

    getCharacter(characterId: string): Promise<CharacterDto> {
        return this.call(LOREPIA_COMMANDS.getCharacter, {
            request: { character_id: characterId },
        });
    }

    getCharacterGreetingCatalog(characterId: string): Promise<CharacterGreetingCatalogDto> {
        return this.call(LOREPIA_COMMANDS.getCharacterGreetingCatalog, {
            request: { character_id: characterId },
        });
    }

    getCharacterRenderProfile(characterId: string): Promise<CharacterRenderProfileDto> {
        return this.call(LOREPIA_COMMANDS.getCharacterRenderProfile, {
            request: { character_id: characterId },
        });
    }

    createPersona(input: CreatePersonaInput): Promise<PersonaDto> {
        return this.call(LOREPIA_COMMANDS.createPersona, { request: input });
    }

    updatePersona(input: UpdatePersonaInput): Promise<PersonaDto> {
        return this.call(LOREPIA_COMMANDS.updatePersona, { request: input });
    }

    getPersona(input: GetPersonaInput): Promise<PersonaDto> {
        return this.call(LOREPIA_COMMANDS.getPersona, { request: input });
    }

    listPersonas(input: ListPersonasInput): Promise<PersonaDto[]> {
        return this.call(LOREPIA_COMMANDS.listPersonas, { request: input });
    }

    listPersonaPage(input: ListPersonaPageInput): Promise<PersonaListPageDto> {
        return this.call(LOREPIA_COMMANDS.listPersonaPage, { request: input });
    }

    deletePersona(input: DeletePersonaInput): Promise<PersonaDeletionReceiptDto> {
        return this.call(LOREPIA_COMMANDS.deletePersona, { request: input });
    }

    getConversationPersonaSelection(
        input: GetConversationPersonaSelectionInput,
    ): Promise<ConversationPersonaSelectionDto> {
        return this.call(LOREPIA_COMMANDS.getConversationPersonaSelection, { request: input });
    }

    selectConversationPersona(
        input: SelectConversationPersonaInput,
    ): Promise<ConversationPersonaSelectionDto> {
        return this.call(LOREPIA_COMMANDS.selectConversationPersona, { request: input });
    }

    clearConversationPersona(
        input: ClearConversationPersonaInput,
    ): Promise<ConversationPersonaSelectionDto> {
        return this.call(LOREPIA_COMMANDS.clearConversationPersona, { request: input });
    }

    resolveAssetDelivery(input: ResolveAssetDeliveryInput): Promise<AssetDeliveryDto> {
        return this.call(LOREPIA_COMMANDS.resolveAssetDelivery, { request: input });
    }
}
