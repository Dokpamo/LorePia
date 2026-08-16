use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use lorepia_domain::{
    BuiltInTemplateValue, CapabilityKey, ConditionExpr, ContentModuleId, DiceExpression,
    InteractionAction, InteractionEffect, InteractionEvent, InteractionProposalDecision,
    InteractionProposalRecord, InteractionProposalRecordId, InteractionProposalStatus,
    InteractionRule, InteractionRuleId, InteractionRuleSet, InteractionRuleSetId, InteractionState,
    MAX_INTERACTION_NATIVE_TEXT_CHARS, MAX_INTERACTION_PROPOSAL_BODY_CHARS,
    MAX_INTERACTION_PROPOSAL_TITLE_CHARS, SafeTemplate, SourceKind, TemplatePart, ValueExpr,
    VariableMap, VariableRef, VariableScope, VariableValue, validate_interaction_native_text,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_MAX_INTERACTION_RULES: usize = 256;
pub const DEFAULT_MAX_INTERACTION_RULE_SETS: usize = 64;
pub const DEFAULT_MAX_ACTIONS_PER_EVENT: usize = 256;
pub const DEFAULT_MAX_ACTIONS_PER_RULE: usize = 64;
pub const DEFAULT_MAX_CONDITION_DEPTH: usize = 16;
pub const DEFAULT_MAX_CONDITION_NODES: usize = 512;
pub const DEFAULT_MAX_TEMPLATE_DEPTH: usize = 16;
pub const DEFAULT_MAX_TEMPLATE_PARTS: usize = 512;
pub const DEFAULT_MAX_VARIABLES: usize = lorepia_domain::MAX_VARIABLES;
pub const DEFAULT_MAX_INTERACTION_PROPOSALS: usize = lorepia_domain::MAX_INTERACTION_PROPOSALS;
pub const DEFAULT_MAX_PENDING_INTERACTION_PROPOSALS: usize = 64;
pub const DEFAULT_MAX_EFFECTS: usize = 512;
pub const DEFAULT_MAX_CHOICES: usize = 64;
pub const DEFAULT_MAX_DICE_COUNT: u16 = 100;
pub const DEFAULT_MAX_DICE_SIDES: u32 = 10_000;
pub const DEFAULT_MAX_TEXT_CHARS: usize = MAX_INTERACTION_NATIVE_TEXT_CHARS;
pub const DEFAULT_MAX_IDENTIFIER_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionLimits {
    pub max_rule_sets: usize,
    pub max_rules: usize,
    pub max_actions_per_event: usize,
    pub max_actions_per_rule: usize,
    pub max_condition_depth: usize,
    pub max_condition_nodes: usize,
    pub max_template_depth: usize,
    pub max_template_parts: usize,
    pub max_variables: usize,
    pub max_proposals: usize,
    pub max_pending_proposals: usize,
    pub max_effects: usize,
    pub max_choices: usize,
    pub max_dice_count: u16,
    pub max_dice_sides: u32,
    pub max_text_chars: usize,
    pub max_identifier_bytes: usize,
}

impl Default for InteractionLimits {
    fn default() -> Self {
        Self {
            max_rule_sets: DEFAULT_MAX_INTERACTION_RULE_SETS,
            max_rules: DEFAULT_MAX_INTERACTION_RULES,
            max_actions_per_event: DEFAULT_MAX_ACTIONS_PER_EVENT,
            max_actions_per_rule: DEFAULT_MAX_ACTIONS_PER_RULE,
            max_condition_depth: DEFAULT_MAX_CONDITION_DEPTH,
            max_condition_nodes: DEFAULT_MAX_CONDITION_NODES,
            max_template_depth: DEFAULT_MAX_TEMPLATE_DEPTH,
            max_template_parts: DEFAULT_MAX_TEMPLATE_PARTS,
            max_variables: DEFAULT_MAX_VARIABLES,
            max_proposals: DEFAULT_MAX_INTERACTION_PROPOSALS,
            max_pending_proposals: DEFAULT_MAX_PENDING_INTERACTION_PROPOSALS,
            max_effects: DEFAULT_MAX_EFFECTS,
            max_choices: DEFAULT_MAX_CHOICES,
            max_dice_count: DEFAULT_MAX_DICE_COUNT,
            max_dice_sides: DEFAULT_MAX_DICE_SIDES,
            max_text_chars: DEFAULT_MAX_TEXT_CHARS,
            max_identifier_bytes: DEFAULT_MAX_IDENTIFIER_BYTES,
        }
    }
}

impl InteractionLimits {
    fn validate(self) -> Result<Self, InteractionFailure> {
        if self.max_rule_sets == 0
            || self.max_rules == 0
            || self.max_actions_per_event == 0
            || self.max_actions_per_rule == 0
            || self.max_condition_depth == 0
            || self.max_condition_nodes == 0
            || self.max_template_depth == 0
            || self.max_template_parts == 0
            || self.max_variables == 0
            || self.max_variables > lorepia_domain::MAX_VARIABLES
            || self.max_proposals == 0
            || self.max_proposals > lorepia_domain::MAX_INTERACTION_PROPOSALS
            || self.max_pending_proposals == 0
            || self.max_pending_proposals > self.max_proposals
            || self.max_pending_proposals > DEFAULT_MAX_PENDING_INTERACTION_PROPOSALS
            || self.max_effects == 0
            || self.max_choices == 0
            || self.max_dice_count == 0
            || self.max_dice_sides < 2
            || self.max_text_chars == 0
            || self.max_identifier_bytes == 0
        {
            return Err(InteractionFailure::new(
                InteractionFailureCode::InvalidLimits,
                "interaction limits exceed hard safety bounds or are otherwise invalid",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionCompileOptions {
    /// Imported rules remain inert unless their exact provenance source ID is
    /// present here. Callers populate this only from persisted user approvals.
    pub approved_import_source_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub struct InteractionTemplateValues {
    pub character_name: Option<String>,
    pub user_name: Option<String>,
    pub persona_name: Option<String>,
    pub persona_description: Option<String>,
    /// The caller supplies an already formatted value. The engine never reads
    /// the wall clock, which keeps previews and execution identical.
    pub current_date: Option<String>,
    pub current_time: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct InteractionContext {
    pub deterministic_seed: u64,
    /// Explicit event time used for durable approval records. The engine never
    /// reads a clock, so replay and preview behavior remain deterministic.
    pub event_epoch_seconds: i64,
    pub model_capabilities: Vec<CapabilityKey>,
    pub template_values: InteractionTemplateValues,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConditionContext<'a> {
    pub variables: &'a VariableMap,
    pub model_capabilities: &'a [CapabilityKey],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionRuleStatus {
    Applied,
    ConditionFalse,
    Disabled,
    PendingImportApproval,
    EventDidNotMatch,
    ActionBudgetExceeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionRuleTrace {
    pub rule_id: InteractionRuleId,
    pub status: InteractionRuleStatus,
    pub action_count: u32,
    pub effect_count: u32,
    pub state_changed: bool,
    pub error: Option<InteractionFailure>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionOutcome {
    pub state: InteractionState,
    pub effects: Vec<InteractionEffect>,
    /// State-changing effects that must be re-dispatched as typed interaction
    /// events after the parent transition is durable. Core/Storage use the
    /// exact rule/action/effect coordinates below to build an idempotent,
    /// bounded derived-event chain; the pure engine never recursively invokes
    /// itself here.
    #[serde(default)]
    pub derived_events: Vec<InteractionDerivedEvent>,
    pub trace: Vec<InteractionRuleTrace>,
    pub state_changed: bool,
}

/// One typed event derived from an exact action effect.
///
/// The source coordinates are deliberately structural rather than caller
/// supplied hashes. Core binds them to the immutable rule-set revision and
/// action payload, and Storage verifies that binding before enqueueing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionDerivedEvent {
    pub event: InteractionEvent,
    pub source_rule_set_id: InteractionRuleSetId,
    pub source_rule_id: InteractionRuleId,
    pub source_action_ordinal: u32,
    pub source_effect_ordinal: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalDecisionOutcome {
    pub state: InteractionState,
    pub proposal: InteractionProposalRecord,
    /// Core must persist `state` with compare-and-swap before dispatching this
    /// event. Rejections never produce an event.
    pub event_after_commit: Option<InteractionEvent>,
}

/// Deterministic terminalization of every pending proposal whose deadline has
/// elapsed. Expiry never dispatches a `UserAction`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalExpiryOutcome {
    pub state: InteractionState,
    pub expired_proposals: Vec<InteractionProposalRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionFailureCode {
    InvalidLimits,
    TooManyRuleSets,
    TooManyRules,
    DuplicateRuleSetId,
    DuplicateRuleId,
    InvalidIdentifier,
    InvalidCondition,
    InvalidTemplate,
    InvalidAction,
    UnsafeText,
    ImportedScopeViolation,
    StateLimitExceeded,
    ProposalLimitExceeded,
    DuplicateProposal,
    UnknownProposal,
    ProposalNotPending,
    ProposalExpired,
    RevisionConflict,
    InvalidTimestamp,
    EffectLimitExceeded,
    MissingVariable,
    TypeMismatch,
    NumericOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionFailure {
    pub code: InteractionFailureCode,
    pub message: String,
}

impl InteractionFailure {
    fn new(code: InteractionFailureCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for InteractionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for InteractionFailure {}

#[derive(Debug, Clone)]
struct CompiledRule {
    set_id: InteractionRuleSetId,
    rule: InteractionRule,
    approved_for_execution: bool,
    set_action_limit: usize,
}

struct ActionApplication<'a> {
    actions: &'a [InteractionAction],
    rule_set_id: &'a InteractionRuleSetId,
    rule_id: &'a InteractionRuleId,
    provenance: &'a lorepia_domain::Provenance,
    context: &'a InteractionContext,
    seed: u64,
    effect_ordinal_base: usize,
}

#[derive(Debug, Clone)]
pub struct InteractionEngine {
    rules: Vec<CompiledRule>,
    limits: InteractionLimits,
}

impl InteractionEngine {
    pub fn compile(
        rule_sets: &[InteractionRuleSet],
        limits: InteractionLimits,
    ) -> Result<Self, InteractionFailure> {
        Self::compile_with_options(rule_sets, limits, &InteractionCompileOptions::default())
    }

    pub fn compile_with_options(
        rule_sets: &[InteractionRuleSet],
        limits: InteractionLimits,
        options: &InteractionCompileOptions,
    ) -> Result<Self, InteractionFailure> {
        let limits = limits.validate()?;
        if rule_sets.len() > limits.max_rule_sets {
            return Err(InteractionFailure::new(
                InteractionFailureCode::TooManyRuleSets,
                format!(
                    "interaction rule set count {} exceeds limit {}",
                    rule_sets.len(),
                    limits.max_rule_sets
                ),
            ));
        }
        let total_rules = rule_sets
            .iter()
            .try_fold(0_usize, |count, set| count.checked_add(set.rules.len()))
            .ok_or_else(|| {
                InteractionFailure::new(
                    InteractionFailureCode::TooManyRules,
                    "interaction rule count overflowed",
                )
            })?;
        if total_rules > limits.max_rules {
            return Err(InteractionFailure::new(
                InteractionFailureCode::TooManyRules,
                format!(
                    "interaction rule count {total_rules} exceeds limit {}",
                    limits.max_rules
                ),
            ));
        }

        let mut rules = Vec::with_capacity(total_rules);
        let mut set_ids = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for set in rule_sets {
            validate_identifier("interaction rule set id", set.id.as_str(), limits)?;
            validate_text("interaction rule set name", &set.name, limits)?;
            if !set_ids.insert(set.id.as_str()) {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::DuplicateRuleSetId,
                    format!("duplicate interaction rule set id `{}`", set.id.as_str()),
                ));
            }
            let set_action_limit = usize::try_from(set.max_actions_per_event).unwrap_or(usize::MAX);
            if set_action_limit == 0 || set_action_limit > limits.max_actions_per_event {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::InvalidLimits,
                    format!(
                        "rule set `{}` action limit must be within 1..={}",
                        set.id.as_str(),
                        limits.max_actions_per_event
                    ),
                ));
            }

            for rule in &set.rules {
                validate_identifier("interaction rule id", rule.id.as_str(), limits)?;
                validate_text("interaction rule name", &rule.name, limits)?;
                if !ids.insert(rule.id.as_str()) {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::DuplicateRuleId,
                        format!("duplicate interaction rule id `{}`", rule.id.as_str()),
                    ));
                }
                validate_event(&rule.event, limits)?;
                if let Some(condition) = &rule.condition {
                    validate_condition(condition, limits)?;
                }
                if rule.actions.len() > limits.max_actions_per_rule {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::InvalidAction,
                        format!(
                            "rule `{}` has {} actions; limit is {}",
                            rule.id.as_str(),
                            rule.actions.len(),
                            limits.max_actions_per_rule
                        ),
                    ));
                }
                for action in &rule.actions {
                    validate_action(action, &rule.provenance, limits)?;
                }
                let approved_for_execution =
                    rule_provenance_is_approved(&set.provenance, &rule.provenance, options)?;
                rules.push(CompiledRule {
                    set_id: set.id.clone(),
                    rule: rule.clone(),
                    approved_for_execution,
                    set_action_limit,
                });
            }
        }

        rules.sort_by(|left, right| {
            left.rule
                .priority
                .cmp(&right.rule.priority)
                .then_with(|| left.rule.id.cmp(&right.rule.id))
        });
        Ok(Self { rules, limits })
    }

    #[must_use]
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    #[allow(clippy::too_many_lines)]
    pub fn handle_event(
        &self,
        state: &InteractionState,
        event: &InteractionEvent,
        context: &InteractionContext,
    ) -> Result<InteractionOutcome, InteractionFailure> {
        self.validate_state(state)?;
        validate_event(event, self.limits)?;
        validate_epoch_seconds("interaction event time", context.event_epoch_seconds)?;

        let mut next_state = state.clone();
        let mut effects = Vec::new();
        let mut derived_events = Vec::new();
        let mut trace = Vec::with_capacity(self.rules.len());
        let mut executed_actions = 0_usize;
        let mut executed_actions_by_set = BTreeMap::<InteractionRuleSetId, usize>::new();
        let seed = event_seed(context.deterministic_seed, event, state.revision);

        for compiled in &self.rules {
            let rule = &compiled.rule;
            if !rule.enabled {
                trace.push(rule_trace(rule, InteractionRuleStatus::Disabled));
                continue;
            }
            if !compiled.approved_for_execution {
                trace.push(rule_trace(
                    rule,
                    InteractionRuleStatus::PendingImportApproval,
                ));
                continue;
            }
            if rule.event != *event {
                trace.push(rule_trace(rule, InteractionRuleStatus::EventDidNotMatch));
                continue;
            }

            if let Some(condition) = &rule.condition {
                let condition_context = ConditionContext {
                    variables: &next_state.variables,
                    model_capabilities: &context.model_capabilities,
                };
                match evaluate_condition_ast(condition, condition_context, self.limits) {
                    Ok(true) => {}
                    Ok(false) => {
                        trace.push(rule_trace(rule, InteractionRuleStatus::ConditionFalse));
                        continue;
                    }
                    Err(error) => {
                        trace.push(failed_trace(rule, error));
                        continue;
                    }
                }
            }

            let next_action_count = executed_actions.saturating_add(rule.actions.len());
            let next_set_action_count = executed_actions_by_set
                .get(&compiled.set_id)
                .copied()
                .unwrap_or(0)
                .saturating_add(rule.actions.len());
            if next_action_count > self.limits.max_actions_per_event
                || next_set_action_count > compiled.set_action_limit
            {
                trace.push(InteractionRuleTrace {
                    rule_id: rule.id.clone(),
                    status: InteractionRuleStatus::ActionBudgetExceeded,
                    action_count: 0,
                    effect_count: 0,
                    state_changed: false,
                    error: Some(InteractionFailure::new(
                        InteractionFailureCode::InvalidAction,
                        "interaction action budget for this event was exhausted",
                    )),
                });
                continue;
            }

            let mut candidate_state = next_state.clone();
            let mut candidate_effects = Vec::new();
            let mut candidate_derived_events = Vec::new();
            let rule_seed = mix_seed(seed, stable_hash(rule.id.as_str().as_bytes()));
            match self.apply_actions(
                &mut candidate_state,
                &mut candidate_effects,
                &mut candidate_derived_events,
                ActionApplication {
                    actions: &rule.actions,
                    rule_set_id: &compiled.set_id,
                    rule_id: &rule.id,
                    provenance: &rule.provenance,
                    context,
                    seed: rule_seed,
                    effect_ordinal_base: effects.len(),
                },
            ) {
                Ok(()) => {
                    if effects.len().saturating_add(candidate_effects.len())
                        > self.limits.max_effects
                    {
                        trace.push(failed_trace(
                            rule,
                            InteractionFailure::new(
                                InteractionFailureCode::EffectLimitExceeded,
                                "interaction effect limit exceeded",
                            ),
                        ));
                        continue;
                    }
                    let state_changed = candidate_state != next_state;
                    let effect_count = u32::try_from(candidate_effects.len()).unwrap_or(u32::MAX);
                    next_state = candidate_state;
                    effects.extend(candidate_effects);
                    derived_events.extend(candidate_derived_events);
                    executed_actions = next_action_count;
                    executed_actions_by_set.insert(compiled.set_id.clone(), next_set_action_count);
                    trace.push(InteractionRuleTrace {
                        rule_id: rule.id.clone(),
                        status: InteractionRuleStatus::Applied,
                        action_count: u32::try_from(rule.actions.len()).unwrap_or(u32::MAX),
                        effect_count,
                        state_changed,
                        error: None,
                    });
                    if rule.stop_after_match {
                        break;
                    }
                }
                Err(error) => trace.push(failed_trace(rule, error)),
            }
        }

        let state_changed = next_state != *state;
        if state_changed {
            next_state.revision = state.revision.checked_add(1).ok_or_else(|| {
                InteractionFailure::new(
                    InteractionFailureCode::NumericOverflow,
                    "interaction state revision overflowed",
                )
            })?;
        }
        Ok(InteractionOutcome {
            state: next_state,
            effects,
            derived_events,
            trace,
            state_changed,
        })
    }

    fn validate_state(&self, state: &InteractionState) -> Result<(), InteractionFailure> {
        validate_interaction_state(state, self.limits)
    }

    #[allow(clippy::too_many_lines)]
    fn apply_actions(
        &self,
        state: &mut InteractionState,
        effects: &mut Vec<InteractionEffect>,
        derived_events: &mut Vec<InteractionDerivedEvent>,
        application: ActionApplication<'_>,
    ) -> Result<(), InteractionFailure> {
        for (index, action) in application.actions.iter().enumerate() {
            validate_action(action, application.provenance, self.limits)?;
            let effect_start = effects.len();
            match action {
                InteractionAction::SetVariable { target, value } => {
                    let value = resolve_value(value, &state.variables)?.clone();
                    set_variable(state, effects, target, value);
                }
                InteractionAction::IncrementVariable { target, amount } => {
                    let previous = state.variables.get(target).cloned();
                    let current = match previous.as_ref() {
                        Some(VariableValue::Integer(value)) => *value,
                        None => 0,
                        Some(_) => {
                            return Err(InteractionFailure::new(
                                InteractionFailureCode::TypeMismatch,
                                format!(
                                    "increment target `{}` is not an integer",
                                    target.id.as_str()
                                ),
                            ));
                        }
                    };
                    let value = current.checked_add(*amount).ok_or_else(|| {
                        InteractionFailure::new(
                            InteractionFailureCode::NumericOverflow,
                            format!("increment target `{}` overflowed", target.id.as_str()),
                        )
                    })?;
                    set_variable(state, effects, target, VariableValue::Integer(value));
                }
                InteractionAction::ActivateKnowledge { entry_id } => {
                    if !state.manually_active_knowledge.contains(entry_id) {
                        state.manually_active_knowledge.push(entry_id.clone());
                        state.manually_active_knowledge.sort();
                        effects.push(InteractionEffect::KnowledgeActivated {
                            entry_id: entry_id.clone(),
                        });
                    }
                }
                InteractionAction::ShowAsset { asset_id, region } => {
                    effects.push(InteractionEffect::AssetShown {
                        asset_id: asset_id.clone(),
                        region: *region,
                    });
                }
                InteractionAction::PlayAudio { asset_id } => {
                    effects.push(InteractionEffect::AudioRequested {
                        asset_id: asset_id.clone(),
                    });
                }
                InteractionAction::PresentChoices { choices } => {
                    let mut rendered_choices = Vec::with_capacity(choices.len());
                    for choice in choices {
                        let enabled = if let Some(condition) = &choice.enabled_when {
                            evaluate_condition_ast(
                                condition,
                                ConditionContext {
                                    variables: &state.variables,
                                    model_capabilities: &application.context.model_capabilities,
                                },
                                self.limits,
                            )?
                        } else {
                            true
                        };
                        if enabled {
                            rendered_choices.push(choice.clone());
                        }
                    }
                    if !rendered_choices.is_empty() {
                        effects.push(InteractionEffect::ChoicesPresented {
                            choices: rendered_choices,
                        });
                    }
                }
                InteractionAction::AppendVisibleSystemEvent { text } => {
                    let rendered = render_safe_template(
                        text,
                        &state.variables,
                        application.context,
                        self.limits,
                    )?;
                    effects.push(InteractionEffect::VisibleSystemEvent { text: rendered });
                }
                InteractionAction::RollDice { expression, target } => {
                    let action_seed =
                        mix_seed(application.seed, u64::try_from(index).unwrap_or(u64::MAX));
                    let (rolls, total) = roll_dice(expression, action_seed)?;
                    if let Some(target) = target {
                        set_variable(state, effects, target, VariableValue::Integer(total));
                    }
                    effects.push(InteractionEffect::DiceRolled {
                        expression: expression.clone(),
                        rolls,
                        total,
                        target: target.clone(),
                    });
                }
                InteractionAction::RequestUserApproval { proposal } => {
                    let body = render_safe_template(
                        &proposal.body,
                        &state.variables,
                        application.context,
                        self.limits,
                    )?;
                    append_pending_proposal(
                        state,
                        application.rule_set_id,
                        application.rule_id,
                        proposal,
                        &body,
                        application.context.event_epoch_seconds,
                        self.limits,
                    )?;
                    effects.push(InteractionEffect::ApprovalRequested {
                        rule_set_id: application.rule_set_id.clone(),
                        rule_id: application.rule_id.clone(),
                        proposal_id: proposal.id.clone(),
                        title: proposal.title.clone(),
                        body,
                        expires_after_seconds: proposal.expires_after_seconds,
                    });
                }
            }

            for (effect_ordinal, effect) in effects.iter().enumerate().skip(effect_start) {
                let event = match effect {
                    InteractionEffect::VariableSet { target, .. } => {
                        InteractionEvent::VariableChanged {
                            variable: target.clone(),
                        }
                    }
                    InteractionEffect::KnowledgeActivated { entry_id } => {
                        InteractionEvent::KnowledgeActivated {
                            entry_id: entry_id.clone(),
                        }
                    }
                    InteractionEffect::AssetShown { .. }
                    | InteractionEffect::AudioRequested { .. }
                    | InteractionEffect::ChoicesPresented { .. }
                    | InteractionEffect::VisibleSystemEvent { .. }
                    | InteractionEffect::DiceRolled { .. }
                    | InteractionEffect::ApprovalRequested { .. } => continue,
                };
                let source_action_ordinal = u32::try_from(index).map_err(|_| {
                    InteractionFailure::new(
                        InteractionFailureCode::NumericOverflow,
                        "interaction action ordinal overflowed",
                    )
                })?;
                let source_effect_ordinal = application
                    .effect_ordinal_base
                    .checked_add(effect_ordinal)
                    .and_then(|ordinal| u32::try_from(ordinal).ok())
                    .ok_or_else(|| {
                        InteractionFailure::new(
                            InteractionFailureCode::NumericOverflow,
                            "interaction effect ordinal overflowed",
                        )
                    })?;
                derived_events.push(InteractionDerivedEvent {
                    event,
                    source_rule_set_id: application.rule_set_id.clone(),
                    source_rule_id: application.rule_id.clone(),
                    source_action_ordinal,
                    source_effect_ordinal,
                });
            }

            if state.variables.values.len() > self.limits.max_variables {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::StateLimitExceeded,
                    "interaction variable count exceeds the configured limit",
                ));
            }
            if state.manually_active_knowledge.len() > lorepia_domain::MAX_KNOWLEDGE_ENTRIES {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::StateLimitExceeded,
                    "active knowledge count exceeds the canonical domain limit",
                ));
            }
            if state.proposals.len() > self.limits.max_proposals {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::ProposalLimitExceeded,
                    "interaction proposal count exceeds the configured limit",
                ));
            }
            if effects.len() > self.limits.max_effects {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::EffectLimitExceeded,
                    "interaction effect limit exceeded",
                ));
            }
        }
        Ok(())
    }
}

