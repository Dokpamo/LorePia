//! Application-owned projections for bounded creator and memory catalogs.
use crate::revision::project_revisions;
use crate::{
    ContentModule, ConversationBranchId, ConversationId, Core, CoreResult, InteractionRuleSet,
    KnowledgeBook, MemoryProfile, MemoryRecord, Revisioned, TransformSet,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatorDocumentKind {
    MemoryProfile,
    KnowledgeBook,
    TransformSet,
    InteractionRuleSet,
    ContentModule,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadPageCursor {
    pub scope: String,
    pub after_id: String,
    /// Creator pages retain the observed timestamp even if the anchor is edited or deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}
impl From<CreatorDocumentKind> for lorepia_storage::CreatorDocumentKind {
    fn from(value: CreatorDocumentKind) -> Self {
        match value {
            CreatorDocumentKind::MemoryProfile => Self::MemoryProfile,
            CreatorDocumentKind::KnowledgeBook => Self::KnowledgeBook,
            CreatorDocumentKind::TransformSet => Self::TransformSet,
            CreatorDocumentKind::InteractionRuleSet => Self::InteractionRuleSet,
            CreatorDocumentKind::ContentModule => Self::ContentModule,
        }
    }
}
impl From<&ReadPageCursor> for lorepia_storage::ReadPageCursor {
    fn from(value: &ReadPageCursor) -> Self {
        Self {
            scope: value.scope.clone(),
            after_id: value.after_id.clone(),
            after_updated_at: value.after_updated_at,
        }
    }
}

pub enum CreatorDocument {
    MemoryProfile(Revisioned<MemoryProfile>),
    KnowledgeBook(Revisioned<KnowledgeBook>),
    TransformSet(Revisioned<TransformSet>),
    InteractionRuleSet(Revisioned<InteractionRuleSet>),
    ContentModule(Box<Revisioned<ContentModule>>),
}
pub struct ReadPage<T> {
    pub items: Vec<T>,
    pub scope: String,
    pub has_more: bool,
}
impl Core {
    pub fn list_creator_documents_page(
        &self,
        kind: CreatorDocumentKind,
        after: Option<&ReadPageCursor>,
        limit: u32,
    ) -> CoreResult<ReadPage<CreatorDocument>> {
        let after = after.map(lorepia_storage::ReadPageCursor::from);
        macro_rules! page {
            ($variant:ident) => {{
                let page = self.storage().list_creator_documents_page(
                    kind.into(),
                    after.as_ref(),
                    limit,
                )?;
                Ok(ReadPage {
                    items: project_revisions(page.items)
                        .into_iter()
                        .map(|value| CreatorDocument::$variant(value.into()))
                        .collect(),
                    scope: page.scope,
                    has_more: page.has_more,
                })
            }};
        }
        match kind {
            CreatorDocumentKind::MemoryProfile => page!(MemoryProfile),
            CreatorDocumentKind::KnowledgeBook => page!(KnowledgeBook),
            CreatorDocumentKind::TransformSet => page!(TransformSet),
            CreatorDocumentKind::InteractionRuleSet => page!(InteractionRuleSet),
            CreatorDocumentKind::ContentModule => page!(ContentModule),
        }
    }
    pub fn list_memory_records_page(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        include_invalidated: bool,
        after: Option<&ReadPageCursor>,
        limit: u32,
    ) -> CoreResult<ReadPage<Revisioned<MemoryRecord>>> {
        let after = after.map(lorepia_storage::ReadPageCursor::from);
        let page = self.storage().list_memory_records_page(
            conversation_id,
            branch_id,
            include_invalidated,
            after.as_ref(),
            limit,
        )?;
        Ok(ReadPage {
            items: project_revisions(page.items),
            scope: page.scope,
            has_more: page.has_more,
        })
    }
}
