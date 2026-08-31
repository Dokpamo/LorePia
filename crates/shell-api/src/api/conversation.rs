use lorepia_core::{ConversationBranchId, ConversationId, ConversationMode, MessageId};

use crate::{
    ConversationBranchDto, ConversationDto, ConversationStateDto, CreateConversationBranchInput,
    CreateConversationInput, MessageDto, RemoveMessageInput, SelectConversationBranchInput,
    SetConversationModeInput, ShellError, ShellResult,
};

use super::{ShellApi, validate_chat_route, validate_identifier, validate_optional_identifier};

impl ShellApi {
    pub fn open_conversation(&self, character_id: &str) -> ShellResult<ConversationDto> {
        validate_identifier("character_id", character_id)?;
        self.core
            .open_conversation(character_id)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn create_conversation(
        &self,
        input: CreateConversationInput,
    ) -> ShellResult<ConversationDto> {
        validate_identifier("character_id", &input.character_id)?;
        if let Some(greeting) = input.greeting {
            validate_optional_identifier(
                "character_content_revision_id",
                greeting.character_content_revision_id.as_deref(),
            )?;
            validate_optional_identifier("greeting_id", greeting.greeting_id.as_deref())?;
            self.core
                .create_conversation_with_greeting(
                    &input.character_id,
                    input.title,
                    ConversationMode::from(input.mode),
                    greeting.character_content_revision_id.as_deref(),
                    greeting.greeting_id.as_deref(),
                )
                .map(|started| started.conversation.into())
                .map_err(ShellError::from)
        } else {
            self.core
                .create_conversation(
                    &input.character_id,
                    input.title,
                    ConversationMode::from(input.mode),
                )
                .map(Into::into)
                .map_err(ShellError::from)
        }
    }

    pub fn list_conversations(&self) -> ShellResult<Vec<ConversationDto>> {
        self.core
            .list_conversations()
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn list_conversations_for_character(
        &self,
        character_id: &str,
    ) -> ShellResult<Vec<ConversationDto>> {
        validate_identifier("character_id", character_id)?;
        self.core
            .list_conversations_for_character(character_id)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn get_conversation(&self, conversation_id: &str) -> ShellResult<ConversationDto> {
        validate_identifier("conversation_id", conversation_id)?;
        self.core
            .get_conversation(&ConversationId(conversation_id.to_owned()))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn open_existing_conversation(
        &self,
        conversation_id: &str,
    ) -> ShellResult<ConversationDto> {
        validate_identifier("conversation_id", conversation_id)?;
        self.core
            .open_existing_conversation(&ConversationId(conversation_id.to_owned()))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn get_conversation_state(
        &self,
        conversation_id: &str,
    ) -> ShellResult<ConversationStateDto> {
        validate_identifier("conversation_id", conversation_id)?;
        self.core
            .get_conversation_state(&ConversationId(conversation_id.to_owned()))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn list_conversation_branches(
        &self,
        conversation_id: &str,
    ) -> ShellResult<Vec<ConversationBranchDto>> {
        validate_identifier("conversation_id", conversation_id)?;
        self.core
            .list_conversation_branches(&ConversationId(conversation_id.to_owned()))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn create_conversation_branch(
        &self,
        input: CreateConversationBranchInput,
    ) -> ShellResult<ConversationBranchDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_optional_identifier("from_message_id", input.from_message_id.as_deref())?;
        let from_message_id = input.from_message_id.map(MessageId);
        self.core
            .create_conversation_branch(
                &ConversationId(input.conversation_id),
                from_message_id.as_ref(),
                input.title,
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn select_conversation_branch(
        &self,
        input: SelectConversationBranchInput,
    ) -> ShellResult<ConversationStateDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        self.core
            .select_conversation_branch(
                &ConversationId(input.conversation_id),
                &ConversationBranchId(input.branch_id),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn set_conversation_mode(
        &self,
        input: SetConversationModeInput,
    ) -> ShellResult<ConversationStateDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        self.core
            .set_conversation_mode(
                &ConversationId(input.conversation_id),
                ConversationMode::from(input.mode),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn list_branch_messages(&self, branch_id: &str) -> ShellResult<Vec<MessageDto>> {
        validate_identifier("branch_id", branch_id)?;
        self.core
            .list_branch_message_presentations(&ConversationBranchId(branch_id.to_owned()))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn list_messages(&self, conversation_id: &str) -> ShellResult<Vec<MessageDto>> {
        validate_identifier("conversation_id", conversation_id)?;
        self.core
            .list_message_presentations(&ConversationId(conversation_id.to_owned()))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn remove_message_from_branch(
        &self,
        input: RemoveMessageInput,
    ) -> ShellResult<ConversationBranchDto> {
        validate_chat_route(
            &input.conversation_id,
            &input.branch_id,
            input.expected_head.as_deref(),
        )?;
        validate_identifier("message_id", &input.message_id)?;
        let expected_head = input.expected_head.map(MessageId);
        self.core
            .remove_message_from_branch(
                &ConversationId(input.conversation_id),
                &ConversationBranchId(input.branch_id),
                expected_head.as_ref(),
                &MessageId(input.message_id),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }
}