/// Approves one exact pending proposal using an interaction-state revision
/// compare-and-swap.
///
/// Core must durably commit the returned state before dispatching
/// `event_after_commit`. The event contains only the validated proposal ID;
/// callers cannot submit an arbitrary action.
pub fn approve_pending(
    state: &InteractionState,
    proposal_id: &str,
    expected_revision: u64,
    caller_now_epoch_seconds: i64,
) -> Result<InteractionProposalDecisionOutcome, InteractionFailure> {
    decide_pending(
        state,
        proposal_id,
        InteractionProposalDecision::Approve,
        expected_revision,
        caller_now_epoch_seconds,
    )
}

/// Rejects one exact pending proposal while retaining its durable audit record.
pub fn reject_pending(
    state: &InteractionState,
    proposal_id: &str,
    expected_revision: u64,
    caller_now_epoch_seconds: i64,
) -> Result<InteractionProposalDecisionOutcome, InteractionFailure> {
    decide_pending(
        state,
        proposal_id,
        InteractionProposalDecision::Reject,
        expected_revision,
        caller_now_epoch_seconds,
    )
}

/// Marks all due pending proposals expired under one state-revision CAS.
///
/// If no deadline has elapsed the state and revision are returned unchanged.
/// Otherwise all due records become terminal at `caller_now_epoch_seconds` and
/// the state revision advances exactly once. No post-commit action is produced.
pub fn expire_pending_proposals(
    state: &InteractionState,
    expected_revision: u64,
    caller_now_epoch_seconds: i64,
) -> Result<InteractionProposalExpiryOutcome, InteractionFailure> {
    let limits = InteractionLimits::default().validate()?;
    validate_interaction_state(state, limits)?;
    validate_epoch_seconds("proposal expiry time", caller_now_epoch_seconds)?;
    if state.revision != expected_revision {
        return Err(InteractionFailure::new(
            InteractionFailureCode::RevisionConflict,
            format!(
                "interaction state revision is {}; expected {expected_revision}",
                state.revision
            ),
        ));
    }
    let due_indices = state
        .proposals
        .iter()
        .enumerate()
        .filter_map(|(index, proposal)| {
            (proposal.status == InteractionProposalStatus::Pending
                && proposal
                    .expires_at_epoch_seconds
                    .is_some_and(|expires_at| caller_now_epoch_seconds >= expires_at))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    if due_indices.is_empty() {
        return Ok(InteractionProposalExpiryOutcome {
            state: state.clone(),
            expired_proposals: Vec::new(),
        });
    }
    let next_revision = state.revision.checked_add(1).ok_or_else(|| {
        InteractionFailure::new(
            InteractionFailureCode::NumericOverflow,
            "interaction state revision overflowed",
        )
    })?;
    let mut next_state = state.clone();
    let mut expired_proposals = Vec::with_capacity(due_indices.len());
    for index in due_indices {
        let proposal = &mut next_state.proposals[index];
        proposal.status = InteractionProposalStatus::Expired;
        proposal.decided_at_epoch_seconds = Some(caller_now_epoch_seconds);
        expired_proposals.push(proposal.clone());
    }
    next_state.revision = next_revision;
    validate_interaction_state(&next_state, limits)?;
    Ok(InteractionProposalExpiryOutcome {
        state: next_state,
        expired_proposals,
    })
}

/// Marks one exact due pending proposal expired under one state-revision CAS.
///
/// Generation-attempt aggregates serialize decisions one proposal at a time,
/// so expiring one record must not terminalize any neighboring due proposal.
/// No post-commit action is produced.
pub fn expire_pending_proposal(
    state: &InteractionState,
    proposal_id: &str,
    expected_revision: u64,
    caller_now_epoch_seconds: i64,
) -> Result<InteractionProposalExpiryOutcome, InteractionFailure> {
    let limits = InteractionLimits::default().validate()?;
    validate_interaction_state(state, limits)?;
    validate_identifier("proposal id", proposal_id, limits)?;
    validate_epoch_seconds("proposal expiry time", caller_now_epoch_seconds)?;
    if state.revision != expected_revision {
        return Err(InteractionFailure::new(
            InteractionFailureCode::RevisionConflict,
            format!(
                "interaction state revision is {}; expected {expected_revision}",
                state.revision
            ),
        ));
    }
    let proposal_index = state
        .proposals
        .iter()
        .position(|proposal| {
            proposal.proposal_id == proposal_id
                && proposal.status == InteractionProposalStatus::Pending
        })
        .ok_or_else(|| {
            if state
                .proposals
                .iter()
                .any(|proposal| proposal.proposal_id == proposal_id)
            {
                InteractionFailure::new(
                    InteractionFailureCode::ProposalNotPending,
                    format!("interaction proposal `{proposal_id}` is no longer pending"),
                )
            } else {
                InteractionFailure::new(
                    InteractionFailureCode::UnknownProposal,
                    format!("interaction proposal `{proposal_id}` does not exist"),
                )
            }
        })?;
    let current = &state.proposals[proposal_index];
    if caller_now_epoch_seconds < current.requested_at_epoch_seconds {
        return Err(InteractionFailure::new(
            InteractionFailureCode::InvalidTimestamp,
            "proposal expiry time precedes its request time",
        ));
    }
    if current
        .expires_at_epoch_seconds
        .is_none_or(|expires_at| caller_now_epoch_seconds < expires_at)
    {
        return Err(InteractionFailure::new(
            InteractionFailureCode::InvalidTimestamp,
            format!("interaction proposal `{proposal_id}` is not due"),
        ));
    }
    let next_revision = state.revision.checked_add(1).ok_or_else(|| {
        InteractionFailure::new(
            InteractionFailureCode::NumericOverflow,
            "interaction state revision overflowed",
        )
    })?;
    let mut next_state = state.clone();
    let expired = {
        let proposal = &mut next_state.proposals[proposal_index];
        proposal.status = InteractionProposalStatus::Expired;
        proposal.decided_at_epoch_seconds = Some(caller_now_epoch_seconds);
        proposal.clone()
    };
    next_state.revision = next_revision;
    validate_interaction_state(&next_state, limits)?;
    Ok(InteractionProposalExpiryOutcome {
        state: next_state,
        expired_proposals: vec![expired],
    })
}

/// Applies a proposal decision without executing an action.
///
/// An approval returns one post-commit `UserAction(proposal_id)` event. A
/// rejection returns no event. Replays fail because the durable proposal no
/// longer has `Pending` status.
pub fn decide_pending(
    state: &InteractionState,
    proposal_id: &str,
    decision: InteractionProposalDecision,
    expected_revision: u64,
    caller_now_epoch_seconds: i64,
) -> Result<InteractionProposalDecisionOutcome, InteractionFailure> {
    let limits = InteractionLimits::default().validate()?;
    validate_interaction_state(state, limits)?;
    validate_identifier("proposal id", proposal_id, limits)?;
    validate_epoch_seconds("proposal decision time", caller_now_epoch_seconds)?;
    if state.revision != expected_revision {
        return Err(InteractionFailure::new(
            InteractionFailureCode::RevisionConflict,
            format!(
                "interaction state revision is {}; expected {expected_revision}",
                state.revision
            ),
        ));
    }

    let proposal_index = state
        .proposals
        .iter()
        .position(|proposal| {
            proposal.proposal_id == proposal_id
                && proposal.status == InteractionProposalStatus::Pending
        })
        .ok_or_else(|| {
            if state
                .proposals
                .iter()
                .any(|proposal| proposal.proposal_id == proposal_id)
            {
                InteractionFailure::new(
                    InteractionFailureCode::ProposalNotPending,
                    format!("interaction proposal `{proposal_id}` is no longer pending"),
                )
            } else {
                InteractionFailure::new(
                    InteractionFailureCode::UnknownProposal,
                    format!("interaction proposal `{proposal_id}` does not exist"),
                )
            }
        })?;
    let current = &state.proposals[proposal_index];
    if caller_now_epoch_seconds < current.requested_at_epoch_seconds {
        return Err(InteractionFailure::new(
            InteractionFailureCode::InvalidTimestamp,
            "proposal decision time precedes its request time",
        ));
    }
    if current
        .expires_at_epoch_seconds
        .is_some_and(|expires_at| caller_now_epoch_seconds >= expires_at)
    {
        return Err(InteractionFailure::new(
            InteractionFailureCode::ProposalExpired,
            format!("interaction proposal `{proposal_id}` has expired"),
        ));
    }

    let next_revision = state.revision.checked_add(1).ok_or_else(|| {
        InteractionFailure::new(
            InteractionFailureCode::NumericOverflow,
            "interaction state revision overflowed",
        )
    })?;
    let mut next_state = state.clone();
    let proposal = {
        let proposal = &mut next_state.proposals[proposal_index];
        proposal.status = match decision {
            InteractionProposalDecision::Approve => InteractionProposalStatus::Approved,
            InteractionProposalDecision::Reject => InteractionProposalStatus::Rejected,
        };
        proposal.decided_at_epoch_seconds = Some(caller_now_epoch_seconds);
        proposal.clone()
    };
    next_state.revision = next_revision;
    let event_after_commit =
        (decision == InteractionProposalDecision::Approve).then(|| InteractionEvent::UserAction {
            action_id: proposal.proposal_id.clone(),
        });

    Ok(InteractionProposalDecisionOutcome {
        state: next_state,
        proposal,
        event_after_commit,
    })
}

#[allow(clippy::too_many_lines)]
fn validate_interaction_state(
    state: &InteractionState,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    if state.variables.values.len() > limits.max_variables {
        return Err(InteractionFailure::new(
            InteractionFailureCode::StateLimitExceeded,
            "interaction variable count exceeds the configured limit",
        ));
    }
    let mut variables = BTreeSet::new();
    for binding in &state.variables.values {
        validate_variable_ref(&binding.variable, limits)?;
        validate_variable_value(&binding.value, limits)?;
        if !variables.insert(&binding.variable) {
            return Err(InteractionFailure::new(
                InteractionFailureCode::StateLimitExceeded,
                "interaction state contains duplicate variable bindings",
            ));
        }
    }

    if state.manually_active_knowledge.len() > lorepia_domain::MAX_KNOWLEDGE_ENTRIES {
        return Err(InteractionFailure::new(
            InteractionFailureCode::StateLimitExceeded,
            "active knowledge count exceeds the canonical domain limit",
        ));
    }
    let mut knowledge = BTreeSet::new();
    for entry_id in &state.manually_active_knowledge {
        validate_identifier("knowledge entry id", entry_id.as_str(), limits)?;
        if !knowledge.insert(entry_id) {
            return Err(InteractionFailure::new(
                InteractionFailureCode::StateLimitExceeded,
                "interaction state contains duplicate active knowledge IDs",
            ));
        }
    }

    if state.proposals.len() > limits.max_proposals {
        return Err(InteractionFailure::new(
            InteractionFailureCode::ProposalLimitExceeded,
            "interaction proposal count exceeds the configured limit",
        ));
    }
    let mut record_ids = BTreeSet::new();
    let mut pending_proposal_ids = BTreeSet::new();
    let mut pending_count = 0_usize;
    for proposal in &state.proposals {
        validate_identifier("proposal record id", proposal.id.as_str(), limits)?;
        validate_identifier(
            "proposal rule set id",
            proposal.rule_set_id.as_str(),
            limits,
        )?;
        validate_identifier("proposal rule id", proposal.rule_id.as_str(), limits)?;
        validate_identifier("proposal id", &proposal.proposal_id, limits)?;
        validate_legacy_stored_proposal_text(
            "proposal title",
            &proposal.title,
            MAX_INTERACTION_PROPOSAL_TITLE_CHARS,
        )?;
        validate_legacy_stored_proposal_text(
            "proposal body",
            &proposal.body,
            MAX_INTERACTION_PROPOSAL_BODY_CHARS,
        )?;
        validate_epoch_seconds("proposal request time", proposal.requested_at_epoch_seconds)?;
        if proposal.source_interaction_state_revision > state.revision {
            return Err(InteractionFailure::new(
                InteractionFailureCode::StateLimitExceeded,
                "proposal source revision exceeds the durable interaction state revision",
            ));
        }
        if proposal
            .expires_at_epoch_seconds
            .is_some_and(|expires_at| expires_at < proposal.requested_at_epoch_seconds)
        {
            return Err(InteractionFailure::new(
                InteractionFailureCode::InvalidTimestamp,
                "proposal expiration must not predate its request time",
            ));
        }
        if let Some(decided_at) = proposal.decided_at_epoch_seconds {
            validate_epoch_seconds("proposal decision time", decided_at)?;
            if decided_at < proposal.requested_at_epoch_seconds
                || (proposal.status != InteractionProposalStatus::Expired
                    && proposal
                        .expires_at_epoch_seconds
                        .is_some_and(|expires_at| decided_at >= expires_at))
            {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::InvalidTimestamp,
                    "proposal decision time is outside its valid lifetime",
                ));
            }
        }
        match proposal.status {
            InteractionProposalStatus::Pending => {
                if proposal.decided_at_epoch_seconds.is_some() {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::ProposalNotPending,
                        "pending proposal has a decision timestamp",
                    ));
                }
                if !pending_proposal_ids.insert(&proposal.proposal_id) {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::DuplicateProposal,
                        "interaction state contains duplicate pending proposal IDs",
                    ));
                }
                pending_count = pending_count.saturating_add(1);
            }
            InteractionProposalStatus::Approved | InteractionProposalStatus::Rejected => {
                if proposal.decided_at_epoch_seconds.is_none() {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::ProposalNotPending,
                        "decided proposal is missing its decision timestamp",
                    ));
                }
            }
            InteractionProposalStatus::Expired => {
                let valid_expiry = proposal
                    .expires_at_epoch_seconds
                    .zip(proposal.decided_at_epoch_seconds)
                    .is_some_and(|(expires_at, expired_at)| expired_at >= expires_at);
                if !valid_expiry {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::InvalidTimestamp,
                        "expired proposal lacks a terminal timestamp at or after its deadline",
                    ));
                }
            }
        }
        let expected_record_id = proposal_record_id(
            &proposal.rule_set_id,
            &proposal.rule_id,
            &proposal.proposal_id,
            proposal.source_interaction_state_revision,
        )?;
        if expected_record_id != proposal.id {
            return Err(InteractionFailure::new(
                InteractionFailureCode::DuplicateProposal,
                "proposal record ID does not match its deterministic binding",
            ));
        }
        if !record_ids.insert(proposal.id.as_str()) {
            return Err(InteractionFailure::new(
                InteractionFailureCode::DuplicateProposal,
                "interaction state contains a duplicate proposal record ID",
            ));
        }
    }
    if pending_count > limits.max_pending_proposals {
        return Err(InteractionFailure::new(
            InteractionFailureCode::ProposalLimitExceeded,
            "pending interaction proposal count exceeds the configured limit",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_pending_proposal(
    state: &mut InteractionState,
    rule_set_id: &InteractionRuleSetId,
    rule_id: &InteractionRuleId,
    proposal: &lorepia_domain::ProposalSpec,
    rendered_body: &str,
    event_epoch_seconds: i64,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    validate_epoch_seconds("interaction event time", event_epoch_seconds)?;
    validate_required_native_plain_text(
        "proposal title",
        &proposal.title,
        MAX_INTERACTION_PROPOSAL_TITLE_CHARS,
        limits,
    )?;
    validate_required_native_plain_text(
        "proposal body",
        rendered_body,
        MAX_INTERACTION_PROPOSAL_BODY_CHARS,
        limits,
    )?;
    if state.proposals.len() >= limits.max_proposals {
        return Err(InteractionFailure::new(
            InteractionFailureCode::ProposalLimitExceeded,
            "interaction proposal audit limit has been reached",
        ));
    }
    if state
        .proposals
        .iter()
        .filter(|proposal| proposal.status == InteractionProposalStatus::Pending)
        .count()
        >= limits.max_pending_proposals
    {
        return Err(InteractionFailure::new(
            InteractionFailureCode::ProposalLimitExceeded,
            "pending interaction proposal limit has been reached",
        ));
    }
    if state.proposals.iter().any(|existing| {
        existing.proposal_id == proposal.id && existing.status == InteractionProposalStatus::Pending
    }) {
        return Err(InteractionFailure::new(
            InteractionFailureCode::DuplicateProposal,
            format!(
                "proposal ID `{}` already has a pending request",
                proposal.id
            ),
        ));
    }

    let expires_at_epoch_seconds = proposal
        .expires_after_seconds
        .map(|seconds| {
            event_epoch_seconds
                .checked_add(i64::from(seconds))
                .ok_or_else(|| {
                    InteractionFailure::new(
                        InteractionFailureCode::NumericOverflow,
                        "proposal expiration time overflowed",
                    )
                })
        })
        .transpose()?;
    let record_id = proposal_record_id(rule_set_id, rule_id, &proposal.id, state.revision)?;
    if state
        .proposals
        .iter()
        .any(|existing| existing.id == record_id)
    {
        return Err(InteractionFailure::new(
            InteractionFailureCode::DuplicateProposal,
            "deterministic proposal record was already created",
        ));
    }
    state.proposals.push(InteractionProposalRecord {
        id: record_id,
        rule_set_id: rule_set_id.clone(),
        rule_id: rule_id.clone(),
        proposal_id: proposal.id.clone(),
        title: proposal.title.clone(),
        body: rendered_body.to_owned(),
        status: InteractionProposalStatus::Pending,
        source_interaction_state_revision: state.revision,
        requested_at_epoch_seconds: event_epoch_seconds,
        expires_at_epoch_seconds,
        decided_at_epoch_seconds: None,
    });
    Ok(())
}

fn proposal_record_id(
    rule_set_id: &InteractionRuleSetId,
    rule_id: &InteractionRuleId,
    proposal_id: &str,
    source_revision: u64,
) -> Result<InteractionProposalRecordId, InteractionFailure> {
    let mut hasher = Sha256::new();
    hash_framed_field(&mut hasher, b"lorepia.interaction-proposal.v1")?;
    hash_framed_field(&mut hasher, rule_set_id.as_str().as_bytes())?;
    hash_framed_field(&mut hasher, rule_id.as_str().as_bytes())?;
    hash_framed_field(&mut hasher, proposal_id.as_bytes())?;
    hasher.update(source_revision.to_be_bytes());
    Ok(InteractionProposalRecordId::from(hex::encode(
        hasher.finalize(),
    )))
}

fn hash_framed_field(hasher: &mut Sha256, value: &[u8]) -> Result<(), InteractionFailure> {
    let length = u64::try_from(value.len()).map_err(|_| {
        InteractionFailure::new(
            InteractionFailureCode::NumericOverflow,
            "proposal hash field length overflowed",
        )
    })?;
    hasher.update(length.to_be_bytes());
    hasher.update(value);
    Ok(())
}

fn validate_epoch_seconds(label: &str, epoch_seconds: i64) -> Result<(), InteractionFailure> {
    if epoch_seconds < 0 {
        return Err(InteractionFailure::new(
            InteractionFailureCode::InvalidTimestamp,
            format!("{label} cannot be before the Unix epoch"),
        ));
    }
    Ok(())
}

pub(crate) fn evaluate_condition_ast(
    expression: &ConditionExpr,
    context: ConditionContext<'_>,
    limits: InteractionLimits,
) -> Result<bool, InteractionFailure> {
    let limits = limits.validate()?;
    validate_condition(expression, limits)?;
    evaluate_condition_inner(expression, context)
}

#[allow(clippy::cast_precision_loss)]
fn evaluate_condition_inner(
    expression: &ConditionExpr,
    context: ConditionContext<'_>,
) -> Result<bool, InteractionFailure> {
    match expression {
        ConditionExpr::True => Ok(true),
        ConditionExpr::False => Ok(false),
        ConditionExpr::Equals { variable, value } => {
            Ok(context.variables.get(variable) == Some(value))
        }
        ConditionExpr::NotEquals { variable, value } => {
            Ok(context.variables.get(variable) != Some(value))
        }
        ConditionExpr::GreaterThan { variable, value } => match context.variables.get(variable) {
            Some(VariableValue::Integer(current)) => Ok((*current as f64) > *value),
            Some(VariableValue::Decimal(current)) => Ok(*current > *value),
            None => Ok(false),
            Some(_) => Err(InteractionFailure::new(
                InteractionFailureCode::TypeMismatch,
                format!(
                    "greater-than condition variable `{}` is not numeric",
                    variable.id.as_str()
                ),
            )),
        },
        ConditionExpr::Contains { variable, value } => match context.variables.get(variable) {
            Some(VariableValue::Text(current) | VariableValue::Enum(current)) => {
                Ok(current.contains(value))
            }
            Some(VariableValue::StringList(current)) => Ok(current.contains(value)),
            None => Ok(false),
            Some(_) => Err(InteractionFailure::new(
                InteractionFailureCode::TypeMismatch,
                format!(
                    "contains condition variable `{}` is not text or a string list",
                    variable.id.as_str()
                ),
            )),
        },
        ConditionExpr::Exists { variable } => Ok(context.variables.get(variable).is_some()),
        ConditionExpr::ModelSupports { capability } => {
            Ok(context.model_capabilities.contains(capability))
        }
        ConditionExpr::All { expressions } => {
            for expression in expressions {
                if !evaluate_condition_inner(expression, context)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        ConditionExpr::Any { expressions } => {
            for expression in expressions {
                if evaluate_condition_inner(expression, context)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        ConditionExpr::Not { expression } => Ok(!evaluate_condition_inner(expression, context)?),
    }
}

fn validate_condition(
    expression: &ConditionExpr,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    fn walk(
        expression: &ConditionExpr,
        limits: InteractionLimits,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<(), InteractionFailure> {
        if depth > limits.max_condition_depth {
            return Err(InteractionFailure::new(
                InteractionFailureCode::InvalidCondition,
                "condition nesting exceeds the configured depth limit",
            ));
        }
        *nodes = nodes.saturating_add(1);
        if *nodes > limits.max_condition_nodes {
            return Err(InteractionFailure::new(
                InteractionFailureCode::InvalidCondition,
                "condition node count exceeds the configured limit",
            ));
        }

        match expression {
            ConditionExpr::Equals { variable, value }
            | ConditionExpr::NotEquals { variable, value } => {
                validate_variable_ref(variable, limits)?;
                validate_variable_value(value, limits)?;
            }
            ConditionExpr::GreaterThan { variable, value } => {
                validate_variable_ref(variable, limits)?;
                if !value.is_finite() {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::InvalidCondition,
                        "condition numeric value must be finite",
                    ));
                }
            }
            ConditionExpr::Contains { variable, value } => {
                validate_variable_ref(variable, limits)?;
                validate_text("condition contains value", value, limits)?;
            }
            ConditionExpr::Exists { variable } => validate_variable_ref(variable, limits)?,
            ConditionExpr::ModelSupports { .. } | ConditionExpr::True | ConditionExpr::False => {}
            ConditionExpr::All { expressions } | ConditionExpr::Any { expressions } => {
                for expression in expressions {
                    walk(expression, limits, depth + 1, nodes)?;
                }
            }
            ConditionExpr::Not { expression } => walk(expression, limits, depth + 1, nodes)?,
        }
        Ok(())
    }

    let mut nodes = 0;
    walk(expression, limits, 1, &mut nodes)
}

fn validate_template(
    template: &SafeTemplate,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    fn walk(
        template: &SafeTemplate,
        limits: InteractionLimits,
        depth: usize,
        parts: &mut usize,
    ) -> Result<(), InteractionFailure> {
        if depth > limits.max_template_depth {
            return Err(InteractionFailure::new(
                InteractionFailureCode::InvalidTemplate,
                "template nesting exceeds the configured depth limit",
            ));
        }
        let output_limit = usize::try_from(template.max_output_chars).unwrap_or(usize::MAX);
        if output_limit == 0 || output_limit > limits.max_text_chars {
            return Err(InteractionFailure::new(
                InteractionFailureCode::InvalidTemplate,
                "template output limit is zero or exceeds the engine text limit",
            ));
        }
        for part in &template.parts {
            *parts = parts.saturating_add(1);
            if *parts > limits.max_template_parts {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::InvalidTemplate,
                    "template part count exceeds the configured limit",
                ));
            }
            match part {
                TemplatePart::Text { value } => validate_text("template text", value, limits)?,
                TemplatePart::Variable { variable } | TemplatePart::Join { variable, .. } => {
                    validate_variable_ref(variable, limits)?;
                    if let TemplatePart::Join { separator, .. } = part {
                        validate_text("template join separator", separator, limits)?;
                    }
                }
                TemplatePart::BuiltIn { .. } => {}
                TemplatePart::Slot { name } => {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::InvalidTemplate,
                        format!("interaction templates do not accept unresolved slot `{name}`"),
                    ));
                }
                TemplatePart::Conditional {
                    condition,
                    then_template,
                    else_template,
                } => {
                    validate_condition(condition, limits)?;
                    walk(then_template, limits, depth + 1, parts)?;
                    if let Some(else_template) = else_template {
                        walk(else_template, limits, depth + 1, parts)?;
                    }
                }
            }
        }
        Ok(())
    }

    let mut parts = 0;
    walk(template, limits, 1, &mut parts)
}

fn render_safe_template(
    template: &SafeTemplate,
    variables: &VariableMap,
    context: &InteractionContext,
    limits: InteractionLimits,
) -> Result<String, InteractionFailure> {
    validate_template(template, limits)?;
    let mut output = String::new();
    render_template_into(template, variables, context, limits, &mut output)?;
    validate_native_plain_text(&output, limits)?;
    Ok(output)
}

fn render_template_into(
    template: &SafeTemplate,
    variables: &VariableMap,
    context: &InteractionContext,
    limits: InteractionLimits,
    output: &mut String,
) -> Result<(), InteractionFailure> {
    let template_limit = usize::try_from(template.max_output_chars).unwrap_or(usize::MAX);
    let starting_chars = output.chars().count();
    for part in &template.parts {
        match part {
            TemplatePart::Text { value } => output.push_str(value),
            TemplatePart::Variable { variable } => {
                let value = variables.get(variable).ok_or_else(|| {
                    InteractionFailure::new(
                        InteractionFailureCode::MissingVariable,
                        format!(
                            "template variable `{}` does not exist",
                            variable.id.as_str()
                        ),
                    )
                })?;
                output.push_str(&display_variable_value(value));
            }
            TemplatePart::BuiltIn { value } => {
                let value = built_in_value(*value, &context.template_values).ok_or_else(|| {
                    InteractionFailure::new(
                        InteractionFailureCode::MissingVariable,
                        "required interaction template built-in value was not provided",
                    )
                })?;
                output.push_str(value);
            }
            TemplatePart::Slot { name } => {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::InvalidTemplate,
                    format!("unresolved interaction template slot `{name}`"),
                ));
            }
            TemplatePart::Join {
                variable,
                separator,
            } => match variables.get(variable) {
                Some(VariableValue::StringList(values)) => {
                    for (index, value) in values.iter().enumerate() {
                        if index > 0 {
                            output.push_str(separator);
                        }
                        output.push_str(value);
                        enforce_output_limit(output, starting_chars, template_limit, limits)?;
                    }
                }
                Some(_) => {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::TypeMismatch,
                        format!(
                            "join template variable `{}` is not a string list",
                            variable.id.as_str()
                        ),
                    ));
                }
                None => {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::MissingVariable,
                        format!(
                            "join template variable `{}` does not exist",
                            variable.id.as_str()
                        ),
                    ));
                }
            },
            TemplatePart::Conditional {
                condition,
                then_template,
                else_template,
            } => {
                let branch = if evaluate_condition_ast(
                    condition,
                    ConditionContext {
                        variables,
                        model_capabilities: &context.model_capabilities,
                    },
                    limits,
                )? {
                    Some(then_template.as_ref())
                } else {
                    else_template.as_deref()
                };
                if let Some(branch) = branch {
                    render_template_into(branch, variables, context, limits, output)?;
                }
            }
        }
        enforce_output_limit(output, starting_chars, template_limit, limits)?;
    }
    Ok(())
}

