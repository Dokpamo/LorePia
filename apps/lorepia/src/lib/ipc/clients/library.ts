import type {
    CharacterGreetingDetailDto,
    CharacterGreetingDetailInput,
} from '../contracts/character';
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
    StorageOverviewDto,
    CharacterDto,
    CharacterGreetingCatalogDto,
    CharacterRenderProfileDto,
    CharacterRenderProfileScopeInput,
} from '../contracts';

import { isMemorySupervisorStatus } from '../client-payload-guards';

import { LOREPIA_COMMANDS, LOREPIA_EVENTS } from '../commands';

import { ClientTransportBase } from './transport';

export abstract class LibraryClient extends ClientTransportBase {
    getStorageOverview(): Promise<StorageOverviewDto> {
        return this.call(LOREPIA_COMMANDS.getStorageOverview);
    }
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

    getCharacterGreetingDetail(
        input: CharacterGreetingDetailInput,
    ): Promise<CharacterGreetingDetailDto> {
        return this.call(LOREPIA_COMMANDS.getCharacterGreetingDetail, { request: input });
    }

    getCharacterRenderProfile(
        characterId: string,
        scope?: CharacterRenderProfileScopeInput,
    ): Promise<CharacterRenderProfileDto> {
        return this.call(LOREPIA_COMMANDS.getCharacterRenderProfile, {
            request: { character_id: characterId, ...scope },
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
