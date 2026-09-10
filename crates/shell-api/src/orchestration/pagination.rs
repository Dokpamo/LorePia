//! Bounded page projections and compatibility adapters for creator/memory lists.
use super::{
    ConversationBranchId, ConversationId, CreatorContentModuleDocumentDto,
    CreatorInteractionRuleSetDocumentDto, CreatorKnowledgeBookDocumentDto,
    CreatorMemoryProfileDocumentDto, CreatorTransformSetDocumentDto, Deserialize,
    MemoryRecordProjectionDto, RevisionedDto, Serialize, ShellApi, ShellError, ShellResult,
    shell_invalid, validate_document, validate_identifier,
};
use lorepia_core::{CreatorDocument, CreatorDocumentKind, ReadPageCursor};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListCreatorDocumentsPageInput {
    pub kind: CreatorDocumentKind,
    pub after: Option<ReadPageCursor>,
    pub limit: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListMemoryRecordsPageInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub include_invalidated: bool,
    pub after: Option<ReadPageCursor>,
    pub limit: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CreatorDocumentDto {
    MemoryProfile(RevisionedDto<CreatorMemoryProfileDocumentDto>),
    KnowledgeBook(RevisionedDto<CreatorKnowledgeBookDocumentDto>),
    TransformSet(RevisionedDto<CreatorTransformSetDocumentDto>),
    InteractionRuleSet(RevisionedDto<CreatorInteractionRuleSetDocumentDto>),
    ContentModule(RevisionedDto<CreatorContentModuleDocumentDto>),
}
impl CreatorDocumentDto {
    fn updated_at(&self) -> chrono::DateTime<chrono::Utc> {
        match self {
            Self::MemoryProfile(value) => value.updated_at,
            Self::KnowledgeBook(value) => value.updated_at,
            Self::TransformSet(value) => value.updated_at,
            Self::InteractionRuleSet(value) => value.updated_at,
            Self::ContentModule(value) => value.updated_at,
        }
    }
    fn id(&self) -> &str {
        match self {
            Self::MemoryProfile(value) => &value.value.id,
            Self::KnowledgeBook(value) => &value.value.id,
            Self::TransformSet(value) => &value.value.id,
            Self::InteractionRuleSet(value) => &value.value.id,
            Self::ContentModule(value) => &value.value.id,
        }
    }
}
impl TryFrom<CreatorDocument> for CreatorDocumentDto {
    type Error = ShellError;
    fn try_from(value: CreatorDocument) -> ShellResult<Self> {
        Ok(match value {
            CreatorDocument::MemoryProfile(v) => {
                Self::MemoryProfile(RevisionedDto::from(v).try_project(TryInto::try_into)?)
            }
            CreatorDocument::KnowledgeBook(v) => {
                Self::KnowledgeBook(RevisionedDto::from(v).try_project(TryInto::try_into)?)
            }
            CreatorDocument::TransformSet(v) => {
                Self::TransformSet(RevisionedDto::from(v).try_project(TryInto::try_into)?)
            }
            CreatorDocument::InteractionRuleSet(v) => {
                Self::InteractionRuleSet(RevisionedDto::from(v).try_project(TryInto::try_into)?)
            }
            CreatorDocument::ContentModule(v) => {
                Self::ContentModule(RevisionedDto::from(*v).try_project(TryInto::try_into)?)
            }
        })
    }
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreatorDocumentsPageDto {
    pub kind: CreatorDocumentKind,
    pub documents: Vec<CreatorDocumentDto>,
    pub next_cursor: Option<ReadPageCursor>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecordsPageDto {
    pub records: Vec<MemoryRecordProjectionDto>,
    pub next_cursor: Option<ReadPageCursor>,
}

impl ShellApi {
    pub fn list_creator_documents_page(
        &self,
        input: ListCreatorDocumentsPageInput,
    ) -> ShellResult<CreatorDocumentsPageDto> {
        let page = self
            .core
            .list_creator_documents_page(input.kind, input.after.as_ref(), input.limit)
            .map_err(ShellError::from)?;
        let mut result = CreatorDocumentsPageDto {
            kind: input.kind,
            documents: page
                .items
                .into_iter()
                .map(TryInto::try_into)
                .collect::<ShellResult<Vec<_>>>()?,
            next_cursor: None,
        };
        let mut more = page.has_more;
        loop {
            result.next_cursor = if more {
                result.documents.last().map(|value| ReadPageCursor {
                    scope: page.scope.clone(),
                    after_id: value.id().to_owned(),
                    after_updated_at: Some(value.updated_at()),
                })
            } else {
                None
            };
            match validate_document(&result) {
                Ok(()) => return Ok(result),
                Err(_) if result.documents.len() > 1 => {
                    result.documents.pop();
                    more = true;
                }
                Err(error) => return Err(error),
            }
        }
    }
    pub fn list_memory_records_page(
        &self,
        input: ListMemoryRecordsPageInput,
    ) -> ShellResult<MemoryRecordsPageDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        let page = self
            .core
            .list_memory_records_page(
                &ConversationId(input.conversation_id),
                &ConversationBranchId(input.branch_id),
                input.include_invalidated,
                input.after.as_ref(),
                input.limit,
            )
            .map_err(ShellError::from)?;
        let mut result = MemoryRecordsPageDto {
            records: page
                .items
                .into_iter()
                .map(RevisionedDto::from)
                .map(Into::into)
                .collect(),
            next_cursor: None,
        };
        let mut more = page.has_more;
        loop {
            result.next_cursor = if more {
                result.records.last().map(|value| ReadPageCursor {
                    scope: page.scope.clone(),
                    after_id: value.id.clone(),
                    after_updated_at: None,
                })
            } else {
                None
            };
            match validate_document(&result) {
                Ok(()) => return Ok(result),
                Err(_) if result.records.len() > 1 => {
                    result.records.pop();
                    more = true;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

// Compatibility endpoints keep their response shapes and read only the first SQL page.
impl ShellApi {
    pub fn list_memory_profiles(
        &self,
    ) -> ShellResult<Vec<RevisionedDto<CreatorMemoryProfileDocumentDto>>> {
        self.list_creator_documents_page(ListCreatorDocumentsPageInput {
            kind: CreatorDocumentKind::MemoryProfile,
            after: None,
            limit: 100,
        })?
        .documents
        .into_iter()
        .map(|document| match document {
            CreatorDocumentDto::MemoryProfile(value) => Ok(value),
            _ => Err(shell_invalid("creator page kind mismatch")),
        })
        .collect()
    }
    pub fn list_knowledge_books(
        &self,
    ) -> ShellResult<Vec<RevisionedDto<CreatorKnowledgeBookDocumentDto>>> {
        self.list_creator_documents_page(ListCreatorDocumentsPageInput {
            kind: CreatorDocumentKind::KnowledgeBook,
            after: None,
            limit: 100,
        })?
        .documents
        .into_iter()
        .map(|document| match document {
            CreatorDocumentDto::KnowledgeBook(value) => Ok(value),
            _ => Err(shell_invalid("creator page kind mismatch")),
        })
        .collect()
    }
    pub fn list_transform_sets(
        &self,
    ) -> ShellResult<Vec<RevisionedDto<CreatorTransformSetDocumentDto>>> {
        self.list_creator_documents_page(ListCreatorDocumentsPageInput {
            kind: CreatorDocumentKind::TransformSet,
            after: None,
            limit: 100,
        })?
        .documents
        .into_iter()
        .map(|document| match document {
            CreatorDocumentDto::TransformSet(value) => Ok(value),
            _ => Err(shell_invalid("creator page kind mismatch")),
        })
        .collect()
    }
    pub fn list_interaction_rule_sets(
        &self,
    ) -> ShellResult<Vec<RevisionedDto<CreatorInteractionRuleSetDocumentDto>>> {
        self.list_creator_documents_page(ListCreatorDocumentsPageInput {
            kind: CreatorDocumentKind::InteractionRuleSet,
            after: None,
            limit: 100,
        })?
        .documents
        .into_iter()
        .map(|document| match document {
            CreatorDocumentDto::InteractionRuleSet(value) => Ok(value),
            _ => Err(shell_invalid("creator page kind mismatch")),
        })
        .collect()
    }
    pub fn list_content_modules(
        &self,
    ) -> ShellResult<Vec<RevisionedDto<CreatorContentModuleDocumentDto>>> {
        self.list_creator_documents_page(ListCreatorDocumentsPageInput {
            kind: CreatorDocumentKind::ContentModule,
            after: None,
            limit: 100,
        })?
        .documents
        .into_iter()
        .map(|document| match document {
            CreatorDocumentDto::ContentModule(value) => Ok(value),
            _ => Err(shell_invalid("creator page kind mismatch")),
        })
        .collect()
    }
}
#[cfg(test)]
mod tests;