fn enforce_output_limit(
    output: &str,
    starting_chars: usize,
    template_limit: usize,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    let chars = output.chars().count();
    if chars > limits.max_text_chars || chars.saturating_sub(starting_chars) > template_limit {
        return Err(InteractionFailure::new(
            InteractionFailureCode::InvalidTemplate,
            "interaction template exceeded its output limit",
        ));
    }
    Ok(())
}

fn built_in_value(value: BuiltInTemplateValue, values: &InteractionTemplateValues) -> Option<&str> {
    match value {
        BuiltInTemplateValue::CharacterName => values.character_name.as_deref(),
        BuiltInTemplateValue::UserName => values.user_name.as_deref(),
        BuiltInTemplateValue::PersonaName => values.persona_name.as_deref(),
        BuiltInTemplateValue::PersonaDescription => values.persona_description.as_deref(),
        BuiltInTemplateValue::CurrentDate => values.current_date.as_deref(),
        BuiltInTemplateValue::CurrentTime => values.current_time.as_deref(),
    }
}

fn display_variable_value(value: &VariableValue) -> String {
    match value {
        VariableValue::Bool(value) => value.to_string(),
        VariableValue::Integer(value) => value.to_string(),
        VariableValue::Decimal(value) => value.to_string(),
        VariableValue::Text(value) | VariableValue::Enum(value) => value.clone(),
        VariableValue::StringList(values) => values.join(", "),
    }
}

