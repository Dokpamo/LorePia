mod bootstrap;
mod conversation;
mod generation;
mod library;

pub use bootstrap::{BootstrapDto, HealthDto};
pub use conversation::{
    ConversationBranchDto, ConversationDto, ConversationGreetingSelectionInput,
    ConversationModeDto, ConversationStateDto, CreateConversationBranchInput,
    CreateConversationInput, MessageDisplayProjectionDto, MessageDto, MessageRoleDto,
    MessageStatusDto, MessageTransformDiagnosticDto, MessageTransformDispositionDto,
    MessageTransformStageDto, RemoveMessageInput, SelectConversationBranchInput,
    SetConversationModeInput,
};
pub use generation::{
    ChatEventDto, ChatEventKindDto, EditUserMessageInput, GenerateRuntimeTextInput,
    GenerationSelectionInput, GenerationStartedDto, GenerationTargetDto, GenerationUsageDto,
    MessageActionGenerationDto, RegenerateAssistantMessageInput, RuntimeGenerationAuditInput,
    RuntimeGenerationCapabilityInput, RuntimePromptMessageInput, RuntimePromptRoleInput,
    RuntimeTextGenerationDto, SendMessageInput,
};
pub use library::{
    CharacterDisplayTransformDto, CharacterDto, CharacterGreetingCatalogDto,
    CharacterGreetingKindDto, CharacterGreetingOptionDto, CharacterRenderAssetDto,
    CharacterRenderProfileDto, CharacterRuntimeKnowledgeDto, CharacterRuntimeScriptDto,
    ContentKindDto, ImportImagePreviewDto, ImportInspectionDto, ImportWarningDto,
};

#[allow(unused_imports)]
pub use library::{ImportDynamicContentReviewDto, ImportRegexRuleReviewDto};

#[cfg(test)]
mod tests {
    include!("dto/tests.rs");
}
