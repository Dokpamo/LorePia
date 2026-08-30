use std::{
    io::{Read, Write},
    net::{IpAddr, TcpListener, TcpStream},
    path::Path,
    process::Command,
    sync::{Arc, Barrier, mpsc as std_mpsc},
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use lorepia_domain::{
    ActivationRule, AuxiliaryTaskKind, BlockSource, BuiltInTemplateValue, ConnectionConfigEntry,
    ConnectionConfigValue, ContentCapability, ContentModule, ContentModuleId, ControlId,
    ControlKind, ControlSpec, DiceExpression, GenerationPromptCacheMode,
    GenerationPromptCacheSettings, GenerationPromptCacheTtl, GenerationReasoningEffort,
    GenerationReasoningMode, GenerationReasoningSettings, GenerationReasoningSummary,
    GenerationUsage, HistorySelector, InstructionAuthority, InteractionAction, InteractionEffect,
    InteractionEvent, InteractionProposalDecision, InteractionProposalStatus, InteractionRule,
    InteractionRuleId, InteractionRuleSet, InteractionRuleSetId, KnowledgeBook, KnowledgeBookId,
    KnowledgeEntry, KnowledgeEntryId, KnowledgePlacement, MemoryKind, MemoryProfile,
    MemoryProfileId, MemoryRecord, MemoryRecordId, MergePolicy, ModelSyncState, ModuleBindingId,
    ModuleRevisionResolutionMode, ModuleScope, OpenRouterReasoningDetail,
    OpenRouterReasoningTopology, OverflowPolicy, PackageMetadata, PlacementZone, PresetMetadata,
    PromptBlock, PromptBlockId, PromptBlockKind, PromptContextSnapshotV1, PromptPreset,
    ProposalSpec, Provenance, ProviderCapabilities, RateLimit, ResolvedPromptPlan, RoleHint,
    SafeRegex, SafeTemplate, SourceKind, SummarySchemaId, TaskProfile, TaskProfileId, TemplatePart,
    TemplateSlot, TokenBudget, TokenPolicy, TransformRule, TransformRuleId, TransformSetId,
    ValueExpr, VariableId, VariableRef, VariableScope, VariableType, VariableValue, VersionedJson,
};
use lorepia_providers::{EmbeddingPurpose, ProviderEvent, ProviderEventSender, StaticProvider};
use lorepia_storage::{
    GenerationAttemptStatus, KnowledgeEmbeddingWrite, LifecycleOccurrenceKind,
    MemoryQueryEmbeddingIntent, PromptPresetBinding, PromptResponseLength,
    ProviderCredentialObservedStatus, ProviderCredentialOperationKind,
};
use tempfile::{NamedTempFile, TempDir, tempdir};

use super::*;
use crate::{
    ContentModuleActivationRequest, ContentModuleBindingDraft, ContentModuleRuntimeTarget,
    MessagePresentation, ModuleActivationApproval, ModuleMergeResolutionSet,
};

include!("credential_identity.rs");
include!("credential_lifetime.rs");
include!("legacy_profiles.rs");
include!("provider_doubles.rs");
include!("generation_fixtures.rs");
include!("provider_catalog.rs");
include!("provider_contracts.rs");
include!("generation_lifecycle.rs");
include!("content_import_validation.rs");
include!("conversation_branching.rs");
include!("async_reviewed_generation.rs");
include!("generation_output.rs");
include!("semantic_prompt.rs");
include!("generation_events.rs");