fn validate_event(
    event: &InteractionEvent,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    match event {
        InteractionEvent::UserAction { action_id } => {
            validate_identifier("action id", action_id, limits)
        }
        InteractionEvent::VariableChanged { variable } => validate_variable_ref(variable, limits),
        InteractionEvent::KnowledgeActivated { entry_id } => {
            validate_identifier("knowledge entry id", entry_id.as_str(), limits)
        }
        InteractionEvent::ConversationOpened
        | InteractionEvent::ConversationStarted
        | InteractionEvent::BeforeGeneration
        | InteractionEvent::AfterGeneration
        | InteractionEvent::MessageCommitted => Ok(()),
    }
}

fn validate_action(
    action: &InteractionAction,
    provenance: &lorepia_domain::Provenance,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    match action {
        InteractionAction::SetVariable { target, value } => {
            validate_write_target(target, provenance, limits)?;
            match value {
                ValueExpr::Literal { value } => validate_variable_value(value, limits),
                ValueExpr::Variable { variable } => validate_variable_ref(variable, limits),
            }
        }
        InteractionAction::IncrementVariable { target, .. } => {
            validate_write_target(target, provenance, limits)
        }
        InteractionAction::ActivateKnowledge { entry_id } => {
            validate_identifier("knowledge entry id", entry_id.as_str(), limits)
        }
        InteractionAction::ShowAsset { asset_id, .. }
        | InteractionAction::PlayAudio { asset_id } => {
            validate_identifier("asset id", asset_id.as_str(), limits)
        }
        InteractionAction::PresentChoices { choices } => {
            if choices.is_empty() || choices.len() > limits.max_choices {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::InvalidAction,
                    "choice count is empty or exceeds the configured limit",
                ));
            }
            let mut ids = BTreeSet::new();
            for choice in choices {
                validate_identifier("choice id", &choice.id, limits)?;
                validate_native_plain_text(&choice.label, limits)?;
                validate_variable_value(&choice.value, limits)?;
                if let Some(condition) = &choice.enabled_when {
                    validate_condition(condition, limits)?;
                }
                if !ids.insert(choice.id.as_str()) {
                    return Err(InteractionFailure::new(
                        InteractionFailureCode::InvalidAction,
                        format!("duplicate choice id `{}`", choice.id),
                    ));
                }
            }
            Ok(())
        }
        InteractionAction::AppendVisibleSystemEvent { text } => validate_template(text, limits),
        InteractionAction::RollDice { expression, target } => {
            validate_dice(expression, limits)?;
            if let Some(target) = target {
                validate_write_target(target, provenance, limits)?;
            }
            Ok(())
        }
        InteractionAction::RequestUserApproval { proposal } => {
            validate_identifier("proposal id", &proposal.id, limits)?;
            validate_required_native_plain_text(
                "proposal title",
                &proposal.title,
                MAX_INTERACTION_PROPOSAL_TITLE_CHARS,
                limits,
            )?;
            validate_template(&proposal.body, limits)?;
            if proposal.expires_after_seconds == Some(0) {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::InvalidAction,
                    "approval expiry must be greater than zero",
                ));
            }
            Ok(())
        }
    }
}

