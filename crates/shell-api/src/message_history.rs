//! Bounded branch history; full-list compatibility methods remain available.
use lorepia_core::{ConversationBranchId, CoreError, MessageId, MessagePresentationPage};
use serde::{Deserialize, Serialize};

use crate::{MessageDto, ShellApi, ShellError, ShellResult, api::validate_identifier};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListBranchMessagesPageInput {
    pub branch_id: String,
    pub before_message_id: Option<String>,
    pub after_message_id: Option<String>,
    pub limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_message_ids: Option<Vec<String>>,
    #[serde(default)]
    pub include_last_assistant: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchMessagesPageDto {
    pub messages: Vec<MessageDto>,
    pub has_older: bool,
    pub has_newer: bool,
    pub head_message_id: Option<String>,
    pub total_messages: u64,
    pub start_index: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained_message_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Supplemental latest branch assistant when absent from `messages`.
    pub last_assistant_message: Option<MessageDto>,
}

impl From<MessagePresentationPage> for BranchMessagesPageDto {
    fn from(value: MessagePresentationPage) -> Self {
        Self {
            messages: value.messages.into_iter().map(Into::into).collect(),
            last_assistant_message: value.last_assistant_message.map(Into::into),
            has_older: value.has_older,
            has_newer: value.has_newer,
            head_message_id: value.head_message_id.map(|id| id.0),
            total_messages: value.total_messages,
            start_index: value.start_index,
            retained_message_ids: value
                .retained_message_ids
                .map(|ids| ids.into_iter().map(|id| id.0).collect()),
        }
    }
}

impl ShellApi {
    pub fn list_branch_messages_page(
        &self,
        input: ListBranchMessagesPageInput,
    ) -> ShellResult<BranchMessagesPageDto> {
        validate_identifier("branch_id", &input.branch_id)?;
        for (field, value) in [
            ("before_message_id", input.before_message_id.as_deref()),
            ("after_message_id", input.after_message_id.as_deref()),
        ] {
            if let Some(value) = value {
                validate_identifier(field, value)?;
            }
        }
        if !(1..=128).contains(&input.limit)
            || (input.before_message_id.is_some() && input.after_message_id.is_some())
        {
            return Err(ShellError::from(CoreError::invalid(
                "message page requires one direction and a limit from 1 to 128",
            )));
        }
        if let Some(ids) = &input.check_message_ids {
            if ids.len() > 256 {
                return Err(ShellError::from(CoreError::invalid(
                    "message membership check exceeds 256 candidates",
                )));
            }
            for id in ids {
                validate_identifier("check_message_ids", id)?;
            }
        }
        let check = input
            .check_message_ids
            .map(|ids| ids.into_iter().map(MessageId).collect::<Vec<_>>());
        let before = input.before_message_id.map(MessageId);
        let after = input.after_message_id.map(MessageId);
        self.core
            .list_branch_message_presentations_page(
                &ConversationBranchId(input.branch_id),
                before.as_ref(),
                after.as_ref(),
                input.limit,
                check.as_deref(),
                input.include_last_assistant,
            )
            .map(Into::into)
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests;