fn validate_dice(
    expression: &DiceExpression,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    if expression.count == 0
        || expression.count > limits.max_dice_count
        || expression.sides < 2
        || expression.sides > limits.max_dice_sides
    {
        return Err(InteractionFailure::new(
            InteractionFailureCode::InvalidAction,
            "dice expression is outside the configured count or side limits",
        ));
    }
    let maximum = i64::from(expression.count)
        .checked_mul(i64::from(expression.sides))
        .and_then(|value| value.checked_add(expression.modifier));
    let minimum = i64::from(expression.count).checked_add(expression.modifier);
    if maximum.is_none() || minimum.is_none() {
        return Err(InteractionFailure::new(
            InteractionFailureCode::InvalidAction,
            "dice expression total can overflow",
        ));
    }
    Ok(())
}

fn validate_write_target(
    target: &VariableRef,
    provenance: &lorepia_domain::Provenance,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    validate_variable_ref(target, limits)?;
    if matches!(
        provenance.source_kind,
        SourceKind::ImportedPackage | SourceKind::ImportedStandard
    ) {
        let expected = provenance.source_id.as_deref().ok_or_else(|| {
            InteractionFailure::new(
                InteractionFailureCode::ImportedScopeViolation,
                "imported rules that write variables require a provenance source ID",
            )
        })?;
        if target.scope != VariableScope::Module
            || target.namespace.as_ref().map(ContentModuleId::as_str) != Some(expected)
        {
            return Err(InteractionFailure::new(
                InteractionFailureCode::ImportedScopeViolation,
                "an imported rule may write only its own namespaced module variables",
            ));
        }
    }
    Ok(())
}

fn validate_variable_ref(
    variable: &VariableRef,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    validate_identifier("variable id", variable.id.as_str(), limits)?;
    if let Some(namespace) = &variable.namespace {
        validate_identifier("variable namespace", namespace.as_str(), limits)?;
    }
    if variable.scope == VariableScope::Module && variable.namespace.is_none() {
        return Err(InteractionFailure::new(
            InteractionFailureCode::InvalidIdentifier,
            "module variables require a namespace",
        ));
    }
    Ok(())
}

fn validate_variable_value(
    value: &VariableValue,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    match value {
        VariableValue::Decimal(value) if !value.is_finite() => Err(InteractionFailure::new(
            InteractionFailureCode::InvalidAction,
            "decimal variable values must be finite",
        )),
        VariableValue::Text(value) | VariableValue::Enum(value) => {
            validate_text("variable value", value, limits)
        }
        VariableValue::StringList(values) => {
            if values.len() > limits.max_choices {
                return Err(InteractionFailure::new(
                    InteractionFailureCode::InvalidAction,
                    "string-list variable exceeds the configured item limit",
                ));
            }
            for value in values {
                validate_text("string-list item", value, limits)?;
            }
            Ok(())
        }
        VariableValue::Bool(_) | VariableValue::Integer(_) | VariableValue::Decimal(_) => Ok(()),
    }
}

fn validate_identifier(
    label: &str,
    value: &str,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    if value.is_empty()
        || value.len() > limits.max_identifier_bytes
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(InteractionFailure::new(
            InteractionFailureCode::InvalidIdentifier,
            format!(
                "{label} must contain only ASCII letters, digits, `.`, `_`, or `-` and fit the configured limit"
            ),
        ));
    }
    Ok(())
}

fn validate_text(
    label: &str,
    value: &str,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    if value.chars().count() > limits.max_text_chars {
        return Err(InteractionFailure::new(
            InteractionFailureCode::UnsafeText,
            format!("{label} exceeds the configured text limit"),
        ));
    }
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(InteractionFailure::new(
            InteractionFailureCode::UnsafeText,
            format!("{label} contains disallowed control characters"),
        ));
    }
    Ok(())
}

fn validate_native_plain_text(
    value: &str,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    validate_text("rendered native text", value, limits)?;
    validate_interaction_native_text("rendered native text", value).map_err(|_| {
        InteractionFailure::new(
            InteractionFailureCode::UnsafeText,
            "interaction output violates the canonical native plain-text contract",
        )
    })
}

fn validate_required_native_plain_text(
    label: &str,
    value: &str,
    canonical_max_chars: usize,
    limits: InteractionLimits,
) -> Result<(), InteractionFailure> {
    validate_native_plain_text(value, limits)?;
    let characters = value.chars().count();
    if characters == 0 || characters > canonical_max_chars {
        return Err(InteractionFailure::new(
            InteractionFailureCode::UnsafeText,
            format!("{label} must contain between 1 and {canonical_max_chars} characters"),
        ));
    }
    Ok(())
}

/// Existing state may contain proposal text committed under the historical
/// 16-Ki-scalar contract. Keep it processable so read/reopen paths can project
/// the record as a typed redaction; every newly rendered proposal still uses
/// `validate_required_native_plain_text` and the canonical native boundary.
fn validate_legacy_stored_proposal_text(
    label: &str,
    value: &str,
    maximum_chars: usize,
) -> Result<(), InteractionFailure> {
    let characters = value.chars().count();
    if characters == 0 || characters > maximum_chars {
        return Err(InteractionFailure::new(
            InteractionFailureCode::UnsafeText,
            format!("{label} must contain between 1 and {maximum_chars} characters"),
        ));
    }
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(InteractionFailure::new(
            InteractionFailureCode::UnsafeText,
            format!("{label} contains disallowed control characters"),
        ));
    }
    let normalized = value.to_ascii_lowercase();
    if (normalized.contains('<') && normalized.contains('>'))
        || normalized.contains("javascript:")
        || normalized.contains("data:text/html")
    {
        return Err(InteractionFailure::new(
            InteractionFailureCode::UnsafeText,
            format!("{label} must remain plain native text"),
        ));
    }
    Ok(())
}

fn rule_provenance_is_approved(
    set_provenance: &lorepia_domain::Provenance,
    rule_provenance: &lorepia_domain::Provenance,
    options: &InteractionCompileOptions,
) -> Result<bool, InteractionFailure> {
    let set_source = imported_source_id(set_provenance)?;
    let rule_source = imported_source_id(rule_provenance)?;
    if let Some(set_source) = set_source {
        if rule_source != Some(set_source) {
            return Err(InteractionFailure::new(
                InteractionFailureCode::ImportedScopeViolation,
                "an imported interaction rule set may contain only rules from the same imported source",
            ));
        }
        return Ok(options.approved_import_source_ids.contains(set_source));
    }
    Ok(rule_source.is_none_or(|source| options.approved_import_source_ids.contains(source)))
}

fn imported_source_id(
    provenance: &lorepia_domain::Provenance,
) -> Result<Option<&str>, InteractionFailure> {
    if matches!(
        provenance.source_kind,
        SourceKind::ImportedPackage | SourceKind::ImportedStandard
    ) {
        return provenance.source_id.as_deref().map(Some).ok_or_else(|| {
            InteractionFailure::new(
                InteractionFailureCode::InvalidIdentifier,
                "imported interaction content requires a provenance source ID",
            )
        });
    }
    Ok(None)
}

fn resolve_value<'a>(
    expression: &'a ValueExpr,
    variables: &'a VariableMap,
) -> Result<&'a VariableValue, InteractionFailure> {
    match expression {
        ValueExpr::Literal { value } => Ok(value),
        ValueExpr::Variable { variable } => variables.get(variable).ok_or_else(|| {
            InteractionFailure::new(
                InteractionFailureCode::MissingVariable,
                format!("source variable `{}` does not exist", variable.id.as_str()),
            )
        }),
    }
}

fn set_variable(
    state: &mut InteractionState,
    effects: &mut Vec<InteractionEffect>,
    target: &VariableRef,
    value: VariableValue,
) {
    let previous = state.variables.get(target).cloned();
    state.variables.insert(target.clone(), value.clone());
    if previous.as_ref() != Some(&value) {
        effects.push(InteractionEffect::VariableSet {
            target: target.clone(),
            previous,
            value,
        });
    }
}

fn rule_trace(rule: &InteractionRule, status: InteractionRuleStatus) -> InteractionRuleTrace {
    InteractionRuleTrace {
        rule_id: rule.id.clone(),
        status,
        action_count: 0,
        effect_count: 0,
        state_changed: false,
        error: None,
    }
}

fn failed_trace(rule: &InteractionRule, error: InteractionFailure) -> InteractionRuleTrace {
    InteractionRuleTrace {
        rule_id: rule.id.clone(),
        status: InteractionRuleStatus::Failed,
        action_count: 0,
        effect_count: 0,
        state_changed: false,
        error: Some(error),
    }
}

fn event_seed(seed: u64, event: &InteractionEvent, revision: u64) -> u64 {
    let discriminant = match event {
        InteractionEvent::ConversationOpened => 1,
        InteractionEvent::ConversationStarted => 2,
        InteractionEvent::BeforeGeneration => 3,
        InteractionEvent::AfterGeneration => 4,
        InteractionEvent::MessageCommitted => 5,
        InteractionEvent::UserAction { .. } => 6,
        InteractionEvent::VariableChanged { .. } => 7,
        InteractionEvent::KnowledgeActivated { .. } => 8,
    };
    let mut mixed = mix_seed(seed, discriminant);
    mixed = mix_seed(mixed, revision);
    match event {
        InteractionEvent::UserAction { action_id } => {
            mix_seed(mixed, stable_hash(action_id.as_bytes()))
        }
        InteractionEvent::VariableChanged { variable } => {
            let namespace = variable
                .namespace
                .as_ref()
                .map_or(0, |value| stable_hash(value.as_str().as_bytes()));
            mix_seed(
                mix_seed(
                    mix_seed(mixed, variable_scope_seed(variable.scope)),
                    namespace,
                ),
                stable_hash(variable.id.as_str().as_bytes()),
            )
        }
        InteractionEvent::KnowledgeActivated { entry_id } => {
            mix_seed(mixed, stable_hash(entry_id.as_str().as_bytes()))
        }
        InteractionEvent::ConversationOpened
        | InteractionEvent::ConversationStarted
        | InteractionEvent::BeforeGeneration
        | InteractionEvent::AfterGeneration
        | InteractionEvent::MessageCommitted => mixed,
    }
}

const fn variable_scope_seed(scope: VariableScope) -> u64 {
    match scope {
        VariableScope::App => 1,
        VariableScope::User => 2,
        VariableScope::Persona => 3,
        VariableScope::Character => 4,
        VariableScope::Conversation => 5,
        VariableScope::Branch => 6,
        VariableScope::Session => 7,
        VariableScope::Turn => 8,
        VariableScope::Module => 9,
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

const fn mix_seed(left: u64, right: u64) -> u64 {
    left ^ right
        .wrapping_add(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(left << 6)
        .wrapping_add(left >> 2)
}

fn roll_dice(
    expression: &DiceExpression,
    seed: u64,
) -> Result<(Vec<u32>, i64), InteractionFailure> {
    let mut generator = SplitMix64::new(seed);
    let mut rolls = Vec::with_capacity(usize::from(expression.count));
    let mut total = expression.modifier;
    for _ in 0..expression.count {
        let roll = generator.next_bounded(expression.sides) + 1;
        total = total.checked_add(i64::from(roll)).ok_or_else(|| {
            InteractionFailure::new(
                InteractionFailureCode::NumericOverflow,
                "dice total overflowed",
            )
        })?;
        rolls.push(roll);
    }
    Ok((rolls, total))
}

#[derive(Debug, Clone, Copy)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn next_bounded(&mut self, upper_exclusive: u32) -> u32 {
        let upper = u64::from(upper_exclusive);
        let rejection_limit = u64::MAX - (u64::MAX % upper);
        loop {
            let value = self.next();
            if value < rejection_limit {
                return u32::try_from(value % upper).unwrap_or(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{
        AssetId, ContentModuleId, InteractionRuleSetId, KnowledgeEntryId, Provenance, SafeTemplate,
        SourceKind, TemplatePart, UiRegion, VariableBinding, VariableId,
    };

    use super::*;

    fn provenance(source_kind: SourceKind, source_id: Option<&str>) -> Provenance {
        Provenance {
            source_kind,
            source_id: source_id.map(str::to_owned),
            source_hash: None,
            author: None,
            license: None,
            imported_at: None,
        }
    }

    fn variable(scope: VariableScope, namespace: Option<&str>, id: &str) -> VariableRef {
        VariableRef {
            scope,
            namespace: namespace.map(ContentModuleId::from),
            id: VariableId::from(id),
        }
    }

    fn template(text: &str) -> SafeTemplate {
        SafeTemplate {
            parts: vec![TemplatePart::Text {
                value: text.to_owned(),
            }],
            max_output_chars: 1_024,
        }
    }

    fn approval_action(proposal_id: &str, expires_after_seconds: Option<u32>) -> InteractionAction {
        InteractionAction::RequestUserApproval {
            proposal: lorepia_domain::ProposalSpec {
                id: proposal_id.to_owned(),
                title: format!("Approve {proposal_id}"),
                body: template("Review this proposal."),
                expires_after_seconds,
            },
        }
    }

    fn rule(id: &str, event: InteractionEvent, actions: Vec<InteractionAction>) -> InteractionRule {
        InteractionRule {
            id: InteractionRuleId::from(id),
            name: id.to_owned(),
            enabled: true,
            imported_author_enabled: false,
            event,
            condition: None,
            actions,
            priority: 0,
            stop_after_match: false,
            provenance: provenance(SourceKind::UserCreated, None),
        }
    }

    fn rule_set(rules: Vec<InteractionRule>) -> InteractionRuleSet {
        InteractionRuleSet {
            id: InteractionRuleSetId::from("rules"),
            name: "Rules".to_owned(),
            schema_version: 1,
            rules,
            max_actions_per_event: 128,
            provenance: provenance(SourceKind::UserCreated, None),
        }
    }

    #[test]
    fn matching_rules_apply_once_in_deterministic_order() {
        let score = variable(VariableScope::Conversation, None, "score");
        let rules = vec![
            InteractionRule {
                priority: 20,
                ..rule(
                    "later",
                    InteractionEvent::MessageCommitted,
                    vec![InteractionAction::IncrementVariable {
                        target: score.clone(),
                        amount: 10,
                    }],
                )
            },
            InteractionRule {
                priority: 10,
                ..rule(
                    "earlier",
                    InteractionEvent::MessageCommitted,
                    vec![InteractionAction::SetVariable {
                        target: score.clone(),
                        value: ValueExpr::Literal {
                            value: VariableValue::Integer(1),
                        },
                    }],
                )
            },
        ];
        let engine = InteractionEngine::compile(&[rule_set(rules)], InteractionLimits::default())
            .expect("compile");

        let result = engine
            .handle_event(
                &InteractionState {
                    variables: VariableMap::default(),
                    manually_active_knowledge: Vec::new(),
                    proposals: Vec::new(),
                    revision: 0,
                },
                &InteractionEvent::MessageCommitted,
                &InteractionContext::default(),
            )
            .expect("handle event");

        assert_eq!(
            result.state.variables.get(&score),
            Some(&VariableValue::Integer(11))
        );
        assert_eq!(
            result
                .trace
                .iter()
                .map(|entry| entry.rule_id.as_str())
                .collect::<Vec<_>>(),
            vec!["earlier", "later"]
        );
        assert_eq!(result.state.revision, 1);
    }

    #[test]
    fn choice_projection_keeps_only_enabled_normal_choices() {
        let choices = vec![
            lorepia_domain::ChoiceSpec {
                id: "visible-choice".to_owned(),
                label: "표시 선택지".to_owned(),
                value: VariableValue::Bool(true),
                enabled_when: Some(ConditionExpr::True),
            },
            lorepia_domain::ChoiceSpec {
                id: "hidden-choice".to_owned(),
                label: "숨김 선택지".to_owned(),
                value: VariableValue::Bool(false),
                enabled_when: Some(ConditionExpr::False),
            },
        ];
        let engine = InteractionEngine::compile(
            &[rule_set(vec![rule(
                "normal-choices",
                InteractionEvent::ConversationOpened,
                vec![InteractionAction::PresentChoices { choices }],
            )])],
            InteractionLimits::default(),
        )
        .expect("normal choices compile");
        let outcome = engine
            .handle_event(
                &InteractionState {
                    variables: VariableMap::default(),
                    manually_active_knowledge: Vec::new(),
                    proposals: Vec::new(),
                    revision: 0,
                },
                &InteractionEvent::ConversationOpened,
                &InteractionContext::default(),
            )
            .expect("normal choice projection applies");

        let InteractionEffect::ChoicesPresented { choices } = &outcome.effects[0] else {
            panic!("normal choices must produce a choice effect");
        };
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, "visible-choice");
    }

    #[test]
    fn failed_rule_is_transactional_and_later_rule_continues() {
        let score = variable(VariableScope::Conversation, None, "score");
        let missing = variable(VariableScope::Conversation, None, "missing");
        let engine = InteractionEngine::compile(
            &[rule_set(vec![
                rule(
                    "fails",
                    InteractionEvent::BeforeGeneration,
                    vec![
                        InteractionAction::SetVariable {
                            target: score.clone(),
                            value: ValueExpr::Literal {
                                value: VariableValue::Integer(5),
                            },
                        },
                        InteractionAction::SetVariable {
                            target: score.clone(),
                            value: ValueExpr::Variable { variable: missing },
                        },
                    ],
                ),
                InteractionRule {
                    priority: 1,
                    ..rule(
                        "continues",
                        InteractionEvent::BeforeGeneration,
                        vec![InteractionAction::SetVariable {
                            target: score.clone(),
                            value: ValueExpr::Literal {
                                value: VariableValue::Integer(9),
                            },
                        }],
                    )
                },
            ])],
            InteractionLimits::default(),
        )
        .expect("compile");
        let initial = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: Vec::new(),
            proposals: Vec::new(),
            revision: 0,
        };
        let result = engine
            .handle_event(
                &initial,
                &InteractionEvent::BeforeGeneration,
                &InteractionContext::default(),
            )
            .expect("handle");

        assert_eq!(
            result.state.variables.get(&score),
            Some(&VariableValue::Integer(9))
        );
        assert_eq!(result.trace[0].status, InteractionRuleStatus::Failed);
        assert_eq!(result.trace[0].effect_count, 0);
        assert_eq!(result.trace[1].status, InteractionRuleStatus::Applied);
        assert_eq!(result.effects.len(), 1);
    }

    #[test]
    fn imported_rules_are_inert_until_exact_source_is_approved() {
        let score = variable(VariableScope::Module, Some("safe_module"), "score");
        let mut imported = rule(
            "imported",
            InteractionEvent::ConversationStarted,
            vec![InteractionAction::SetVariable {
                target: score.clone(),
                value: ValueExpr::Literal {
                    value: VariableValue::Integer(1),
                },
            }],
        );
        imported.provenance = provenance(SourceKind::ImportedPackage, Some("safe_module"));
        let mut set = rule_set(vec![imported]);
        set.provenance = provenance(SourceKind::ImportedPackage, Some("safe_module"));
        let engine =
            InteractionEngine::compile(std::slice::from_ref(&set), InteractionLimits::default())
                .expect("pending import compiles");
        let initial = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: Vec::new(),
            proposals: Vec::new(),
            revision: 0,
        };
        let pending = engine
            .handle_event(
                &initial,
                &InteractionEvent::ConversationStarted,
                &InteractionContext::default(),
            )
            .expect("pending");
        assert!(pending.state.variables.values.is_empty());
        assert_eq!(
            pending.trace[0].status,
            InteractionRuleStatus::PendingImportApproval
        );

        let engine = InteractionEngine::compile_with_options(
            &[set],
            InteractionLimits::default(),
            &InteractionCompileOptions {
                approved_import_source_ids: BTreeSet::from(["safe_module".to_owned()]),
            },
        )
        .expect("approved compile");
        let applied = engine
            .handle_event(
                &initial,
                &InteractionEvent::ConversationStarted,
                &InteractionContext::default(),
            )
            .expect("applied");
        assert_eq!(
            applied.state.variables.get(&score),
            Some(&VariableValue::Integer(1))
        );
    }

    #[test]
    fn imported_write_families_preserve_exact_own_module_authority() {
        for source_kind in [SourceKind::ImportedPackage, SourceKind::ImportedStandard] {
            let target = variable(VariableScope::Module, Some("safe_module"), "score");
            let actions = [
                InteractionAction::SetVariable {
                    target: target.clone(),
                    value: ValueExpr::Literal {
                        value: VariableValue::Integer(1),
                    },
                },
                InteractionAction::IncrementVariable {
                    target: target.clone(),
                    amount: 1,
                },
                InteractionAction::RollDice {
                    expression: DiceExpression {
                        count: 1,
                        sides: 6,
                        modifier: 0,
                    },
                    target: Some(target.clone()),
                },
            ];
            for (index, action) in actions.into_iter().enumerate() {
                let mut imported = rule(
                    &format!("imported-{index}"),
                    InteractionEvent::ConversationStarted,
                    vec![action],
                );
                imported.provenance = provenance(source_kind.clone(), Some("safe_module"));
                let mut set = rule_set(vec![imported]);
                set.provenance = provenance(source_kind.clone(), Some("safe_module"));
                let engine = InteractionEngine::compile_with_options(
                    &[set],
                    InteractionLimits::default(),
                    &InteractionCompileOptions {
                        approved_import_source_ids: BTreeSet::from(["safe_module".to_owned()]),
                    },
                )
                .expect("exact own-module write compiles");
                let outcome = engine
                    .handle_event(
                        &InteractionState {
                            variables: VariableMap::default(),
                            manually_active_knowledge: Vec::new(),
                            proposals: Vec::new(),
                            revision: 0,
                        },
                        &InteractionEvent::ConversationStarted,
                        &InteractionContext {
                            deterministic_seed: 7,
                            ..InteractionContext::default()
                        },
                    )
                    .expect("exact own-module write executes");
                assert!(outcome.state.variables.get(&target).is_some());
                assert_eq!(outcome.trace[0].status, InteractionRuleStatus::Applied);
            }
        }
    }

    #[test]
    fn user_created_write_authority_remains_scope_agnostic() {
        for (index, scope) in [
            VariableScope::App,
            VariableScope::User,
            VariableScope::Persona,
            VariableScope::Character,
            VariableScope::Conversation,
            VariableScope::Branch,
            VariableScope::Session,
            VariableScope::Turn,
            VariableScope::Module,
        ]
        .into_iter()
        .enumerate()
        {
            let namespace = (scope == VariableScope::Module).then_some("local_module");
            let target = variable(scope, namespace, &format!("value-{index}"));
            InteractionEngine::compile(
                &[rule_set(vec![rule(
                    &format!("user-write-{index}"),
                    InteractionEvent::ConversationOpened,
                    vec![InteractionAction::SetVariable {
                        target,
                        value: ValueExpr::Literal {
                            value: VariableValue::Integer(1),
                        },
                    }],
                )])],
                InteractionLimits::default(),
            )
            .expect("user-created rules retain their existing write scopes");
        }
    }

    #[test]
    fn imported_module_cannot_write_another_module_namespace() {
        let mut imported = rule(
            "escape",
            InteractionEvent::ConversationStarted,
            vec![InteractionAction::SetVariable {
                target: variable(VariableScope::Module, Some("other_module"), "score"),
                value: ValueExpr::Literal {
                    value: VariableValue::Integer(1),
                },
            }],
        );
        imported.provenance = provenance(SourceKind::ImportedPackage, Some("safe_module"));
        let error =
            InteractionEngine::compile(&[rule_set(vec![imported])], InteractionLimits::default())
                .expect_err("namespace escape must fail");
        assert_eq!(error.code, InteractionFailureCode::ImportedScopeViolation);
    }

    #[test]
    fn imported_set_cannot_disguise_a_child_rule_as_user_authored() {
        let mut imported_set = rule_set(vec![rule(
            "disguised",
            InteractionEvent::ConversationOpened,
            Vec::new(),
        )]);
        imported_set.provenance = provenance(SourceKind::ImportedPackage, Some("hostile_module"));
        let error = InteractionEngine::compile(&[imported_set], InteractionLimits::default())
            .expect_err("import provenance mismatch must fail");
        assert_eq!(error.code, InteractionFailureCode::ImportedScopeViolation);
    }

    #[test]
    fn action_budgets_are_enforced_per_set_and_globally() {
        let mut first = rule_set(vec![rule(
            "first",
            InteractionEvent::ConversationOpened,
            vec![InteractionAction::ActivateKnowledge {
                entry_id: KnowledgeEntryId::from("one"),
            }],
        )]);
        first.id = InteractionRuleSetId::from("first-set");
        first.max_actions_per_event = 1;
        let mut second = rule_set(vec![rule(
            "second",
            InteractionEvent::ConversationOpened,
            vec![InteractionAction::ActivateKnowledge {
                entry_id: KnowledgeEntryId::from("two"),
            }],
        )]);
        second.id = InteractionRuleSetId::from("second-set");
        second.max_actions_per_event = 1;

        let engine = InteractionEngine::compile(&[first, second], InteractionLimits::default())
            .expect("distinct per-set budgets compile");
        let outcome = engine
            .handle_event(
                &InteractionState {
                    variables: VariableMap::default(),
                    manually_active_knowledge: Vec::new(),
                    proposals: Vec::new(),
                    revision: 0,
                },
                &InteractionEvent::ConversationOpened,
                &InteractionContext::default(),
            )
            .expect("both sets execute within their own budget");
        assert_eq!(outcome.effects.len(), 2);
        assert!(
            outcome
                .trace
                .iter()
                .all(|entry| entry.status == InteractionRuleStatus::Applied)
        );
    }

    #[test]
    fn duplicate_rule_set_ids_are_rejected() {
        let first = rule_set(vec![rule(
            "first",
            InteractionEvent::ConversationOpened,
            Vec::new(),
        )]);
        let second = rule_set(vec![rule(
            "second",
            InteractionEvent::ConversationOpened,
            Vec::new(),
        )]);
        let error = InteractionEngine::compile(&[first, second], InteractionLimits::default())
            .expect_err("duplicate set IDs must fail");
        assert_eq!(error.code, InteractionFailureCode::DuplicateRuleSetId);
    }

    #[test]
    fn dice_is_deterministic_and_writes_only_validated_target() {
        let target = variable(VariableScope::Conversation, None, "roll");
        let engine = InteractionEngine::compile(
            &[rule_set(vec![rule(
                "dice",
                InteractionEvent::UserAction {
                    action_id: "roll".to_owned(),
                },
                vec![InteractionAction::RollDice {
                    expression: DiceExpression {
                        count: 4,
                        sides: 6,
                        modifier: 2,
                    },
                    target: Some(target.clone()),
                }],
            )])],
            InteractionLimits::default(),
        )
        .expect("compile");
        let initial = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: Vec::new(),
            proposals: Vec::new(),
            revision: 0,
        };
        let event = InteractionEvent::UserAction {
            action_id: "roll".to_owned(),
        };
        let context = InteractionContext {
            deterministic_seed: 42,
            ..InteractionContext::default()
        };
        let first = engine
            .handle_event(&initial, &event, &context)
            .expect("first");
        let second = engine
            .handle_event(&initial, &event, &context)
            .expect("second");
        assert_eq!(first.effects, second.effects);

        let dice_effect = first
            .effects
            .iter()
            .find_map(|effect| match effect {
                InteractionEffect::DiceRolled { rolls, total, .. } => Some((rolls, total)),
                _ => None,
            })
            .expect("dice effect");
        assert_eq!(dice_effect.0.len(), 4);
        assert!(dice_effect.0.iter().all(|roll| (1..=6).contains(roll)));
        assert_eq!(
            *dice_effect.1,
            dice_effect
                .0
                .iter()
                .map(|roll| i64::from(*roll))
                .sum::<i64>()
                + 2
        );
        assert_eq!(
            first.state.variables.get(&target),
            Some(&VariableValue::Integer(*dice_effect.1))
        );
    }

    fn approval_engine() -> (InteractionEngine, VariableRef) {
        let flag = variable(VariableScope::Conversation, None, "approved");
        let request = rule(
            "request",
            InteractionEvent::UserAction {
                action_id: "ask".to_owned(),
            },
            vec![InteractionAction::RequestUserApproval {
                proposal: lorepia_domain::ProposalSpec {
                    id: "proposal-1".to_owned(),
                    title: "Change state".to_owned(),
                    body: template("Allow this state change?"),
                    expires_after_seconds: Some(60),
                },
            }],
        );
        let approve = rule(
            "approve",
            InteractionEvent::UserAction {
                action_id: "proposal-1".to_owned(),
            },
            vec![InteractionAction::SetVariable {
                target: flag.clone(),
                value: ValueExpr::Literal {
                    value: VariableValue::Bool(true),
                },
            }],
        );
        let engine = InteractionEngine::compile(
            &[rule_set(vec![request, approve])],
            InteractionLimits::default(),
        )
        .expect("compile");
        (engine, flag)
    }

    fn request_approval(engine: &InteractionEngine) -> InteractionOutcome {
        let initial = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: Vec::new(),
            proposals: Vec::new(),
            revision: 0,
        };
        engine
            .handle_event(
                &initial,
                &InteractionEvent::UserAction {
                    action_id: "ask".to_owned(),
                },
                &InteractionContext {
                    event_epoch_seconds: 100,
                    ..InteractionContext::default()
                },
            )
            .expect("request")
    }

    #[test]
    fn approval_is_durable_cas_bound_and_dispatches_one_validated_user_action() {
        let (engine, flag) = approval_engine();
        let requested = request_approval(&engine);
        assert!(requested.state_changed);
        assert_eq!(requested.state.revision, 1);
        assert_eq!(requested.state.proposals.len(), 1);
        assert_eq!(
            requested.state.proposals[0].status,
            InteractionProposalStatus::Pending
        );
        assert_eq!(
            requested.state.proposals[0].expires_at_epoch_seconds,
            Some(160)
        );
        assert!(matches!(
            requested.effects.first(),
            Some(InteractionEffect::ApprovalRequested {
                rule_set_id,
                rule_id,
                proposal_id,
                expires_after_seconds: Some(60),
                ..
            }) if rule_set_id.as_str() == "rules"
                && rule_id.as_str() == "request"
                && proposal_id == "proposal-1"
        ));

        let decision = approve_pending(
            &requested.state,
            "proposal-1",
            requested.state.revision,
            120,
        )
        .expect("approve exact pending proposal");
        assert_eq!(decision.state.revision, 2);
        assert_eq!(
            decision.proposal.status,
            InteractionProposalStatus::Approved
        );
        assert_eq!(decision.proposal.decided_at_epoch_seconds, Some(120));
        let dispatch = decision
            .event_after_commit
            .as_ref()
            .expect("approval returns one post-commit event");
        assert_eq!(
            dispatch,
            &InteractionEvent::UserAction {
                action_id: "proposal-1".to_owned(),
            }
        );

        let approved = engine
            .handle_event(
                &decision.state,
                dispatch,
                &InteractionContext {
                    event_epoch_seconds: 120,
                    ..InteractionContext::default()
                },
            )
            .expect("approved action");
        assert_eq!(
            approved.state.variables.get(&flag),
            Some(&VariableValue::Bool(true))
        );
        let replay = approve_pending(&decision.state, "proposal-1", decision.state.revision, 121)
            .expect_err("a decided proposal cannot be replayed");
        assert_eq!(replay.code, InteractionFailureCode::ProposalNotPending);
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one lifecycle test proves decision, expiry, replay, and re-request semantics"
    )]
    fn proposal_decision_enforces_revision_expiry_and_inert_rejection() {
        let engine = InteractionEngine::compile(
            &[rule_set(vec![rule(
                "request",
                InteractionEvent::ConversationOpened,
                vec![approval_action("proposal-expiring", Some(60))],
            )])],
            InteractionLimits::default(),
        )
        .expect("compile");
        let initial = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: Vec::new(),
            proposals: Vec::new(),
            revision: 0,
        };
        let context = InteractionContext {
            event_epoch_seconds: 100,
            ..InteractionContext::default()
        };
        let requested = engine
            .handle_event(&initial, &InteractionEvent::ConversationOpened, &context)
            .expect("request");
        let repeated = engine
            .handle_event(&initial, &InteractionEvent::ConversationOpened, &context)
            .expect("same input is deterministic");
        assert_eq!(
            requested.state.proposals[0].id,
            repeated.state.proposals[0].id
        );

        let stale = approve_pending(&requested.state, "proposal-expiring", 0, 120)
            .expect_err("stale revision must fail");
        assert_eq!(stale.code, InteractionFailureCode::RevisionConflict);
        let expired = approve_pending(
            &requested.state,
            "proposal-expiring",
            requested.state.revision,
            160,
        )
        .expect_err("expiration is exclusive");
        assert_eq!(expired.code, InteractionFailureCode::ProposalExpired);
        let mut two_due = requested.state.clone();
        let mut neighboring = two_due.proposals[0].clone();
        neighboring.proposal_id = "proposal-neighbor".to_owned();
        neighboring.id = proposal_record_id(
            &neighboring.rule_set_id,
            &neighboring.rule_id,
            &neighboring.proposal_id,
            neighboring.source_interaction_state_revision,
        )
        .expect("derive neighboring proposal record");
        two_due.proposals.push(neighboring);
        let one_expired =
            expire_pending_proposal(&two_due, "proposal-expiring", two_due.revision, 160)
                .expect("expire one exact proposal");
        assert_eq!(one_expired.expired_proposals.len(), 1);
        assert_eq!(one_expired.state.revision, 2);
        assert_eq!(
            one_expired.state.proposals[1].status,
            InteractionProposalStatus::Pending,
            "attempt aggregate expiry must not terminalize a neighboring due proposal"
        );
        let expired_state =
            expire_pending_proposals(&requested.state, requested.state.revision, 160)
                .expect("expiry transition");
        assert_eq!(expired_state.state.revision, 2);
        assert_eq!(expired_state.expired_proposals.len(), 1);
        assert_eq!(
            expired_state.expired_proposals[0].status,
            InteractionProposalStatus::Expired
        );
        assert_eq!(
            expired_state.expired_proposals[0].decided_at_epoch_seconds,
            Some(160)
        );
        let replay = approve_pending(
            &expired_state.state,
            "proposal-expiring",
            expired_state.state.revision,
            161,
        )
        .expect_err("expired proposal is terminal");
        assert_eq!(replay.code, InteractionFailureCode::ProposalNotPending);
        let before_deadline =
            expire_pending_proposals(&requested.state, requested.state.revision, 159)
                .expect("no-op before deadline");
        assert!(before_deadline.expired_proposals.is_empty());
        assert_eq!(before_deadline.state, requested.state);

        let rejected = reject_pending(
            &requested.state,
            "proposal-expiring",
            requested.state.revision,
            159,
        )
        .expect("reject before expiry");
        assert_eq!(rejected.state.revision, 2);
        assert_eq!(
            rejected.proposal.status,
            InteractionProposalStatus::Rejected
        );
        assert_eq!(rejected.proposal.decided_at_epoch_seconds, Some(159));
        assert!(rejected.event_after_commit.is_none());
        assert_eq!(rejected.state.proposals.len(), 1);

        let requested_again = engine
            .handle_event(
                &rejected.state,
                &InteractionEvent::ConversationOpened,
                &InteractionContext {
                    event_epoch_seconds: 200,
                    ..InteractionContext::default()
                },
            )
            .expect("a decided proposal ID may be requested again");
        assert_eq!(requested_again.state.revision, 3);
        assert_eq!(requested_again.state.proposals.len(), 2);
        assert_ne!(
            requested_again.state.proposals[0].id,
            requested_again.state.proposals[1].id
        );
        let approved_again = approve_pending(
            &requested_again.state,
            "proposal-expiring",
            requested_again.state.revision,
            220,
        )
        .expect("the pending record is selected ahead of historic records");
        assert_eq!(
            approved_again.proposal.status,
            InteractionProposalStatus::Approved
        );
        assert_eq!(
            approved_again.state.proposals[0].status,
            InteractionProposalStatus::Rejected
        );
        assert_eq!(
            approved_again.state.proposals[1].status,
            InteractionProposalStatus::Approved
        );
    }

    #[test]
    fn duplicate_and_pending_limit_failures_are_transactional() {
        let mut duplicate_second = rule(
            "duplicate-second",
            InteractionEvent::ConversationOpened,
            vec![approval_action("same-proposal", None)],
        );
        duplicate_second.priority = 1;
        let duplicate_engine = InteractionEngine::compile(
            &[rule_set(vec![
                rule(
                    "duplicate-first",
                    InteractionEvent::ConversationOpened,
                    vec![approval_action("same-proposal", None)],
                ),
                duplicate_second,
            ])],
            InteractionLimits::default(),
        )
        .expect("compile duplicates for runtime replay guard");
        let initial = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: Vec::new(),
            proposals: Vec::new(),
            revision: 0,
        };
        let outcome = duplicate_engine
            .handle_event(
                &initial,
                &InteractionEvent::ConversationOpened,
                &InteractionContext {
                    event_epoch_seconds: 10,
                    ..InteractionContext::default()
                },
            )
            .expect("event continues after transactional duplicate failure");
        assert_eq!(outcome.state.proposals.len(), 1);
        assert_eq!(outcome.effects.len(), 1);
        assert_eq!(outcome.trace[0].status, InteractionRuleStatus::Applied);
        assert_eq!(outcome.trace[1].status, InteractionRuleStatus::Failed);
        assert_eq!(
            outcome.trace[1].error.as_ref().map(|error| &error.code),
            Some(&InteractionFailureCode::DuplicateProposal)
        );

        let mut second = rule(
            "second",
            InteractionEvent::ConversationOpened,
            vec![approval_action("proposal-two", None)],
        );
        second.priority = 1;
        let limited_engine = InteractionEngine::compile(
            &[rule_set(vec![
                rule(
                    "first",
                    InteractionEvent::ConversationOpened,
                    vec![approval_action("proposal-one", None)],
                ),
                second,
            ])],
            InteractionLimits {
                max_pending_proposals: 1,
                ..InteractionLimits::default()
            },
        )
        .expect("compile pending limit");
        let limited = limited_engine
            .handle_event(
                &initial,
                &InteractionEvent::ConversationOpened,
                &InteractionContext {
                    event_epoch_seconds: 10,
                    ..InteractionContext::default()
                },
            )
            .expect("pending limit is a per-rule failure");
        assert_eq!(limited.state.proposals.len(), 1);
        assert_eq!(
            limited.trace[1].error.as_ref().map(|error| &error.code),
            Some(&InteractionFailureCode::ProposalLimitExceeded)
        );
    }

    #[test]
    fn proposal_records_enforce_canonical_text_and_knowledge_limits() {
        let oversized_title = InteractionAction::RequestUserApproval {
            proposal: lorepia_domain::ProposalSpec {
                id: "oversized-title".to_owned(),
                title: "x".repeat(MAX_INTERACTION_PROPOSAL_TITLE_CHARS + 1),
                body: template("Review this proposal."),
                expires_after_seconds: None,
            },
        };
        let title_error = InteractionEngine::compile(
            &[rule_set(vec![rule(
                "oversized-title",
                InteractionEvent::ConversationOpened,
                vec![oversized_title],
            )])],
            InteractionLimits::default(),
        )
        .expect_err("proposal titles must remain domain-persistable");
        assert_eq!(title_error.code, InteractionFailureCode::UnsafeText);

        let empty_body = InteractionAction::RequestUserApproval {
            proposal: lorepia_domain::ProposalSpec {
                id: "empty-body".to_owned(),
                title: "Review".to_owned(),
                body: template(""),
                expires_after_seconds: None,
            },
        };
        let engine = InteractionEngine::compile(
            &[rule_set(vec![rule(
                "empty-body",
                InteractionEvent::ConversationOpened,
                vec![empty_body],
            )])],
            InteractionLimits::default(),
        )
        .expect("the rendered body is validated transactionally");
        let outcome = engine
            .handle_event(
                &InteractionState {
                    variables: VariableMap::default(),
                    manually_active_knowledge: Vec::new(),
                    proposals: Vec::new(),
                    revision: 0,
                },
                &InteractionEvent::ConversationOpened,
                &InteractionContext::default(),
            )
            .expect("invalid proposal text is a per-rule failure");
        assert!(outcome.state.proposals.is_empty());
        assert!(outcome.effects.is_empty());
        assert_eq!(outcome.trace[0].status, InteractionRuleStatus::Failed);
        assert_eq!(
            outcome.trace[0].error.as_ref().map(|error| &error.code),
            Some(&InteractionFailureCode::UnsafeText)
        );

        let too_much_knowledge = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: (0..=lorepia_domain::MAX_KNOWLEDGE_ENTRIES)
                .map(|index| KnowledgeEntryId::from(format!("entry-{index}")))
                .collect(),
            proposals: Vec::new(),
            revision: 0,
        };
        let state_error = InteractionEngine::compile(&[], InteractionLimits::default())
            .expect("empty engine")
            .handle_event(
                &too_much_knowledge,
                &InteractionEvent::ConversationOpened,
                &InteractionContext::default(),
            )
            .expect_err("knowledge count must match the domain limit");
        assert_eq!(state_error.code, InteractionFailureCode::StateLimitExceeded);
    }

    #[test]
    fn native_text_rejects_html_after_variable_interpolation() {
        let text = variable(VariableScope::Conversation, None, "text");
        let mut variables = VariableMap {
            values: vec![VariableBinding {
                variable: text.clone(),
                value: VariableValue::Text("<script>alert(1)</script>".to_owned()),
            }],
        };
        variables
            .values
            .sort_by(|left, right| left.variable.cmp(&right.variable));
        let action = InteractionAction::AppendVisibleSystemEvent {
            text: SafeTemplate {
                parts: vec![TemplatePart::Variable { variable: text }],
                max_output_chars: 1_024,
            },
        };
        let engine = InteractionEngine::compile(
            &[rule_set(vec![rule(
                "plain-text",
                InteractionEvent::ConversationOpened,
                vec![action],
            )])],
            InteractionLimits::default(),
        )
        .expect("compile");
        let initial = InteractionState {
            variables,
            manually_active_knowledge: Vec::new(),
            proposals: Vec::new(),
            revision: 0,
        };
        let outcome = engine
            .handle_event(
                &initial,
                &InteractionEvent::ConversationOpened,
                &InteractionContext::default(),
            )
            .expect("engine continues");
        assert!(outcome.effects.is_empty());
        assert_eq!(outcome.trace[0].status, InteractionRuleStatus::Failed);
        assert_eq!(
            outcome.trace[0].error.as_ref().map(|error| &error.code),
            Some(&InteractionFailureCode::UnsafeText)
        );
    }

    #[test]
    fn code_network_file_and_html_actions_cannot_deserialize() {
        for json in [
            r#"{"kind":"execute_code","source":"steal()"}"#,
            r#"{"kind":"network_request","url":"https://example.invalid"}"#,
            r#"{"kind":"read_file","path":"/etc/passwd"}"#,
            r#"{"kind":"insert_html","html":"<b>x</b>"}"#,
        ] {
            assert!(serde_json::from_str::<InteractionAction>(json).is_err());
        }
    }

    #[test]
    fn condition_depth_action_count_and_dice_are_bounded() {
        let mut condition = ConditionExpr::True;
        for _ in 0..4 {
            condition = ConditionExpr::Not {
                expression: Box::new(condition),
            };
        }
        let limits = InteractionLimits {
            max_condition_depth: 3,
            max_actions_per_rule: 1,
            ..InteractionLimits::default()
        };
        let mut deep = rule("deep", InteractionEvent::ConversationOpened, Vec::new());
        deep.condition = Some(condition);
        assert_eq!(
            InteractionEngine::compile(&[rule_set(vec![deep])], limits)
                .expect_err("deep condition")
                .code,
            InteractionFailureCode::InvalidCondition
        );

        let too_many = rule(
            "actions",
            InteractionEvent::ConversationOpened,
            vec![
                InteractionAction::ActivateKnowledge {
                    entry_id: KnowledgeEntryId::from("one"),
                },
                InteractionAction::ActivateKnowledge {
                    entry_id: KnowledgeEntryId::from("two"),
                },
            ],
        );
        assert_eq!(
            InteractionEngine::compile(&[rule_set(vec![too_many])], limits)
                .expect_err("too many actions")
                .code,
            InteractionFailureCode::InvalidAction
        );

        let invalid_dice = rule(
            "dice",
            InteractionEvent::ConversationOpened,
            vec![InteractionAction::RollDice {
                expression: DiceExpression {
                    count: 1,
                    sides: 1,
                    modifier: 0,
                },
                target: None,
            }],
        );
        assert_eq!(
            InteractionEngine::compile(
                &[rule_set(vec![invalid_dice])],
                InteractionLimits::default(),
            )
            .expect_err("one-sided dice")
            .code,
            InteractionFailureCode::InvalidAction
        );
    }

    #[test]
    fn asset_ids_are_identifiers_not_paths_or_urls() {
        let action = InteractionAction::ShowAsset {
            asset_id: AssetId::from("../../secret"),
            region: UiRegion::Message,
        };
        let error = InteractionEngine::compile(
            &[rule_set(vec![rule(
                "asset",
                InteractionEvent::ConversationOpened,
                vec![action],
            )])],
            InteractionLimits::default(),
        )
        .expect_err("path-like asset id");
        assert_eq!(error.code, InteractionFailureCode::InvalidIdentifier);
    }
}
