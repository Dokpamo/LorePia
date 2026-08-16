//! Provider-neutral parameter validation and exact provider wire mapping.
//!
//! This module intentionally separates three concerns:
//!
//! 1. a type-safe parameter schema used to build native controls,
//! 2. evaluation of preset values before any network work starts, and
//! 3. exact, capability-gated conversion to provider request patches.
//!
//! An inherited or untouched value never produces a request field. Provider
//! differences that cannot be represented without loss are rejected instead of
//! being approximated.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use lorepia_domain::{
    ApiFamily, GenerationPromptCacheMode, GenerationPromptCacheSettings, GenerationPromptCacheTtl,
    GenerationReasoningEffort, GenerationReasoningMode, GenerationReasoningSettings,
    GenerationReasoningSummary, ParameterConditionOperator, ParameterDefaultMode, ParameterId,
    ParameterLiteral, ParameterSpec, ParameterValue, ParameterValueState, ProviderParameterMapping,
    ProviderParameterTarget, ToolPolicy, UiParameterLevel,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::registry::{OpenRouterReasoningEffort, OpenRouterReasoningEffortSupport};

const MAX_PARAMETER_COUNT: usize = 128;
const MAX_PARAMETER_ID_BYTES: usize = 64;
const MAX_LABEL_KEY_BYTES: usize = 128;
const MAX_FIELD_PATH_BYTES: usize = 128;
const MAX_ENUM_CHOICES: usize = 64;
const MAX_STRING_BYTES: u32 = 16 * 1024;
const MAX_LIST_ITEMS: u32 = 64;
const MAX_LIST_ITEM_BYTES: u32 = 4 * 1024;
const MAX_JSON_SCHEMA_BYTES: u32 = 64 * 1024;
const MAX_CONDITION_DEPTH: usize = 8;
const MAX_CUSTOM_CACHE_TTL_SECONDS: u32 = 30 * 24 * 60 * 60;
const MAX_EXACT_INTEGER_F64: f64 = 9_007_199_254_740_991.0;

/// A typed choice for integer parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerChoice {
    pub value: i64,
    pub label_key: String,
}

/// A typed choice for floating-point parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberChoice {
    pub value: f64,
    pub label_key: String,
}

/// A typed choice for string or enum parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringChoice {
    pub value: String,
    pub label_key: String,
}

/// A safe sum type for native parameter controls.
///
/// Unlike the legacy flat manifest representation, impossible combinations such
/// as a boolean with numeric bounds cannot be represented.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParameterSchema {
    Boolean,
    Integer {
        minimum: Option<i64>,
        maximum: Option<i64>,
        step: Option<u64>,
        choices: Vec<IntegerChoice>,
    },
    Number {
        minimum: Option<f64>,
        maximum: Option<f64>,
        step: Option<f64>,
        choices: Vec<NumberChoice>,
    },
    String {
        min_bytes: u32,
        max_bytes: u32,
        choices: Vec<StringChoice>,
    },
    Enum {
        choices: Vec<StringChoice>,
    },
    StringList {
        min_items: u32,
        max_items: u32,
        max_item_bytes: u32,
        unique_items: bool,
    },
    JsonSchema {
        max_bytes: u32,
    },
    StopSequenceList {
        max_items: u32,
        max_item_bytes: u32,
    },
    ToolPolicy {
        allowed: Vec<ToolPolicy>,
    },
}

/// A bounded boolean expression controlling whether a parameter is visible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operator", rename_all = "snake_case")]
pub enum ParameterConditionExpr {
    Equals {
        parameter_id: ParameterId,
        value: ParameterLiteral,
    },
    NotEquals {
        parameter_id: ParameterId,
        value: ParameterLiteral,
    },
    All {
        conditions: Vec<Self>,
    },
    Any {
        conditions: Vec<Self>,
    },
}

/// A cross-parameter rule evaluated before a request is constructed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParameterRule {
    MutuallyExclusive {
        parameter_id: ParameterId,
        message_key: String,
    },
    Requires {
        parameter_id: ParameterId,
        message_key: String,
    },
}

/// A compiled, type-safe parameter definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedParameterSpec {
    pub id: ParameterId,
    pub label_key: String,
    pub description_key: Option<String>,
    pub schema: ParameterSchema,
    pub default_mode: ParameterDefaultMode,
    pub visibility: Option<ParameterConditionExpr>,
    pub rules: Vec<ParameterRule>,
    pub provider_mapping: ProviderParameterMapping,
    pub level: UiParameterLevel,
}

/// Stable codes suitable for native UI handling and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterIssueCode {
    InvalidDefinition,
    DuplicateParameter,
    UnknownParameter,
    DuplicateValue,
    RequiredValueMissing,
    TypeMismatch,
    OutOfBounds,
    InvalidStep,
    InvalidChoice,
    InvalidJsonSchema,
    HiddenValue,
    MutuallyExclusive,
    MissingRequirement,
    UnsupportedMapping,
    ConflictingRequestField,
    UnsupportedReasoning,
    UnsupportedPromptCache,
    InvalidPromptCacheReference,
}

/// A renderable validation issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterIssue {
    pub code: ParameterIssueCode,
    pub parameter_id: Option<ParameterId>,
    pub related_parameter_id: Option<ParameterId>,
    pub message: String,
}

impl ParameterIssue {
    fn new(
        code: ParameterIssueCode,
        parameter_id: Option<ParameterId>,
        related_parameter_id: Option<ParameterId>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            parameter_id,
            related_parameter_id,
            message: message.into(),
        }
    }
}

/// Aggregate failure returned before provider networking is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterEngineError {
    pub issues: Vec<ParameterIssue>,
}

impl fmt::Display for ParameterEngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(issue) = self.issues.first() {
            write!(formatter, "{}", issue.message)
        } else {
            formatter.write_str("parameter validation failed")
        }
    }
}

impl std::error::Error for ParameterEngineError {}

impl ParameterEngineError {
    fn one(issue: ParameterIssue) -> Self {
        Self {
            issues: vec![issue],
        }
    }
}

/// A native-ready parameter control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterControl {
    pub id: ParameterId,
    pub label_key: String,
    pub description_key: Option<String>,
    pub schema: ParameterSchema,
    pub state: ParameterValueState,
    pub visible: bool,
    pub enabled: bool,
    pub issues: Vec<ParameterIssue>,
}

/// Controls grouped exactly as the native settings surfaces render them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterEditorModel {
    pub basic: Vec<ParameterControl>,
    pub advanced: Vec<ParameterControl>,
    pub expert: Vec<ParameterControl>,
}

/// An explicit parameter that survived schema and cross-field validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedParameter {
    pub parameter_id: ParameterId,
    pub mapping: ProviderParameterMapping,
    pub value: ParameterLiteral,
}

/// A complete evaluation useful for live validation in native UIs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterEvaluation {
    pub editor: ParameterEditorModel,
    pub applied: Vec<AppliedParameter>,
    pub omitted_provider_defaults: Vec<ParameterId>,
    pub issues: Vec<ParameterIssue>,
}

impl ParameterEvaluation {
    pub fn is_valid(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Validated request parameters. Constructed only by [`ParameterEngine`].
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedParameterSet {
    applied: Vec<AppliedParameter>,
    omitted_provider_defaults: Vec<ParameterId>,
}

impl ValidatedParameterSet {
    pub fn applied(&self) -> &[AppliedParameter] {
        &self.applied
    }

    pub fn omitted_provider_defaults(&self) -> &[ParameterId] {
        &self.omitted_provider_defaults
    }
}

/// Compiles manifest parameter specs and evaluates generation presets.
#[derive(Debug, Clone)]
pub struct ParameterEngine {
    specs: Vec<TypedParameterSpec>,
    indices: BTreeMap<String, usize>,
}

impl ParameterEngine {
    /// Compiles the existing manifest DTO into the safe schema.
    pub fn from_manifest_specs(specs: &[ParameterSpec]) -> Result<Self, ParameterEngineError> {
        let mut typed = Vec::with_capacity(specs.len());
        let mut issues = Vec::new();
        for spec in specs {
            match TypedParameterSpec::try_from(spec) {
                Ok(spec) => typed.push(spec),
                Err(mut error) => issues.append(&mut error.issues),
            }
        }
        if issues.is_empty() {
            Self::new(typed)
        } else {
            Err(ParameterEngineError { issues })
        }
    }

    /// Compiles a manifest parameter contract and verifies that every declared
    /// mapping belongs to the selected closed adapter family.
    ///
    /// This is stricter than [`Self::from_manifest_specs`]: unsupported
    /// mappings are rejected even when the corresponding preset value is
    /// untouched and would otherwise be omitted from a request.
    pub fn from_manifest_specs_for_family(
        family: ApiFamily,
        specs: &[ParameterSpec],
    ) -> Result<Self, ParameterEngineError> {
        let engine = Self::from_manifest_specs(specs)?;
        engine.validate_mappings_for_family(family)?;
        Ok(engine)
    }

    /// Validates definitions, references, mappings, and bounds.
    pub fn new(specs: Vec<TypedParameterSpec>) -> Result<Self, ParameterEngineError> {
        let mut issues = Vec::new();
        if specs.len() > MAX_PARAMETER_COUNT {
            issues.push(ParameterIssue::new(
                ParameterIssueCode::InvalidDefinition,
                None,
                None,
                format!("at most {MAX_PARAMETER_COUNT} parameters are allowed"),
            ));
        }

        let mut indices = BTreeMap::new();
        for (index, spec) in specs.iter().enumerate() {
            validate_definition_shape(spec, &mut issues);
            let normalized = normalize_id(&spec.id);
            if indices.insert(normalized, index).is_some() {
                issues.push(ParameterIssue::new(
                    ParameterIssueCode::DuplicateParameter,
                    Some(spec.id.clone()),
                    None,
                    "parameter ids must be unique ignoring ASCII case",
                ));
            }
        }

        let mut mappings = BTreeSet::new();
        for spec in &specs {
            if !mappings.insert((
                match spec.provider_mapping.target {
                    ProviderParameterTarget::RequestBody => 0_u8,
                    ProviderParameterTarget::RequestHeader => 1_u8,
                },
                spec.provider_mapping.field_name.to_ascii_lowercase(),
            )) {
                issues.push(ParameterIssue::new(
                    ParameterIssueCode::InvalidDefinition,
                    Some(spec.id.clone()),
                    None,
                    "two parameters may not map to the same request field",
                ));
            }
            validate_condition_references(
                spec.visibility.as_ref(),
                &specs,
                &indices,
                0,
                &spec.id,
                &mut issues,
            );
            for rule in &spec.rules {
                let related = match rule {
                    ParameterRule::MutuallyExclusive { parameter_id, .. }
                    | ParameterRule::Requires { parameter_id, .. } => parameter_id,
                };
                if normalize_id(related) == normalize_id(&spec.id) {
                    issues.push(ParameterIssue::new(
                        ParameterIssueCode::InvalidDefinition,
                        Some(spec.id.clone()),
                        Some(related.clone()),
                        "a parameter rule may not reference itself",
                    ));
                } else if !indices.contains_key(&normalize_id(related)) {
                    issues.push(ParameterIssue::new(
                        ParameterIssueCode::InvalidDefinition,
                        Some(spec.id.clone()),
                        Some(related.clone()),
                        "parameter rule references an unknown parameter",
                    ));
                }
            }
        }

        if issues.is_empty() {
            Ok(Self { specs, indices })
        } else {
            Err(ParameterEngineError { issues })
        }
    }

    /// Produces grouped controls and all current validation issues.
    #[allow(clippy::too_many_lines)]
    pub fn evaluate(&self, values: &[ParameterValue]) -> ParameterEvaluation {
        let (states, mut issues) = self.collect_states(values);
        let explicit_ids = states
            .iter()
            .filter_map(|(id, state)| {
                matches!(state, ParameterValueState::Explicit(_)).then_some(id.clone())
            })
            .collect::<BTreeSet<_>>();

        let mut controls = Vec::with_capacity(self.specs.len());
        let mut applied = Vec::new();
        let mut omitted_provider_defaults = Vec::new();

        for spec in &self.specs {
            let id = normalize_id(&spec.id);
            let state = states
                .get(&id)
                .cloned()
                .unwrap_or(ParameterValueState::InheritProviderDefault);
            let visible = condition_is_true(spec.visibility.as_ref(), &states);
            let mut control_issues = Vec::new();

            match &state {
                ParameterValueState::InheritProviderDefault => {
                    omitted_provider_defaults.push(spec.id.clone());
                    if visible && spec.default_mode == ParameterDefaultMode::ExplicitRequired {
                        control_issues.push(ParameterIssue::new(
                            ParameterIssueCode::RequiredValueMissing,
                            Some(spec.id.clone()),
                            None,
                            "this visible parameter requires an explicit value",
                        ));
                    }
                }
                ParameterValueState::Explicit(value) => {
                    if !visible {
                        control_issues.push(ParameterIssue::new(
                            ParameterIssueCode::HiddenValue,
                            Some(spec.id.clone()),
                            None,
                            "a hidden parameter may not retain an explicit user value",
                        ));
                    }
                    if let Err(message) = validate_literal(&spec.schema, value) {
                        let code = classify_literal_error(&message);
                        control_issues.push(ParameterIssue::new(
                            code,
                            Some(spec.id.clone()),
                            None,
                            message,
                        ));
                    } else if visible {
                        applied.push(AppliedParameter {
                            parameter_id: spec.id.clone(),
                            mapping: spec.provider_mapping.clone(),
                            value: value.clone(),
                        });
                    }
                }
            }

            if visible && explicit_ids.contains(&id) {
                for rule in &spec.rules {
                    match rule {
                        ParameterRule::MutuallyExclusive {
                            parameter_id,
                            message_key,
                        } if explicit_ids.contains(&normalize_id(parameter_id)) => {
                            control_issues.push(ParameterIssue::new(
                                ParameterIssueCode::MutuallyExclusive,
                                Some(spec.id.clone()),
                                Some(parameter_id.clone()),
                                message_key.clone(),
                            ));
                        }
                        ParameterRule::Requires {
                            parameter_id,
                            message_key,
                        } if !explicit_ids.contains(&normalize_id(parameter_id)) => {
                            control_issues.push(ParameterIssue::new(
                                ParameterIssueCode::MissingRequirement,
                                Some(spec.id.clone()),
                                Some(parameter_id.clone()),
                                message_key.clone(),
                            ));
                        }
                        ParameterRule::MutuallyExclusive { .. }
                        | ParameterRule::Requires { .. } => {}
                    }
                }
            }

            let disabled_by_conflict = !explicit_ids.contains(&id)
                && spec.rules.iter().any(|rule| match rule {
                    ParameterRule::MutuallyExclusive { parameter_id, .. } => {
                        explicit_ids.contains(&normalize_id(parameter_id))
                    }
                    ParameterRule::Requires { .. } => false,
                });

            issues.extend(control_issues.iter().cloned());
            controls.push((
                spec.level,
                ParameterControl {
                    id: spec.id.clone(),
                    label_key: spec.label_key.clone(),
                    description_key: spec.description_key.clone(),
                    schema: spec.schema.clone(),
                    state,
                    visible,
                    enabled: visible && !disabled_by_conflict,
                    issues: control_issues,
                },
            ));
        }

        let mut editor = ParameterEditorModel::default();
        for (level, control) in controls {
            if !control.visible {
                continue;
            }
            match level {
                UiParameterLevel::Basic => editor.basic.push(control),
                UiParameterLevel::Advanced => editor.advanced.push(control),
                UiParameterLevel::Expert => editor.expert.push(control),
                UiParameterLevel::HiddenInternal => {}
            }
        }

        ParameterEvaluation {
            editor,
            applied,
            omitted_provider_defaults,
            issues,
        }
    }

    /// Rejects any invalid or unsupported preset before an adapter can perform
    /// networking.
    pub fn validate_for_request(
        &self,
        values: &[ParameterValue],
    ) -> Result<ValidatedParameterSet, ParameterEngineError> {
        let evaluation = self.evaluate(values);
        if evaluation.issues.is_empty() {
            Ok(ValidatedParameterSet {
                applied: evaluation.applied,
                omitted_provider_defaults: evaluation.omitted_provider_defaults,
            })
        } else {
            Err(ParameterEngineError {
                issues: evaluation.issues,
            })
        }
    }

    /// Verifies the complete parameter contract against one closed adapter
    /// family, including parameters currently inheriting provider defaults.
    pub fn validate_mappings_for_family(
        &self,
        family: ApiFamily,
    ) -> Result<(), ParameterEngineError> {
        let issues = self
            .specs
            .iter()
            .filter_map(|spec| validate_spec_mapping_for_family(family, spec).err())
            .flat_map(|error| error.issues)
            .collect::<Vec<_>>();
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ParameterEngineError { issues })
        }
    }

    fn collect_states(
        &self,
        values: &[ParameterValue],
    ) -> (BTreeMap<String, ParameterValueState>, Vec<ParameterIssue>) {
        let mut states = BTreeMap::new();
        let mut issues = Vec::new();
        for value in values {
            let id = normalize_id(&value.parameter_id);
            if !self.indices.contains_key(&id) {
                issues.push(ParameterIssue::new(
                    ParameterIssueCode::UnknownParameter,
                    Some(value.parameter_id.clone()),
                    None,
                    "preset contains an unknown parameter",
                ));
                continue;
            }
            if states.insert(id, value.state.clone()).is_some() {
                issues.push(ParameterIssue::new(
                    ParameterIssueCode::DuplicateValue,
                    Some(value.parameter_id.clone()),
                    None,
                    "preset contains more than one value for a parameter",
                ));
            }
        }
        (states, issues)
    }
}

impl TryFrom<&ParameterSpec> for TypedParameterSpec {
    type Error = ParameterEngineError;

    #[allow(clippy::too_many_lines)]
    fn try_from(spec: &ParameterSpec) -> Result<Self, Self::Error> {
        use lorepia_domain::ParameterType;

        let schema = match spec.value_type {
            ParameterType::Boolean => {
                require_no_bounds(spec)?;
                require_no_choices(spec)?;
                ParameterSchema::Boolean
            }
            ParameterType::Integer => ParameterSchema::Integer {
                minimum: optional_integral_bound(spec.minimum, &spec.id, "minimum")?,
                maximum: optional_integral_bound(spec.maximum, &spec.id, "maximum")?,
                step: optional_integral_step(spec.step, &spec.id)?,
                choices: integer_choices(spec)?,
            },
            ParameterType::Number => ParameterSchema::Number {
                minimum: finite_optional(spec.minimum, &spec.id, "minimum")?,
                maximum: finite_optional(spec.maximum, &spec.id, "maximum")?,
                step: finite_optional(spec.step, &spec.id, "step")?,
                choices: number_choices(spec)?,
            },
            ParameterType::String => {
                require_no_bounds(spec)?;
                ParameterSchema::String {
                    min_bytes: 0,
                    max_bytes: MAX_STRING_BYTES,
                    choices: string_choices(spec, false)?,
                }
            }
            ParameterType::Enum => {
                require_no_bounds(spec)?;
                ParameterSchema::Enum {
                    choices: string_choices(spec, true)?,
                }
            }
            ParameterType::StringList => {
                require_no_bounds(spec)?;
                require_no_choices(spec)?;
                ParameterSchema::StringList {
                    min_items: 0,
                    max_items: MAX_LIST_ITEMS,
                    max_item_bytes: MAX_LIST_ITEM_BYTES,
                    unique_items: false,
                }
            }
            ParameterType::JsonSchema => {
                require_no_bounds(spec)?;
                require_no_choices(spec)?;
                ParameterSchema::JsonSchema {
                    max_bytes: MAX_JSON_SCHEMA_BYTES,
                }
            }
            ParameterType::StopSequenceList => {
                require_no_bounds(spec)?;
                require_no_choices(spec)?;
                ParameterSchema::StopSequenceList {
                    max_items: MAX_LIST_ITEMS,
                    max_item_bytes: MAX_LIST_ITEM_BYTES,
                }
            }
            ParameterType::ToolPolicy => {
                require_no_bounds(spec)?;
                let allowed = if spec.allowed_values.is_empty() {
                    vec![ToolPolicy::None, ToolPolicy::Auto, ToolPolicy::Required]
                } else {
                    spec.allowed_values
                        .iter()
                        .map(|choice| match choice.value {
                            ParameterLiteral::ToolPolicy(value) => Ok(value),
                            _ => Err(definition_error(
                                &spec.id,
                                "tool policy choices must contain tool policy literals",
                            )),
                        })
                        .collect::<Result<Vec<_>, _>>()?
                };
                ParameterSchema::ToolPolicy { allowed }
            }
        };

        let visibility = spec.visibility.as_ref().map(|condition| {
            let fields = (condition.parameter_id.clone(), condition.value.clone());
            match condition.operator {
                ParameterConditionOperator::Equals => ParameterConditionExpr::Equals {
                    parameter_id: fields.0,
                    value: fields.1,
                },
                ParameterConditionOperator::NotEquals => ParameterConditionExpr::NotEquals {
                    parameter_id: fields.0,
                    value: fields.1,
                },
            }
        });
        let rules = spec
            .conflicts
            .iter()
            .map(|conflict| match conflict.kind {
                lorepia_domain::ParameterConflictKind::MutuallyExclusive => {
                    ParameterRule::MutuallyExclusive {
                        parameter_id: conflict.parameter_id.clone(),
                        message_key: conflict.message_key.clone(),
                    }
                }
                lorepia_domain::ParameterConflictKind::Requires => ParameterRule::Requires {
                    parameter_id: conflict.parameter_id.clone(),
                    message_key: conflict.message_key.clone(),
                },
            })
            .collect();

        Ok(Self::new_unchecked(spec, schema, visibility, rules))
    }
}

impl TypedParameterSpec {
    fn new_unchecked(
        spec: &ParameterSpec,
        schema: ParameterSchema,
        visibility: Option<ParameterConditionExpr>,
        rules: Vec<ParameterRule>,
    ) -> Self {
        Self {
            id: spec.id.clone(),
            label_key: spec.label_key.clone(),
            description_key: spec.description_key.clone(),
            schema,
            default_mode: spec.default_mode,
            visibility,
            rules,
            provider_mapping: spec.provider_mapping.clone(),
            level: spec.level,
        }
    }
}

/// Explicit values used only when converting a legacy profile into a preset.
///
/// New presets should start with no values, which preserves provider defaults.
pub fn legacy_generation_preset_values() -> Vec<ParameterValue> {
    vec![
        ParameterValue {
            parameter_id: ParameterId::from("temperature"),
            state: ParameterValueState::Explicit(ParameterLiteral::Number(1.0)),
        },
        ParameterValue {
            parameter_id: ParameterId::from("max_output_tokens"),
            state: ParameterValueState::Explicit(ParameterLiteral::Integer(4096)),
        },
    ]
}

/// A new untouched preset has no explicit values and therefore emits nothing.
pub fn new_generation_preset_values() -> Vec<ParameterValue> {
    Vec::new()
}

fn normalize_id(id: &ParameterId) -> String {
    id.as_str().to_ascii_lowercase()
}

fn definition_error(id: &ParameterId, message: impl Into<String>) -> ParameterEngineError {
    ParameterEngineError::one(ParameterIssue::new(
        ParameterIssueCode::InvalidDefinition,
        Some(id.clone()),
        None,
        message,
    ))
}

fn require_no_bounds(spec: &ParameterSpec) -> Result<(), ParameterEngineError> {
    if spec.minimum.is_some() || spec.maximum.is_some() || spec.step.is_some() {
        Err(definition_error(
            &spec.id,
            "this parameter type may not declare numeric bounds",
        ))
    } else {
        Ok(())
    }
}

fn require_no_choices(spec: &ParameterSpec) -> Result<(), ParameterEngineError> {
    if spec.allowed_values.is_empty() {
        Ok(())
    } else {
        Err(definition_error(
            &spec.id,
            "this parameter type may not declare choices",
        ))
    }
}

fn finite_optional(
    value: Option<f64>,
    id: &ParameterId,
    name: &str,
) -> Result<Option<f64>, ParameterEngineError> {
    match value {
        Some(value) if !value.is_finite() => Err(definition_error(
            id,
            format!("{name} must be a finite number"),
        )),
        value => Ok(value),
    }
}

fn optional_integral_bound(
    value: Option<f64>,
    id: &ParameterId,
    name: &str,
) -> Result<Option<i64>, ParameterEngineError> {
    let Some(value) = finite_optional(value, id, name)? else {
        return Ok(None);
    };
    if value.fract() != 0.0 || !(-MAX_EXACT_INTEGER_F64..=MAX_EXACT_INTEGER_F64).contains(&value) {
        return Err(definition_error(
            id,
            format!("{name} must be an exactly representable integer"),
        ));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(Some(value as i64))
}

fn optional_integral_step(
    value: Option<f64>,
    id: &ParameterId,
) -> Result<Option<u64>, ParameterEngineError> {
    let Some(value) = finite_optional(value, id, "step")? else {
        return Ok(None);
    };
    if value <= 0.0 || value.fract() != 0.0 || value > MAX_EXACT_INTEGER_F64 {
        return Err(definition_error(
            id,
            "integer step must be a positive integer",
        ));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(Some(value as u64))
}

fn integer_choices(spec: &ParameterSpec) -> Result<Vec<IntegerChoice>, ParameterEngineError> {
    spec.allowed_values
        .iter()
        .map(|choice| match choice.value {
            ParameterLiteral::Integer(value) => Ok(IntegerChoice {
                value,
                label_key: choice.label_key.clone(),
            }),
            _ => Err(definition_error(
                &spec.id,
                "integer choices must contain integer literals",
            )),
        })
        .collect()
}

fn number_choices(spec: &ParameterSpec) -> Result<Vec<NumberChoice>, ParameterEngineError> {
    spec.allowed_values
        .iter()
        .map(|choice| match choice.value {
            ParameterLiteral::Number(value) if value.is_finite() => Ok(NumberChoice {
                value,
                label_key: choice.label_key.clone(),
            }),
            ParameterLiteral::Number(_) => {
                Err(definition_error(&spec.id, "number choices must be finite"))
            }
            _ => Err(definition_error(
                &spec.id,
                "number choices must contain number literals",
            )),
        })
        .collect()
}

fn string_choices(
    spec: &ParameterSpec,
    enum_values: bool,
) -> Result<Vec<StringChoice>, ParameterEngineError> {
    spec.allowed_values
        .iter()
        .map(|choice| {
            let value = match &choice.value {
                ParameterLiteral::Enum(value) if enum_values => value,
                ParameterLiteral::String(value) if !enum_values => value,
                _ => {
                    return Err(definition_error(
                        &spec.id,
                        if enum_values {
                            "enum choices must contain enum literals"
                        } else {
                            "string choices must contain string literals"
                        },
                    ));
                }
            };
            Ok(StringChoice {
                value: value.clone(),
                label_key: choice.label_key.clone(),
            })
        })
        .collect()
}

fn validate_definition_shape(spec: &TypedParameterSpec, issues: &mut Vec<ParameterIssue>) {
    let id = spec.id.as_str();
    if id.is_empty()
        || id.len() > MAX_PARAMETER_ID_BYTES
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        issues.push(ParameterIssue::new(
            ParameterIssueCode::InvalidDefinition,
            Some(spec.id.clone()),
            None,
            "parameter id must be a bounded ASCII identifier",
        ));
    }
    if spec.label_key.is_empty() || spec.label_key.len() > MAX_LABEL_KEY_BYTES {
        issues.push(ParameterIssue::new(
            ParameterIssueCode::InvalidDefinition,
            Some(spec.id.clone()),
            None,
            "parameter label key is empty or too long",
        ));
    }
    if !valid_field_name(
        spec.provider_mapping.target,
        &spec.provider_mapping.field_name,
    ) {
        issues.push(ParameterIssue::new(
            ParameterIssueCode::InvalidDefinition,
            Some(spec.id.clone()),
            None,
            "provider mapping field is not a bounded header name or JSON field path",
        ));
    }
    validate_schema_shape(&spec.id, &spec.schema, issues);
}

fn valid_field_name(target: ProviderParameterTarget, field_name: &str) -> bool {
    if field_name.is_empty() || field_name.len() > MAX_FIELD_PATH_BYTES {
        return false;
    }
    match target {
        ProviderParameterTarget::RequestHeader => field_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)),
        ProviderParameterTarget::RequestBody => field_name.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        }),
    }
}

#[allow(clippy::too_many_lines)]
fn validate_schema_shape(
    id: &ParameterId,
    schema: &ParameterSchema,
    issues: &mut Vec<ParameterIssue>,
) {
    let invalid = |message: &str, issues: &mut Vec<ParameterIssue>| {
        issues.push(ParameterIssue::new(
            ParameterIssueCode::InvalidDefinition,
            Some(id.clone()),
            None,
            message,
        ));
    };
    match schema {
        ParameterSchema::Boolean => {}
        ParameterSchema::Integer {
            minimum,
            maximum,
            step,
            choices,
        } => {
            if minimum.zip(*maximum).is_some_and(|(min, max)| min > max) {
                invalid("integer minimum may not exceed maximum", issues);
            }
            if step == &Some(0) {
                invalid("integer step must be positive", issues);
            }
            if choices.len() > MAX_ENUM_CHOICES {
                invalid("too many integer choices", issues);
            }
            let mut seen = BTreeSet::new();
            for choice in choices {
                if !valid_choice_label(&choice.label_key) {
                    invalid("integer choice label key is empty or too long", issues);
                }
                if !seen.insert(choice.value) {
                    invalid("integer choices must be unique", issues);
                } else if validate_literal(schema, &ParameterLiteral::Integer(choice.value))
                    .is_err()
                {
                    invalid("integer choice violates its own schema", issues);
                }
            }
        }
        ParameterSchema::Number {
            minimum,
            maximum,
            step,
            choices,
        } => {
            if minimum.is_some_and(|value| !value.is_finite())
                || maximum.is_some_and(|value| !value.is_finite())
                || step.is_some_and(|value| !value.is_finite() || value <= 0.0)
            {
                invalid(
                    "number bounds and step must be finite and step positive",
                    issues,
                );
            }
            if minimum.zip(*maximum).is_some_and(|(min, max)| min > max) {
                invalid("number minimum may not exceed maximum", issues);
            }
            if choices.len() > MAX_ENUM_CHOICES {
                invalid("too many number choices", issues);
            }
            let mut seen = BTreeSet::new();
            for choice in choices {
                if !valid_choice_label(&choice.label_key) {
                    invalid("number choice label key is empty or too long", issues);
                }
                let normalized_bits = if choice.value == 0.0 {
                    0
                } else {
                    choice.value.to_bits()
                };
                if !seen.insert(normalized_bits) {
                    invalid("number choices must be unique", issues);
                } else if validate_literal(schema, &ParameterLiteral::Number(choice.value)).is_err()
                {
                    invalid("number choice violates its own schema", issues);
                }
            }
        }
        ParameterSchema::String {
            min_bytes,
            max_bytes,
            choices,
        } => {
            if min_bytes > max_bytes || *max_bytes > MAX_STRING_BYTES {
                invalid("string byte bounds are invalid", issues);
            }
            if choices.len() > MAX_ENUM_CHOICES {
                invalid("too many string choices", issues);
            }
            let mut seen = BTreeSet::new();
            for choice in choices {
                if !valid_choice_label(&choice.label_key) {
                    invalid("string choice label key is empty or too long", issues);
                }
                if !seen.insert(choice.value.as_str()) {
                    invalid("string choices must be unique", issues);
                } else if validate_literal(schema, &ParameterLiteral::String(choice.value.clone()))
                    .is_err()
                {
                    invalid("string choice violates its own schema", issues);
                }
            }
        }
        ParameterSchema::Enum { choices } => {
            if choices.is_empty() || choices.len() > MAX_ENUM_CHOICES {
                invalid("enum must declare a bounded non-empty choice list", issues);
            }
            let mut seen = BTreeSet::new();
            for choice in choices {
                if !valid_choice_label(&choice.label_key)
                    || choice.value.is_empty()
                    || choice.value.len() > MAX_STRING_BYTES as usize
                    || !seen.insert(choice.value.clone())
                {
                    invalid(
                        "enum choices and labels must be non-empty, bounded, and unique",
                        issues,
                    );
                }
            }
        }
        ParameterSchema::StringList {
            min_items,
            max_items,
            max_item_bytes,
            ..
        } => {
            if min_items > max_items
                || *max_items > MAX_LIST_ITEMS
                || *max_item_bytes == 0
                || *max_item_bytes > MAX_LIST_ITEM_BYTES
            {
                invalid("string list bounds are invalid", issues);
            }
        }
        ParameterSchema::JsonSchema { max_bytes } => {
            if *max_bytes == 0 || *max_bytes > MAX_JSON_SCHEMA_BYTES {
                invalid("JSON schema byte bound is invalid", issues);
            }
        }
        ParameterSchema::StopSequenceList {
            max_items,
            max_item_bytes,
        } => {
            if *max_items == 0
                || *max_items > MAX_LIST_ITEMS
                || *max_item_bytes == 0
                || *max_item_bytes > MAX_LIST_ITEM_BYTES
            {
                invalid("stop-sequence bounds are invalid", issues);
            }
        }
        ParameterSchema::ToolPolicy { allowed } => {
            if allowed.is_empty()
                || allowed.len() > 3
                || allowed
                    .iter()
                    .enumerate()
                    .any(|(index, value)| allowed[..index].contains(value))
            {
                invalid("tool policy choices must be non-empty and unique", issues);
            }
        }
    }
}

fn valid_choice_label(label_key: &str) -> bool {
    !label_key.is_empty() && label_key.len() <= MAX_LABEL_KEY_BYTES
}

fn validate_condition_references(
    condition: Option<&ParameterConditionExpr>,
    specs: &[TypedParameterSpec],
    indices: &BTreeMap<String, usize>,
    depth: usize,
    owner: &ParameterId,
    issues: &mut Vec<ParameterIssue>,
) {
    let Some(condition) = condition else {
        return;
    };
    if depth >= MAX_CONDITION_DEPTH {
        issues.push(ParameterIssue::new(
            ParameterIssueCode::InvalidDefinition,
            Some(owner.clone()),
            None,
            "parameter visibility expression is too deeply nested",
        ));
        return;
    }
    match condition {
        ParameterConditionExpr::Equals {
            parameter_id,
            value,
        }
        | ParameterConditionExpr::NotEquals {
            parameter_id,
            value,
        } => {
            let Some(index) = indices.get(&normalize_id(parameter_id)).copied() else {
                issues.push(ParameterIssue::new(
                    ParameterIssueCode::InvalidDefinition,
                    Some(owner.clone()),
                    Some(parameter_id.clone()),
                    "parameter visibility references an unknown parameter",
                ));
                return;
            };
            if validate_literal(&specs[index].schema, value).is_err() {
                issues.push(ParameterIssue::new(
                    ParameterIssueCode::InvalidDefinition,
                    Some(owner.clone()),
                    Some(parameter_id.clone()),
                    "parameter visibility literal does not match the referenced schema",
                ));
            }
        }
        ParameterConditionExpr::All { conditions } | ParameterConditionExpr::Any { conditions } => {
            if conditions.is_empty() {
                issues.push(ParameterIssue::new(
                    ParameterIssueCode::InvalidDefinition,
                    Some(owner.clone()),
                    None,
                    "parameter visibility groups may not be empty",
                ));
            }
            for child in conditions {
                validate_condition_references(
                    Some(child),
                    specs,
                    indices,
                    depth + 1,
                    owner,
                    issues,
                );
            }
        }
    }
}

fn condition_is_true(
    condition: Option<&ParameterConditionExpr>,
    states: &BTreeMap<String, ParameterValueState>,
) -> bool {
    let Some(condition) = condition else {
        return true;
    };
    match condition {
        ParameterConditionExpr::Equals {
            parameter_id,
            value,
        } => states
            .get(&normalize_id(parameter_id))
            .and_then(explicit_literal)
            .is_some_and(|actual| actual == value),
        ParameterConditionExpr::NotEquals {
            parameter_id,
            value,
        } => states
            .get(&normalize_id(parameter_id))
            .and_then(explicit_literal)
            .is_some_and(|actual| actual != value),
        ParameterConditionExpr::All { conditions } => conditions
            .iter()
            .all(|condition| condition_is_true(Some(condition), states)),
        ParameterConditionExpr::Any { conditions } => conditions
            .iter()
            .any(|condition| condition_is_true(Some(condition), states)),
    }
}

fn explicit_literal(state: &ParameterValueState) -> Option<&ParameterLiteral> {
    match state {
        ParameterValueState::Explicit(value) => Some(value),
        ParameterValueState::InheritProviderDefault => None,
    }
}

#[allow(clippy::too_many_lines)]
fn validate_literal(schema: &ParameterSchema, value: &ParameterLiteral) -> Result<(), String> {
    match (schema, value) {
        (ParameterSchema::Boolean, ParameterLiteral::Boolean(_)) => Ok(()),
        (
            ParameterSchema::Integer {
                minimum,
                maximum,
                step,
                choices,
            },
            ParameterLiteral::Integer(value),
        ) => {
            if minimum.is_some_and(|minimum| *value < minimum)
                || maximum.is_some_and(|maximum| *value > maximum)
            {
                return Err("integer value is outside the allowed bounds".to_owned());
            }
            if let Some(step) = step {
                let origin = minimum.unwrap_or(0);
                if (i128::from(*value) - i128::from(origin)).unsigned_abs() % u128::from(*step) != 0
                {
                    return Err("integer value does not align to the declared step".to_owned());
                }
            }
            if !choices.is_empty() && !choices.iter().any(|choice| choice.value == *value) {
                return Err("integer value is not an allowed choice".to_owned());
            }
            Ok(())
        }
        (
            ParameterSchema::Number {
                minimum,
                maximum,
                step,
                choices,
            },
            ParameterLiteral::Number(value),
        ) => {
            if !value.is_finite() {
                return Err("number value must be finite".to_owned());
            }
            if minimum.is_some_and(|minimum| *value < minimum)
                || maximum.is_some_and(|maximum| *value > maximum)
            {
                return Err("number value is outside the allowed bounds".to_owned());
            }
            if let Some(step) = step {
                let origin = minimum.unwrap_or(0.0);
                let units = (*value - origin) / step;
                if !units.is_finite() {
                    return Err(
                        "number value cannot be represented on the declared step grid".to_owned(),
                    );
                }
                // Scale by machine precision, not a fixed relative epsilon.
                // A `1e-9 * units` tolerance eventually becomes wider than an
                // entire step and accepts arbitrary off-grid values.
                let tolerance = 16.0 * f64::EPSILON * units.abs().max(1.0);
                if (units - units.round()).abs() > tolerance {
                    return Err("number value does not align to the declared step".to_owned());
                }
            }
            if !choices.is_empty()
                && !choices
                    .iter()
                    .any(|choice| (choice.value - *value).abs() <= f64::EPSILON)
            {
                return Err("number value is not an allowed choice".to_owned());
            }
            Ok(())
        }
        (
            ParameterSchema::String {
                min_bytes,
                max_bytes,
                choices,
            },
            ParameterLiteral::String(value),
        ) => {
            if value.len() < *min_bytes as usize || value.len() > *max_bytes as usize {
                return Err("string value is outside the allowed byte bounds".to_owned());
            }
            if !choices.is_empty() && !choices.iter().any(|choice| choice.value == *value) {
                return Err("string value is not an allowed choice".to_owned());
            }
            Ok(())
        }
        (ParameterSchema::Enum { choices }, ParameterLiteral::Enum(value)) => {
            if choices.iter().any(|choice| choice.value == *value) {
                Ok(())
            } else {
                Err("enum value is not an allowed choice".to_owned())
            }
        }
        (
            ParameterSchema::StringList {
                min_items,
                max_items,
                max_item_bytes,
                unique_items,
            },
            ParameterLiteral::StringList(values),
        ) => validate_string_list(
            values,
            *min_items,
            *max_items,
            *max_item_bytes,
            *unique_items,
            false,
        ),
        (ParameterSchema::JsonSchema { max_bytes }, ParameterLiteral::JsonSchema(value)) => {
            if value.len() > *max_bytes as usize {
                return Err("JSON schema exceeds its byte limit".to_owned());
            }
            let parsed: Value =
                serde_json::from_str(value).map_err(|_| "JSON schema is not valid JSON")?;
            if parsed.as_object().is_none() {
                return Err("JSON schema must be a JSON object".to_owned());
            }
            Ok(())
        }
        (
            ParameterSchema::StopSequenceList {
                max_items,
                max_item_bytes,
            },
            ParameterLiteral::StopSequenceList(values),
        ) => validate_string_list(values, 0, *max_items, *max_item_bytes, true, true),
        (ParameterSchema::ToolPolicy { allowed }, ParameterLiteral::ToolPolicy(value)) => {
            if allowed.contains(value) {
                Ok(())
            } else {
                Err("tool policy is not allowed by this model route".to_owned())
            }
        }
        _ => Err("parameter value type does not match its schema".to_owned()),
    }
}

fn validate_string_list(
    values: &[String],
    min_items: u32,
    max_items: u32,
    max_item_bytes: u32,
    unique_items: bool,
    require_non_empty: bool,
) -> Result<(), String> {
    if values.len() < min_items as usize || values.len() > max_items as usize {
        return Err("list item count is outside the allowed bounds".to_owned());
    }
    if values.iter().any(|value| {
        value.len() > max_item_bytes as usize || (require_non_empty && value.is_empty())
    }) {
        return Err("list item is empty or exceeds its byte limit".to_owned());
    }
    if unique_items && values.iter().collect::<BTreeSet<_>>().len() != values.len() {
        return Err("list items must be unique".to_owned());
    }
    Ok(())
}

fn classify_literal_error(message: &str) -> ParameterIssueCode {
    if message.contains("type does not match") {
        ParameterIssueCode::TypeMismatch
    } else if message.contains("step") {
        ParameterIssueCode::InvalidStep
    } else if message.contains("choice") || message.contains("policy") {
        ParameterIssueCode::InvalidChoice
    } else if message.contains("JSON schema") {
        ParameterIssueCode::InvalidJsonSchema
    } else {
        ParameterIssueCode::OutOfBounds
    }
}

/// Provider-neutral reasoning mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMode {
    ProviderDefault,
    Disabled,
    Automatic,
    Enabled,
}

/// Provider-neutral reasoning effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    ExtraHigh,
    Maximum,
}

/// How much user-visible reasoning summary should be requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningSummaryMode {
    ProviderDefault,
    Disabled,
    Automatic,
    Concise,
    Detailed,
}

/// Common reasoning settings stored in a generation preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningSettings {
    pub mode: ReasoningMode,
    pub effort: Option<ReasoningEffort>,
    pub budget_tokens: Option<u32>,
    pub summary: ReasoningSummaryMode,
    pub preserve_opaque_state: bool,
}

/// Stable pre-network failure used when Gemini opaque reasoning continuity is
/// requested without durable storage for the exact original Content/Part
/// topology.
pub const GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR: &str =
    "Gemini opaque reasoning state requires exact prior content topology";

impl Default for ReasoningSettings {
    fn default() -> Self {
        Self {
            mode: ReasoningMode::ProviderDefault,
            effort: None,
            budget_tokens: None,
            summary: ReasoningSummaryMode::ProviderDefault,
            preserve_opaque_state: false,
        }
    }
}

impl From<GenerationReasoningMode> for ReasoningMode {
    fn from(value: GenerationReasoningMode) -> Self {
        match value {
            GenerationReasoningMode::ProviderDefault => Self::ProviderDefault,
            GenerationReasoningMode::Disabled => Self::Disabled,
            GenerationReasoningMode::Automatic => Self::Automatic,
            GenerationReasoningMode::Enabled => Self::Enabled,
        }
    }
}

impl From<GenerationReasoningEffort> for ReasoningEffort {
    fn from(value: GenerationReasoningEffort) -> Self {
        match value {
            GenerationReasoningEffort::Minimal => Self::Minimal,
            GenerationReasoningEffort::Low => Self::Low,
            GenerationReasoningEffort::Medium => Self::Medium,
            GenerationReasoningEffort::High => Self::High,
            GenerationReasoningEffort::ExtraHigh => Self::ExtraHigh,
            GenerationReasoningEffort::Maximum => Self::Maximum,
        }
    }
}

impl From<GenerationReasoningSummary> for ReasoningSummaryMode {
    fn from(value: GenerationReasoningSummary) -> Self {
        match value {
            GenerationReasoningSummary::ProviderDefault => Self::ProviderDefault,
            GenerationReasoningSummary::Disabled => Self::Disabled,
            GenerationReasoningSummary::Automatic => Self::Automatic,
            GenerationReasoningSummary::Concise => Self::Concise,
            GenerationReasoningSummary::Detailed => Self::Detailed,
        }
    }
}

impl From<&GenerationReasoningSettings> for ReasoningSettings {
    fn from(value: &GenerationReasoningSettings) -> Self {
        Self {
            mode: value.mode.into(),
            effort: value.effort.map(Into::into),
            budget_tokens: value.budget_tokens,
            summary: value.summary.into(),
            preserve_opaque_state: value.preserve_opaque_state,
        }
    }
}

/// An exact wire dialect observed for the selected model route.
///
/// The API family alone is insufficient because model generations within one
/// provider can support different reasoning controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "dialect", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReasoningWireDialect {
    Unsupported,
    OpenAiResponses {
        efforts: Vec<ReasoningEffort>,
        summaries: Vec<ReasoningSummaryMode>,
        supports_disabled: bool,
    },
    OpenAiChatCompletions {
        efforts: Vec<ReasoningEffort>,
        supports_disabled: bool,
    },
    OpenRouter {
        #[serde(default)]
        style: OpenRouterReasoningWireStyle,
        supported_efforts: OpenRouterReasoningEffortSupport,
        default_effort: Option<OpenRouterReasoningEffort>,
        default_enabled: Option<bool>,
        supports_max_tokens: Option<bool>,
        mandatory: Option<bool>,
    },
    AnthropicAdaptive {
        efforts: Vec<ReasoningEffort>,
        summaries: Vec<ReasoningSummaryMode>,
        supports_disabled: bool,
    },
    AnthropicExtended {
        minimum_budget_tokens: u32,
        maximum_budget_tokens: u32,
        efforts: Vec<ReasoningEffort>,
        summaries: Vec<ReasoningSummaryMode>,
        supports_disabled: bool,
    },
    GeminiThinkingLevel {
        efforts: Vec<ReasoningEffort>,
        supports_automatic: bool,
        summaries: Vec<ReasoningSummaryMode>,
    },
    GeminiThinkingBudget {
        minimum_budget_tokens: u32,
        maximum_budget_tokens: u32,
        supports_zero_to_disable: bool,
        supports_automatic: bool,
        summaries: Vec<ReasoningSummaryMode>,
    },
    OllamaBoolean,
    OllamaLevel {
        efforts: Vec<ReasoningEffort>,
        supports_disabled: bool,
    },
}

/// Exact request-body shape used by an `OpenRouter` reasoning control.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenRouterReasoningWireStyle {
    #[default]
    Unified,
    LegacyReasoningEffort,
}

/// Inclusive token bounds for a render-ready reasoning budget control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenBudgetBounds {
    pub minimum: u32,
    pub maximum: u32,
}

/// Overall state of a render-ready control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiControlState {
    Hidden,
    Ready,
    Invalid,
}

/// State of a dependent field inside a render-ready control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiFieldState {
    Hidden,
    Enabled,
    Required,
}

/// Render-ready reasoning control derived entirely in Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningControlModel {
    pub state: UiControlState,
    pub settings: ReasoningSettings,
    pub allowed_modes: Vec<ReasoningMode>,
    pub allowed_efforts: Vec<ReasoningEffort>,
    pub allowed_summaries: Vec<ReasoningSummaryMode>,
    pub budget_bounds: Option<TokenBudgetBounds>,
    pub effort_field: UiFieldState,
    pub budget_field: UiFieldState,
    pub summary_field: UiFieldState,
    pub issues: Vec<ParameterIssue>,
}

/// Provider prompt-cache mode. Local response, embedding, and Ollama model
/// residency caches are deliberately not represented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheMode {
    ProviderDefault,
    Automatic,
    ExplicitBreakpoints,
    ExplicitContext,
    DisabledIfSupported,
}

/// Requested provider prompt-cache lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "seconds", rename_all = "snake_case")]
pub enum PromptCacheTtl {
    ProviderDefault,
    Short,
    Long,
    CustomSeconds(u32),
}

/// Common cache settings stored in a generation preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptCacheSettings {
    pub mode: PromptCacheMode,
    pub ttl: PromptCacheTtl,
    /// Opaque provider cache resource name for `ExplicitContext`.
    ///
    /// This is not a credential. Adapters must still validate it using the
    /// provider-specific grammar below.
    pub context_reference: Option<String>,
}

impl Default for PromptCacheSettings {
    fn default() -> Self {
        Self {
            mode: PromptCacheMode::ProviderDefault,
            ttl: PromptCacheTtl::ProviderDefault,
            context_reference: None,
        }
    }
}

impl From<GenerationPromptCacheMode> for PromptCacheMode {
    fn from(value: GenerationPromptCacheMode) -> Self {
        match value {
            GenerationPromptCacheMode::ProviderDefault => Self::ProviderDefault,
            GenerationPromptCacheMode::Automatic => Self::Automatic,
            GenerationPromptCacheMode::ExplicitBreakpoints => Self::ExplicitBreakpoints,
            GenerationPromptCacheMode::ExplicitContext => Self::ExplicitContext,
            GenerationPromptCacheMode::DisabledIfSupported => Self::DisabledIfSupported,
        }
    }
}

impl From<GenerationPromptCacheTtl> for PromptCacheTtl {
    fn from(value: GenerationPromptCacheTtl) -> Self {
        match value {
            GenerationPromptCacheTtl::ProviderDefault => Self::ProviderDefault,
            GenerationPromptCacheTtl::Short => Self::Short,
            GenerationPromptCacheTtl::Long => Self::Long,
            GenerationPromptCacheTtl::CustomSeconds(seconds) => Self::CustomSeconds(seconds),
        }
    }
}

impl From<&GenerationPromptCacheSettings> for PromptCacheSettings {
    fn from(value: &GenerationPromptCacheSettings) -> Self {
        Self {
            mode: value.mode.into(),
            ttl: value.ttl.into(),
            context_reference: value.context_reference.clone(),
        }
    }
}

/// Exact provider prompt-cache behavior for the selected route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "dialect", rename_all = "snake_case", deny_unknown_fields)]
pub enum PromptCacheWireDialect {
    Unsupported,
    OpenAiAutomatic {
        supports_24_hour_retention: bool,
    },
    Anthropic {
        supports_automatic: bool,
        supports_explicit_breakpoints: bool,
        supports_one_hour_ttl: bool,
    },
    Gemini {
        supports_implicit: bool,
        supports_explicit_context: bool,
    },
}

/// Decodes a source-attributed structured capability value into an exact
/// reasoning wire contract. Malformed, lossy, or family-mismatched metadata is
/// rejected before request construction.
pub fn parse_reasoning_wire_dialect_metadata(
    family: ApiFamily,
    value: &Value,
) -> Result<ReasoningWireDialect, ParameterEngineError> {
    let dialect = serde_json::from_value::<ReasoningWireDialect>(value.clone()).map_err(|_| {
        unsupported_reasoning("reasoning capability metadata has an invalid exact dialect schema")
    })?;
    validate_reasoning_dialect_family(family, &dialect)?;
    validate_reasoning_dialect_metadata_shape(&dialect)?;
    Ok(dialect)
}

/// Decodes a source-attributed structured capability value into an exact
/// provider prompt-cache contract.
pub fn parse_prompt_cache_wire_dialect_metadata(
    family: ApiFamily,
    value: &Value,
) -> Result<PromptCacheWireDialect, ParameterEngineError> {
    let dialect =
        serde_json::from_value::<PromptCacheWireDialect>(value.clone()).map_err(|_| {
            unsupported_cache(
                "prompt-cache capability metadata has an invalid exact dialect schema",
            )
        })?;
    validate_cache_dialect_family(family, dialect)?;
    Ok(dialect)
}

#[allow(clippy::too_many_lines)]
fn validate_reasoning_dialect_metadata_shape(
    dialect: &ReasoningWireDialect,
) -> Result<(), ParameterEngineError> {
    if let ReasoningWireDialect::OpenRouter {
        style,
        supported_efforts,
        default_effort,
        default_enabled,
        mandatory,
        ..
    } = dialect
    {
        validate_openrouter_reasoning_style(*style, supported_efforts)?;
        return validate_openrouter_reasoning_metadata(
            supported_efforts,
            *default_effort,
            *default_enabled,
            *mandatory,
        );
    }
    let (efforts, summaries) = match dialect {
        ReasoningWireDialect::Unsupported | ReasoningWireDialect::OllamaBoolean => {
            return Ok(());
        }
        ReasoningWireDialect::OpenAiResponses {
            efforts, summaries, ..
        }
        | ReasoningWireDialect::AnthropicAdaptive {
            efforts, summaries, ..
        }
        | ReasoningWireDialect::AnthropicExtended {
            efforts, summaries, ..
        }
        | ReasoningWireDialect::GeminiThinkingLevel {
            efforts, summaries, ..
        } => (efforts.as_slice(), summaries.as_slice()),
        ReasoningWireDialect::OpenAiChatCompletions { efforts, .. }
        | ReasoningWireDialect::OllamaLevel { efforts, .. } => {
            (efforts.as_slice(), &[] as &[ReasoningSummaryMode])
        }
        ReasoningWireDialect::OpenRouter { .. } => {
            unreachable!("OpenRouter metadata was validated above")
        }
        ReasoningWireDialect::GeminiThinkingBudget { summaries, .. } => {
            (&[] as &[ReasoningEffort], summaries.as_slice())
        }
    };
    if efforts.iter().copied().collect::<BTreeSet<_>>().len() != efforts.len()
        || summaries
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != summaries.len()
    {
        return Err(unsupported_reasoning(
            "reasoning capability metadata contains duplicate enum values",
        ));
    }
    match dialect {
        ReasoningWireDialect::AnthropicAdaptive {
            efforts, summaries, ..
        }
        | ReasoningWireDialect::AnthropicExtended {
            efforts, summaries, ..
        } if efforts.contains(&ReasoningEffort::Minimal)
            || summaries.iter().any(|summary| {
                matches!(
                    summary,
                    ReasoningSummaryMode::Concise | ReasoningSummaryMode::Detailed
                )
            }) =>
        {
            Err(unsupported_reasoning(
                "Anthropic reasoning metadata contains an unsupported effort or summary level",
            ))
        }
        ReasoningWireDialect::OllamaLevel { efforts, .. } if efforts.is_empty() => {
            Err(unsupported_reasoning(
                "Ollama reasoning level metadata requires at least one supported effort",
            ))
        }
        ReasoningWireDialect::OllamaLevel { efforts, .. }
            if efforts.iter().any(|effort| {
                !matches!(
                    effort,
                    ReasoningEffort::Low
                        | ReasoningEffort::Medium
                        | ReasoningEffort::High
                        | ReasoningEffort::Maximum
                )
            }) =>
        {
            Err(unsupported_reasoning(
                "Ollama native think levels are limited to low, medium, high, and max",
            ))
        }
        ReasoningWireDialect::AnthropicExtended {
            minimum_budget_tokens,
            maximum_budget_tokens,
            ..
        } if *minimum_budget_tokens == 0 || minimum_budget_tokens > maximum_budget_tokens => Err(
            unsupported_reasoning("Anthropic reasoning budget metadata has invalid bounds"),
        ),
        ReasoningWireDialect::GeminiThinkingBudget {
            minimum_budget_tokens,
            maximum_budget_tokens,
            ..
        } if *maximum_budget_tokens == 0 || minimum_budget_tokens > maximum_budget_tokens => Err(
            unsupported_reasoning("Gemini reasoning budget metadata has invalid bounds"),
        ),
        _ => Ok(()),
    }
}

fn validate_openrouter_reasoning_metadata(
    supported_efforts: &OpenRouterReasoningEffortSupport,
    default_effort: Option<OpenRouterReasoningEffort>,
    default_enabled: Option<bool>,
    mandatory: Option<bool>,
) -> Result<(), ParameterEngineError> {
    let exact_efforts = match supported_efforts {
        OpenRouterReasoningEffortSupport::NotExposed => None,
        OpenRouterReasoningEffortSupport::AllGateway => {
            Some(OpenRouterReasoningEffort::ALL.as_slice())
        }
        OpenRouterReasoningEffortSupport::Exact(efforts) => {
            if efforts.len() > OpenRouterReasoningEffort::ALL.len()
                || efforts.iter().copied().collect::<BTreeSet<_>>().len() != efforts.len()
            {
                return Err(unsupported_reasoning(
                    "OpenRouter reasoning metadata contains duplicate or excessive effort values",
                ));
            }
            Some(efforts.as_slice())
        }
    };
    if default_effort
        .is_some_and(|default| exact_efforts.is_none_or(|efforts| !efforts.contains(&default)))
    {
        return Err(unsupported_reasoning(
            "OpenRouter reasoning default is not an exposed supported effort",
        ));
    }
    let supports_none =
        exact_efforts.is_some_and(|efforts| efforts.contains(&OpenRouterReasoningEffort::None));
    if mandatory == Some(true)
        && (supports_none
            || default_effort == Some(OpenRouterReasoningEffort::None)
            || default_enabled == Some(false))
        || default_effort == Some(OpenRouterReasoningEffort::None) && default_enabled == Some(true)
    {
        return Err(unsupported_reasoning(
            "OpenRouter reasoning metadata contains contradictory defaults",
        ));
    }
    Ok(())
}

/// Inclusive seconds bounds for a custom cache TTL field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheTtlBounds {
    pub minimum_seconds: u32,
    pub maximum_seconds: u32,
}

/// Render-ready provider prompt-cache control derived entirely in Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptCacheControlModel {
    pub state: UiControlState,
    pub settings: PromptCacheSettings,
    pub allowed_modes: Vec<PromptCacheMode>,
    pub allowed_ttls: Vec<PromptCacheTtl>,
    /// Whether the enabled TTL field may accept a user-entered seconds value.
    ///
    /// Custom TTLs are represented separately from `allowed_ttls` because that
    /// list contains only concrete preset choices.
    pub supports_custom_ttl: bool,
    pub custom_ttl_bounds: Option<CacheTtlBounds>,
    pub ttl_field: UiFieldState,
    pub context_reference_field: UiFieldState,
    pub issues: Vec<ParameterIssue>,
}

/// A deterministic instruction the adapter must apply to prompt structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptCacheDirective {
    None,
    ProviderManagedAutomatic,
    AnthropicAutomatic {
        ttl: AnthropicCacheTtl,
    },
    AnthropicExplicitBreakpoints {
        ttl: AnthropicCacheTtl,
    },
    AnthropicDisabled,
    GeminiExplicitContext {
        resource_name: String,
        requested_ttl: PromptCacheTtl,
    },
}

/// Anthropic supports exactly its default short TTL or one hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicCacheTtl {
    FiveMinutes,
    OneHour,
}

/// A validated body field patch. Paths are adapter-owned JSON object fields.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestBodyPatch {
    pub(crate) path: String,
    pub(crate) value: Value,
}

impl RequestBodyPatch {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn value(&self) -> &Value {
        &self.value
    }
}

/// Complete parameter plan produced before any network request starts.
///
/// Its fields are crate-private and it intentionally does not implement
/// `Deserialize`, so external callers cannot forge a plan that bypasses
/// [`build_provider_request_plan`] or
/// [`validate_and_build_provider_request_plan`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderRequestPlan {
    pub(crate) family: ApiFamily,
    pub(crate) body_patches: Vec<RequestBodyPatch>,
    pub(crate) prompt_cache: PromptCacheDirective,
    pub(crate) cache_dialect: PromptCacheWireDialect,
    pub(crate) preserve_opaque_reasoning_state: bool,
}

impl ProviderRequestPlan {
    pub fn family(&self) -> ApiFamily {
        self.family
    }

    pub fn body_patches(&self) -> &[RequestBodyPatch] {
        &self.body_patches
    }

    pub fn prompt_cache(&self) -> &PromptCacheDirective {
        &self.prompt_cache
    }

    pub const fn cache_dialect(&self) -> PromptCacheWireDialect {
        self.cache_dialect
    }

    pub fn preserves_opaque_reasoning_state(&self) -> bool {
        self.preserve_opaque_reasoning_state
    }

    /// Applies patches without overwriting adapter-owned request fields.
    pub fn apply_to_body(&self, body: &mut Value) -> Result<(), ParameterEngineError> {
        let mut planned = body.clone();
        for patch in &self.body_patches {
            insert_json_path(&mut planned, &patch.path, patch.value.clone())?;
        }
        *body = planned;
        Ok(())
    }
}

/// Maps validated parameters, reasoning, and cache settings exactly for a
/// selected model route.
///
/// Returning `Err` is the network gate: callers must not construct or send a
/// request when this function rejects a combination.
pub fn build_provider_request_plan(
    family: ApiFamily,
    parameters: &ValidatedParameterSet,
    reasoning: &ReasoningSettings,
    reasoning_dialect: &ReasoningWireDialect,
    prompt_cache: &PromptCacheSettings,
    cache_dialect: PromptCacheWireDialect,
) -> Result<ProviderRequestPlan, ParameterEngineError> {
    validate_reasoning_dialect_family(family, reasoning_dialect)?;
    validate_reasoning_dialect_metadata_shape(reasoning_dialect)?;
    validate_cache_dialect_family(family, cache_dialect)?;
    validate_opaque_reasoning_preservation(family, reasoning)?;

    let mut patches = BTreeMap::<String, Value>::new();
    for parameter in parameters.applied() {
        let (path, value) = map_applied_parameter(family, parameter)?;
        insert_unique_patch(&mut patches, path, value, &parameter.parameter_id)?;
    }
    map_reasoning(family, reasoning, reasoning_dialect, &mut patches)?;
    let prompt_cache = map_prompt_cache(family, prompt_cache, cache_dialect, &mut patches)?;
    validate_cross_feature_combinations(family, reasoning, reasoning_dialect, &patches)?;

    Ok(ProviderRequestPlan {
        family,
        body_patches: patches
            .into_iter()
            .map(|(path, value)| RequestBodyPatch { path, value })
            .collect(),
        prompt_cache,
        cache_dialect,
        preserve_opaque_reasoning_state: reasoning.preserve_opaque_state
            && family != ApiFamily::GeminiGenerateContent,
    })
}

/// Validates a complete family-bound parameter contract and raw preset values,
/// then produces the only request-plan type adapters accept.
///
/// Callers resolving a model route should prefer this atomic gate over
/// separately validating preset values and mapping them later.
pub fn validate_and_build_provider_request_plan(
    engine: &ParameterEngine,
    family: ApiFamily,
    values: &[ParameterValue],
    reasoning: &ReasoningSettings,
    reasoning_dialect: &ReasoningWireDialect,
    prompt_cache: &PromptCacheSettings,
    cache_dialect: PromptCacheWireDialect,
) -> Result<ProviderRequestPlan, ParameterEngineError> {
    engine.validate_mappings_for_family(family)?;
    let parameters = engine.validate_for_request(values)?;
    build_provider_request_plan(
        family,
        &parameters,
        reasoning,
        reasoning_dialect,
        prompt_cache,
        cache_dialect,
    )
}

/// Computes the exact reasoning controls native UIs may display for a route.
#[allow(clippy::match_same_arms, clippy::too_many_lines)]
pub fn render_reasoning_control(
    family: ApiFamily,
    dialect: &ReasoningWireDialect,
    settings: &ReasoningSettings,
) -> ReasoningControlModel {
    let (allowed_modes, allowed_efforts, allowed_summaries, budget_bounds) = match dialect {
        ReasoningWireDialect::Unsupported => (Vec::new(), Vec::new(), Vec::new(), None),
        ReasoningWireDialect::OpenAiResponses {
            efforts,
            summaries,
            supports_disabled,
        } => (
            modes_with_optional_disabled(
                &[ReasoningMode::ProviderDefault, ReasoningMode::Enabled],
                *supports_disabled,
            ),
            efforts.clone(),
            summaries_with_provider_default(summaries),
            None,
        ),
        ReasoningWireDialect::OpenAiChatCompletions {
            efforts,
            supports_disabled,
        } => {
            let base_modes = if efforts.is_empty() {
                vec![ReasoningMode::ProviderDefault]
            } else {
                vec![ReasoningMode::ProviderDefault, ReasoningMode::Enabled]
            };
            (
                modes_with_optional_disabled(&base_modes, *supports_disabled),
                efforts.clone(),
                vec![ReasoningSummaryMode::ProviderDefault],
                None,
            )
        }
        ReasoningWireDialect::OpenRouter {
            style,
            supported_efforts,
            mandatory,
            ..
        } => {
            let efforts = openrouter_reasoning_efforts(supported_efforts);
            let supports_disabled =
                openrouter_supports_disabled(*style, supported_efforts, *mandatory);
            let mut modes = vec![ReasoningMode::ProviderDefault];
            if openrouter_supports_enabled(*style, supported_efforts) {
                modes.push(ReasoningMode::Enabled);
            }
            if supports_disabled {
                modes.push(ReasoningMode::Disabled);
            }
            (
                modes,
                efforts,
                vec![ReasoningSummaryMode::ProviderDefault],
                None,
            )
        }
        ReasoningWireDialect::AnthropicAdaptive {
            efforts,
            summaries,
            supports_disabled,
        } => (
            modes_with_optional_disabled(
                &[ReasoningMode::ProviderDefault, ReasoningMode::Automatic],
                *supports_disabled,
            ),
            efforts.clone(),
            summaries_with_provider_default(summaries),
            None,
        ),
        ReasoningWireDialect::AnthropicExtended {
            minimum_budget_tokens,
            maximum_budget_tokens,
            efforts,
            summaries,
            supports_disabled,
        } => (
            modes_with_optional_disabled(
                &[ReasoningMode::ProviderDefault, ReasoningMode::Enabled],
                *supports_disabled,
            ),
            efforts.clone(),
            summaries_with_provider_default(summaries),
            Some(TokenBudgetBounds {
                minimum: *minimum_budget_tokens,
                maximum: *maximum_budget_tokens,
            }),
        ),
        ReasoningWireDialect::GeminiThinkingLevel {
            efforts,
            supports_automatic,
            summaries,
        } => (
            if *supports_automatic {
                vec![ReasoningMode::ProviderDefault, ReasoningMode::Automatic]
            } else {
                vec![ReasoningMode::ProviderDefault]
            },
            efforts.clone(),
            summaries_with_provider_default(summaries),
            None,
        ),
        ReasoningWireDialect::GeminiThinkingBudget {
            minimum_budget_tokens,
            maximum_budget_tokens,
            supports_zero_to_disable,
            supports_automatic,
            summaries,
        } => {
            let mut modes = vec![ReasoningMode::ProviderDefault, ReasoningMode::Enabled];
            if *supports_automatic {
                modes.push(ReasoningMode::Automatic);
            }
            if *supports_zero_to_disable {
                modes.push(ReasoningMode::Disabled);
            }
            (
                modes,
                Vec::new(),
                summaries_with_provider_default(summaries),
                Some(TokenBudgetBounds {
                    minimum: *minimum_budget_tokens,
                    maximum: *maximum_budget_tokens,
                }),
            )
        }
        ReasoningWireDialect::OllamaBoolean => (
            vec![
                ReasoningMode::ProviderDefault,
                ReasoningMode::Enabled,
                ReasoningMode::Disabled,
            ],
            Vec::new(),
            vec![ReasoningSummaryMode::ProviderDefault],
            None,
        ),
        ReasoningWireDialect::OllamaLevel {
            efforts,
            supports_disabled,
        } => (
            modes_with_optional_disabled(
                &[ReasoningMode::ProviderDefault, ReasoningMode::Enabled],
                *supports_disabled,
            ),
            efforts.clone(),
            vec![ReasoningSummaryMode::ProviderDefault],
            None,
        ),
    };

    let mut rendered_settings = settings.clone();
    if let ReasoningWireDialect::OpenRouter {
        default_effort: Some(default_effort),
        ..
    } = dialect
        && rendered_settings.mode == ReasoningMode::Enabled
        && rendered_settings.effort.is_none()
    {
        rendered_settings.effort = openrouter_effort_to_reasoning_effort(*default_effort);
    }
    let mut patches = BTreeMap::new();
    let issues = validate_reasoning_dialect_family(family, dialect)
        .and_then(|()| validate_reasoning_dialect_metadata_shape(dialect))
        .and_then(|()| validate_opaque_reasoning_preservation(family, &rendered_settings))
        .and_then(|()| map_reasoning(family, &rendered_settings, dialect, &mut patches))
        .err()
        .map_or_else(Vec::new, |error| error.issues);
    let visible = !matches!(dialect, ReasoningWireDialect::Unsupported);
    let effort_enabled = visible
        && settings.mode != ReasoningMode::Disabled
        && !allowed_efforts.is_empty()
        && !matches!(
            dialect,
            ReasoningWireDialect::OpenRouter { .. }
                if settings.mode != ReasoningMode::Enabled
        )
        && !matches!(
            dialect,
            ReasoningWireDialect::OllamaLevel { .. }
                if settings.mode != ReasoningMode::Enabled
        )
        && !matches!(
            dialect,
            ReasoningWireDialect::GeminiThinkingLevel { .. }
                if settings.mode != ReasoningMode::Automatic
        );
    let budget_enabled =
        visible && settings.mode == ReasoningMode::Enabled && budget_bounds.is_some();
    let summary_enabled = visible
        && settings.mode != ReasoningMode::Disabled
        && !matches!(
            dialect,
            ReasoningWireDialect::AnthropicAdaptive { .. }
                | ReasoningWireDialect::AnthropicExtended { .. }
                if settings.mode == ReasoningMode::ProviderDefault
        )
        && allowed_summaries
            .iter()
            .any(|summary| *summary != ReasoningSummaryMode::ProviderDefault);
    if family == ApiFamily::GeminiGenerateContent {
        rendered_settings.preserve_opaque_state = false;
    }
    ReasoningControlModel {
        state: if !visible {
            UiControlState::Hidden
        } else if issues.is_empty() {
            UiControlState::Ready
        } else {
            UiControlState::Invalid
        },
        settings: rendered_settings,
        allowed_modes,
        allowed_efforts,
        allowed_summaries,
        budget_bounds,
        effort_field: if effort_enabled
            && matches!(dialect, ReasoningWireDialect::OpenRouter { .. })
        {
            UiFieldState::Required
        } else if effort_enabled {
            UiFieldState::Enabled
        } else {
            UiFieldState::Hidden
        },
        budget_field: if budget_enabled {
            UiFieldState::Enabled
        } else {
            UiFieldState::Hidden
        },
        summary_field: if summary_enabled {
            UiFieldState::Enabled
        } else {
            UiFieldState::Hidden
        },
        issues,
    }
}

fn openrouter_reasoning_efforts(
    supported: &OpenRouterReasoningEffortSupport,
) -> Vec<ReasoningEffort> {
    let efforts = match supported {
        OpenRouterReasoningEffortSupport::NotExposed => &[][..],
        OpenRouterReasoningEffortSupport::AllGateway => OpenRouterReasoningEffort::ALL.as_slice(),
        OpenRouterReasoningEffortSupport::Exact(efforts) => efforts,
    };
    efforts
        .iter()
        .filter_map(|effort| match effort {
            OpenRouterReasoningEffort::Max => Some(ReasoningEffort::Maximum),
            OpenRouterReasoningEffort::Xhigh => Some(ReasoningEffort::ExtraHigh),
            OpenRouterReasoningEffort::High => Some(ReasoningEffort::High),
            OpenRouterReasoningEffort::Medium => Some(ReasoningEffort::Medium),
            OpenRouterReasoningEffort::Low => Some(ReasoningEffort::Low),
            OpenRouterReasoningEffort::Minimal => Some(ReasoningEffort::Minimal),
            OpenRouterReasoningEffort::None => None,
        })
        .collect()
}

fn openrouter_supports_disabled(
    style: OpenRouterReasoningWireStyle,
    supported: &OpenRouterReasoningEffortSupport,
    mandatory: Option<bool>,
) -> bool {
    if mandatory == Some(true) {
        return false;
    }
    let exact_none = match supported {
        OpenRouterReasoningEffortSupport::AllGateway => true,
        OpenRouterReasoningEffortSupport::Exact(efforts) => {
            efforts.contains(&OpenRouterReasoningEffort::None)
        }
        OpenRouterReasoningEffortSupport::NotExposed => false,
    };
    match style {
        OpenRouterReasoningWireStyle::Unified => mandatory == Some(false) || exact_none,
        OpenRouterReasoningWireStyle::LegacyReasoningEffort => exact_none,
    }
}

fn openrouter_supports_enabled(
    style: OpenRouterReasoningWireStyle,
    supported: &OpenRouterReasoningEffortSupport,
) -> bool {
    match style {
        OpenRouterReasoningWireStyle::Unified => match supported {
            OpenRouterReasoningEffortSupport::NotExposed
            | OpenRouterReasoningEffortSupport::AllGateway => true,
            OpenRouterReasoningEffortSupport::Exact(efforts) => {
                efforts.is_empty()
                    || efforts
                        .iter()
                        .any(|effort| *effort != OpenRouterReasoningEffort::None)
            }
        },
        OpenRouterReasoningWireStyle::LegacyReasoningEffort => match supported {
            OpenRouterReasoningEffortSupport::AllGateway => true,
            OpenRouterReasoningEffortSupport::Exact(efforts) => efforts
                .iter()
                .any(|effort| *effort != OpenRouterReasoningEffort::None),
            OpenRouterReasoningEffortSupport::NotExposed => false,
        },
    }
}

fn validate_openrouter_reasoning_style(
    style: OpenRouterReasoningWireStyle,
    supported: &OpenRouterReasoningEffortSupport,
) -> Result<(), ParameterEngineError> {
    if style == OpenRouterReasoningWireStyle::LegacyReasoningEffort
        && matches!(supported, OpenRouterReasoningEffortSupport::NotExposed)
    {
        return Err(unsupported_reasoning(
            "legacy OpenRouter reasoning_effort requires an exposed supported effort list",
        ));
    }
    Ok(())
}

fn openrouter_enabled_patch_path(style: OpenRouterReasoningWireStyle) -> &'static str {
    match style {
        OpenRouterReasoningWireStyle::Unified => "reasoning.effort",
        OpenRouterReasoningWireStyle::LegacyReasoningEffort => "reasoning_effort",
    }
}

fn insert_openrouter_boolean_patch(
    style: OpenRouterReasoningWireStyle,
    patches: &mut BTreeMap<String, Value>,
    enabled: bool,
) -> Result<(), ParameterEngineError> {
    match style {
        OpenRouterReasoningWireStyle::Unified => {
            insert_normalized_patch(patches, "reasoning.enabled", json!(enabled))
        }
        OpenRouterReasoningWireStyle::LegacyReasoningEffort if !enabled => {
            insert_normalized_patch(patches, "reasoning_effort", json!("none"))
        }
        OpenRouterReasoningWireStyle::LegacyReasoningEffort => Err(unsupported_reasoning(
            "legacy OpenRouter reasoning requires an explicit effort",
        )),
    }
}

const fn openrouter_effort_to_reasoning_effort(
    effort: OpenRouterReasoningEffort,
) -> Option<ReasoningEffort> {
    match effort {
        OpenRouterReasoningEffort::Max => Some(ReasoningEffort::Maximum),
        OpenRouterReasoningEffort::Xhigh => Some(ReasoningEffort::ExtraHigh),
        OpenRouterReasoningEffort::High => Some(ReasoningEffort::High),
        OpenRouterReasoningEffort::Medium => Some(ReasoningEffort::Medium),
        OpenRouterReasoningEffort::Low => Some(ReasoningEffort::Low),
        OpenRouterReasoningEffort::Minimal => Some(ReasoningEffort::Minimal),
        OpenRouterReasoningEffort::None => None,
    }
}

/// Computes the exact provider prompt-cache controls native UIs may display.
#[allow(clippy::too_many_lines)]
pub fn render_prompt_cache_control(
    family: ApiFamily,
    dialect: PromptCacheWireDialect,
    settings: &PromptCacheSettings,
) -> PromptCacheControlModel {
    let (allowed_modes, mut allowed_ttls, mut custom_ttl_bounds) = match dialect {
        PromptCacheWireDialect::Unsupported => (Vec::new(), Vec::new(), None),
        PromptCacheWireDialect::OpenAiAutomatic {
            supports_24_hour_retention,
        } => {
            let mut ttls = vec![PromptCacheTtl::ProviderDefault, PromptCacheTtl::Short];
            if supports_24_hour_retention {
                ttls.push(PromptCacheTtl::Long);
            }
            (
                vec![PromptCacheMode::ProviderDefault, PromptCacheMode::Automatic],
                ttls,
                None,
            )
        }
        PromptCacheWireDialect::Anthropic {
            supports_automatic,
            supports_explicit_breakpoints,
            supports_one_hour_ttl,
        } => {
            let mut modes = vec![
                PromptCacheMode::ProviderDefault,
                PromptCacheMode::DisabledIfSupported,
            ];
            if supports_automatic {
                modes.push(PromptCacheMode::Automatic);
            }
            if supports_explicit_breakpoints {
                modes.push(PromptCacheMode::ExplicitBreakpoints);
            }
            let mut ttls = vec![PromptCacheTtl::ProviderDefault, PromptCacheTtl::Short];
            if supports_one_hour_ttl {
                ttls.push(PromptCacheTtl::Long);
            }
            (modes, ttls, None)
        }
        PromptCacheWireDialect::Gemini {
            supports_implicit,
            supports_explicit_context,
        } => {
            let mut modes = vec![PromptCacheMode::ProviderDefault];
            if supports_implicit {
                modes.push(PromptCacheMode::Automatic);
            }
            if supports_explicit_context {
                modes.push(PromptCacheMode::ExplicitContext);
            }
            // `generateContent` can select an existing cachedContents
            // resource, but that request cannot create/update the resource TTL.
            // Resource lifecycle needs a separate Core workflow before any TTL
            // choice can be exposed here.
            (modes, vec![PromptCacheTtl::ProviderDefault], None)
        }
    };

    let mut patches = BTreeMap::new();
    let issues = validate_cache_dialect_family(family, dialect)
        .and_then(|()| map_prompt_cache(family, settings, dialect, &mut patches))
        .err()
        .map_or_else(Vec::new, |error| error.issues);
    let visible = !matches!(dialect, PromptCacheWireDialect::Unsupported);
    let ttl_enabled = matches!(
        (dialect, settings.mode),
        (
            PromptCacheWireDialect::OpenAiAutomatic { .. },
            PromptCacheMode::Automatic
        ) | (
            PromptCacheWireDialect::Anthropic { .. },
            PromptCacheMode::Automatic | PromptCacheMode::ExplicitBreakpoints
        )
    );
    if !ttl_enabled {
        allowed_ttls = vec![PromptCacheTtl::ProviderDefault];
        custom_ttl_bounds = None;
    }
    let context_reference_visible = settings.mode == PromptCacheMode::ExplicitContext
        && matches!(dialect, PromptCacheWireDialect::Gemini { .. });
    let supports_custom_ttl = custom_ttl_bounds.is_some();
    PromptCacheControlModel {
        state: if !visible {
            UiControlState::Hidden
        } else if issues.is_empty() {
            UiControlState::Ready
        } else {
            UiControlState::Invalid
        },
        settings: settings.clone(),
        allowed_modes,
        allowed_ttls,
        supports_custom_ttl,
        custom_ttl_bounds,
        ttl_field: if ttl_enabled {
            UiFieldState::Enabled
        } else {
            UiFieldState::Hidden
        },
        context_reference_field: if context_reference_visible {
            UiFieldState::Required
        } else {
            UiFieldState::Hidden
        },
        issues,
    }
}

fn modes_with_optional_disabled(
    base: &[ReasoningMode],
    supports_disabled: bool,
) -> Vec<ReasoningMode> {
    let mut modes = base.to_vec();
    if supports_disabled {
        modes.push(ReasoningMode::Disabled);
    }
    modes
}

fn summaries_with_provider_default(
    summaries: &[ReasoningSummaryMode],
) -> Vec<ReasoningSummaryMode> {
    let mut values = vec![ReasoningSummaryMode::ProviderDefault];
    for summary in summaries {
        if !values.contains(summary) {
            values.push(*summary);
        }
    }
    values
}

fn validate_cross_feature_combinations(
    family: ApiFamily,
    reasoning: &ReasoningSettings,
    reasoning_dialect: &ReasoningWireDialect,
    patches: &BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    let anthropic_thinking_active = family == ApiFamily::AnthropicMessages
        && matches!(
            (reasoning_dialect, reasoning.mode),
            (
                ReasoningWireDialect::AnthropicAdaptive { .. },
                ReasoningMode::Automatic
            ) | (
                ReasoningWireDialect::AnthropicExtended { .. },
                ReasoningMode::Enabled
            )
        );
    let anthropic_manual_thinking_active = family == ApiFamily::AnthropicMessages
        && matches!(
            reasoning_dialect,
            ReasoningWireDialect::AnthropicExtended { .. }
        )
        && reasoning.mode == ReasoningMode::Enabled;
    if anthropic_thinking_active {
        if anthropic_manual_thinking_active
            && patches
                .get("tool_choice")
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str)
                .is_some_and(|choice| !matches!(choice, "auto" | "none"))
        {
            return Err(unsupported_reasoning(
                "Anthropic active thinking cannot force tool use",
            ));
        }
        if patches.contains_key("temperature") || patches.contains_key("top_k") {
            return Err(unsupported_reasoning(
                "Anthropic thinking cannot be combined with temperature or top_k",
            ));
        }
        if patches
            .get("top_p")
            .and_then(Value::as_f64)
            .is_some_and(|top_p| !(0.95..=1.0).contains(&top_p))
        {
            return Err(unsupported_reasoning(
                "Anthropic thinking requires top_p between 0.95 and 1",
            ));
        }
    }
    if anthropic_manual_thinking_active
        && let (Some(max_tokens), Some(budget_tokens)) = (
            patches.get("max_tokens").and_then(Value::as_u64),
            reasoning.budget_tokens.map(u64::from),
        )
        && budget_tokens >= max_tokens
    {
        return Err(unsupported_reasoning(
            "Anthropic thinking budget must be less than max_tokens",
        ));
    }

    if family == ApiFamily::OpenAiChatCompletions
        && patches.contains_key("max_tokens")
        && patches.contains_key("max_completion_tokens")
    {
        return Err(ParameterEngineError::one(ParameterIssue::new(
            ParameterIssueCode::ConflictingRequestField,
            None,
            None,
            "max_tokens and max_completion_tokens are mutually exclusive",
        )));
    }

    if family == ApiFamily::GeminiGenerateContent
        && patches.contains_key("generationConfig.responseJsonSchema")
        && patches
            .get("generationConfig.responseMimeType")
            .and_then(Value::as_str)
            != Some("application/json")
    {
        return Err(ParameterEngineError::one(ParameterIssue::new(
            ParameterIssueCode::UnsupportedMapping,
            None,
            None,
            "Gemini responseJsonSchema requires responseMimeType application/json",
        )));
    }
    Ok(())
}

fn validate_reasoning_dialect_family(
    family: ApiFamily,
    dialect: &ReasoningWireDialect,
) -> Result<(), ParameterEngineError> {
    let matches = matches!(
        (family, dialect),
        (_, ReasoningWireDialect::Unsupported)
            | (
                ApiFamily::OpenAiResponses,
                ReasoningWireDialect::OpenAiResponses { .. }
            )
            | (
                ApiFamily::OpenAiChatCompletions,
                ReasoningWireDialect::OpenAiChatCompletions { .. }
                    | ReasoningWireDialect::OpenRouter { .. }
            )
            | (
                ApiFamily::AnthropicMessages,
                ReasoningWireDialect::AnthropicAdaptive { .. }
                    | ReasoningWireDialect::AnthropicExtended { .. }
            )
            | (
                ApiFamily::GeminiGenerateContent,
                ReasoningWireDialect::GeminiThinkingLevel { .. }
                    | ReasoningWireDialect::GeminiThinkingBudget { .. }
            )
            | (
                ApiFamily::OllamaNative,
                ReasoningWireDialect::OllamaBoolean | ReasoningWireDialect::OllamaLevel { .. }
            )
    );
    if matches {
        Ok(())
    } else {
        Err(unsupported_reasoning(
            "reasoning capability dialect does not match the API family",
        ))
    }
}

fn validate_cache_dialect_family(
    family: ApiFamily,
    dialect: PromptCacheWireDialect,
) -> Result<(), ParameterEngineError> {
    let matches = matches!(
        (family, dialect),
        (_, PromptCacheWireDialect::Unsupported)
            | (
                ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions,
                PromptCacheWireDialect::OpenAiAutomatic { .. }
            )
            | (
                ApiFamily::AnthropicMessages,
                PromptCacheWireDialect::Anthropic { .. }
            )
            | (
                ApiFamily::GeminiGenerateContent,
                PromptCacheWireDialect::Gemini { .. }
            )
    );
    if matches {
        Ok(())
    } else {
        Err(unsupported_cache(
            "prompt-cache capability dialect does not match the API family",
        ))
    }
}

fn insert_unique_patch(
    patches: &mut BTreeMap<String, Value>,
    path: String,
    value: Value,
    parameter_id: &ParameterId,
) -> Result<(), ParameterEngineError> {
    if patches.insert(path.clone(), value).is_some() {
        Err(ParameterEngineError::one(ParameterIssue::new(
            ParameterIssueCode::ConflictingRequestField,
            Some(parameter_id.clone()),
            None,
            format!("more than one setting writes request field {path}"),
        )))
    } else {
        Ok(())
    }
}

fn insert_normalized_patch(
    patches: &mut BTreeMap<String, Value>,
    path: &str,
    value: Value,
) -> Result<(), ParameterEngineError> {
    if patches.insert(path.to_owned(), value).is_some() {
        Err(ParameterEngineError::one(ParameterIssue::new(
            ParameterIssueCode::ConflictingRequestField,
            None,
            None,
            format!("normalized setting conflicts with request field {path}"),
        )))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AllowedWireValue {
    Boolean,
    Integer,
    Number,
    String,
    Enum,
    StopSequenceList,
    JsonSchema,
    ToolPolicy,
}

fn validate_spec_mapping_for_family(
    family: ApiFamily,
    spec: &TypedParameterSpec,
) -> Result<(), ParameterEngineError> {
    let expected = validate_mapping_target(
        family,
        &spec.id,
        spec.provider_mapping.target,
        &spec.provider_mapping.field_name,
    )?;
    let actual = schema_wire_value(&spec.schema).ok_or_else(|| {
        unsupported_mapping(
            &spec.id,
            "this parameter schema has no exact mapping in the closed adapter families",
        )
    })?;
    if actual == expected {
        Ok(())
    } else {
        Err(unsupported_mapping(
            &spec.id,
            format!(
                "request field {} cannot represent this parameter schema without loss",
                spec.provider_mapping.field_name
            ),
        ))
    }
}

fn schema_wire_value(schema: &ParameterSchema) -> Option<AllowedWireValue> {
    match schema {
        ParameterSchema::Boolean => Some(AllowedWireValue::Boolean),
        ParameterSchema::Integer { .. } => Some(AllowedWireValue::Integer),
        ParameterSchema::Number { .. } => Some(AllowedWireValue::Number),
        ParameterSchema::String { .. } => Some(AllowedWireValue::String),
        ParameterSchema::Enum { .. } => Some(AllowedWireValue::Enum),
        ParameterSchema::JsonSchema { .. } => Some(AllowedWireValue::JsonSchema),
        ParameterSchema::StopSequenceList { .. } => Some(AllowedWireValue::StopSequenceList),
        ParameterSchema::ToolPolicy { .. } => Some(AllowedWireValue::ToolPolicy),
        ParameterSchema::StringList { .. } => None,
    }
}

fn map_applied_parameter(
    family: ApiFamily,
    parameter: &AppliedParameter,
) -> Result<(String, Value), ParameterEngineError> {
    let field = parameter.mapping.field_name.as_str();
    let expected = validate_mapping_target(
        family,
        &parameter.parameter_id,
        parameter.mapping.target,
        field,
    )?;
    let value = literal_to_wire(family, field, expected, &parameter.value).map_err(|message| {
        unsupported_mapping(&parameter.parameter_id, format!("{field}: {message}"))
    })?;
    Ok((field.to_owned(), value))
}

fn validate_mapping_target(
    family: ApiFamily,
    parameter_id: &ParameterId,
    target: ProviderParameterTarget,
    field: &str,
) -> Result<AllowedWireValue, ParameterEngineError> {
    if target == ProviderParameterTarget::RequestHeader {
        return Err(unsupported_mapping(
            parameter_id,
            "generation parameters may not write request headers",
        ));
    }
    if reserved_parameter_field(field) {
        return Err(unsupported_mapping(
            parameter_id,
            "adapter-owned identity, content, stream, reasoning, cache, or security fields may not be overridden",
        ));
    }
    allowed_parameter_field(family, field).ok_or_else(|| {
        unsupported_mapping(
            parameter_id,
            format!("request field {field} is not allowlisted for this API family"),
        )
    })
}

fn reserved_parameter_field(field: &str) -> bool {
    let lower = field.to_ascii_lowercase();
    let root = lower.split('.').next().unwrap_or_default();
    matches!(
        root,
        "model"
            | "messages"
            | "message"
            | "input"
            | "contents"
            | "system"
            | "instructions"
            | "tools"
            | "stream"
            | "timeout"
            | "url"
            | "endpoint"
            | "authorization"
            | "api_key"
            | "apikey"
            | "metadata"
            | "reasoning"
            | "reasoning_effort"
            | "thinking"
            | "cache_control"
            | "cachedcontent"
            | "prompt_cache_key"
            | "prompt_cache_retention"
    ) || lower.starts_with("generationconfig.thinkingconfig.")
        || lower.starts_with("output_config.effort")
}

#[allow(clippy::match_same_arms)]
fn allowed_parameter_field(family: ApiFamily, field: &str) -> Option<AllowedWireValue> {
    use AllowedWireValue as Kind;

    match (family, field) {
        (ApiFamily::OpenAiResponses, "temperature" | "top_p") => Some(Kind::Number),
        (ApiFamily::OpenAiResponses, "max_output_tokens") => Some(Kind::Integer),
        (ApiFamily::OpenAiResponses, "service_tier" | "truncation") => Some(Kind::Enum),
        (ApiFamily::OpenAiResponses, "parallel_tool_calls") => Some(Kind::Boolean),
        (ApiFamily::OpenAiResponses, "tool_choice") => Some(Kind::ToolPolicy),

        (
            ApiFamily::OpenAiChatCompletions,
            "temperature" | "top_p" | "presence_penalty" | "frequency_penalty",
        ) => Some(Kind::Number),
        (ApiFamily::OpenAiChatCompletions, "max_tokens" | "max_completion_tokens" | "seed") => {
            Some(Kind::Integer)
        }
        (ApiFamily::OpenAiChatCompletions, "stop") => Some(Kind::StopSequenceList),
        (ApiFamily::OpenAiChatCompletions, "service_tier") => Some(Kind::Enum),
        (ApiFamily::OpenAiChatCompletions, "parallel_tool_calls" | "logprobs") => {
            Some(Kind::Boolean)
        }
        (ApiFamily::OpenAiChatCompletions, "tool_choice") => Some(Kind::ToolPolicy),

        (ApiFamily::AnthropicMessages, "temperature" | "top_p") => Some(Kind::Number),
        (ApiFamily::AnthropicMessages, "max_tokens" | "top_k") => Some(Kind::Integer),
        (ApiFamily::AnthropicMessages, "stop_sequences") => Some(Kind::StopSequenceList),
        (ApiFamily::AnthropicMessages, "tool_choice") => Some(Kind::ToolPolicy),
        (ApiFamily::AnthropicMessages, "output_config.format") => Some(Kind::JsonSchema),

        (
            ApiFamily::GeminiGenerateContent,
            "generationConfig.temperature"
            | "generationConfig.topP"
            | "generationConfig.presencePenalty"
            | "generationConfig.frequencyPenalty",
        ) => Some(Kind::Number),
        (
            ApiFamily::GeminiGenerateContent,
            "generationConfig.maxOutputTokens" | "generationConfig.topK" | "generationConfig.seed",
        ) => Some(Kind::Integer),
        (ApiFamily::GeminiGenerateContent, "generationConfig.stopSequences") => {
            Some(Kind::StopSequenceList)
        }
        (ApiFamily::GeminiGenerateContent, "generationConfig.responseMimeType") => {
            Some(Kind::String)
        }
        (ApiFamily::GeminiGenerateContent, "generationConfig.responseLogprobs") => {
            Some(Kind::Boolean)
        }
        (ApiFamily::GeminiGenerateContent, "generationConfig.responseJsonSchema") => {
            Some(Kind::JsonSchema)
        }
        (ApiFamily::GeminiGenerateContent, "toolConfig.functionCallingConfig.mode") => {
            Some(Kind::ToolPolicy)
        }

        (ApiFamily::OllamaNative, "options.temperature" | "options.top_p" | "options.min_p") => {
            Some(Kind::Number)
        }
        (ApiFamily::OllamaNative, "options.num_predict" | "options.top_k" | "options.seed") => {
            Some(Kind::Integer)
        }
        (ApiFamily::OllamaNative, "options.stop") => Some(Kind::StopSequenceList),
        (ApiFamily::OllamaNative, "keep_alive") => Some(Kind::String),
        (ApiFamily::OllamaNative, "format") => Some(Kind::JsonSchema),
        _ => None,
    }
}

#[allow(clippy::match_same_arms)]
fn literal_to_wire(
    family: ApiFamily,
    field: &str,
    expected: AllowedWireValue,
    literal: &ParameterLiteral,
) -> Result<Value, &'static str> {
    match (expected, literal) {
        (AllowedWireValue::Boolean, ParameterLiteral::Boolean(value)) => Ok(json!(value)),
        (AllowedWireValue::Integer, ParameterLiteral::Integer(value)) => Ok(json!(value)),
        (AllowedWireValue::Number, ParameterLiteral::Number(value)) if value.is_finite() => {
            Ok(json!(value))
        }
        (AllowedWireValue::String, ParameterLiteral::String(value)) => Ok(json!(value)),
        (AllowedWireValue::Enum, ParameterLiteral::Enum(value)) => Ok(json!(value)),
        (AllowedWireValue::StopSequenceList, ParameterLiteral::StopSequenceList(value)) => {
            Ok(json!(value))
        }
        (AllowedWireValue::JsonSchema, ParameterLiteral::JsonSchema(value)) => {
            let parsed: Value =
                serde_json::from_str(value).map_err(|_| "JSON schema is invalid")?;
            if parsed.is_object() {
                if family == ApiFamily::AnthropicMessages && field == "output_config.format" {
                    Ok(json!({
                        "type": "json_schema",
                        "schema": parsed,
                    }))
                } else {
                    Ok(parsed)
                }
            } else {
                Err("JSON schema must be an object")
            }
        }
        (AllowedWireValue::ToolPolicy, ParameterLiteral::ToolPolicy(policy)) => {
            map_tool_policy(family, field, *policy)
        }
        _ => Err("literal type does not match the allowlisted request field"),
    }
}

fn map_tool_policy(
    family: ApiFamily,
    field: &str,
    policy: ToolPolicy,
) -> Result<Value, &'static str> {
    match (family, field, policy) {
        (
            ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions,
            "tool_choice",
            ToolPolicy::None,
        ) => Ok(json!("none")),
        (
            ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions,
            "tool_choice",
            ToolPolicy::Auto,
        ) => Ok(json!("auto")),
        (
            ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions,
            "tool_choice",
            ToolPolicy::Required,
        ) => Ok(json!("required")),
        (ApiFamily::AnthropicMessages, "tool_choice", ToolPolicy::None) => {
            Ok(json!({ "type": "none" }))
        }
        (ApiFamily::AnthropicMessages, "tool_choice", ToolPolicy::Auto) => {
            Ok(json!({ "type": "auto" }))
        }
        (ApiFamily::AnthropicMessages, "tool_choice", ToolPolicy::Required) => {
            Ok(json!({ "type": "any" }))
        }
        (
            ApiFamily::GeminiGenerateContent,
            "toolConfig.functionCallingConfig.mode",
            ToolPolicy::None,
        ) => Ok(json!("NONE")),
        (
            ApiFamily::GeminiGenerateContent,
            "toolConfig.functionCallingConfig.mode",
            ToolPolicy::Auto,
        ) => Ok(json!("AUTO")),
        (
            ApiFamily::GeminiGenerateContent,
            "toolConfig.functionCallingConfig.mode",
            ToolPolicy::Required,
        ) => Ok(json!("ANY")),
        _ => Err("tool policy has no exact mapping for this request field"),
    }
}

fn unsupported_mapping(
    parameter_id: &ParameterId,
    message: impl Into<String>,
) -> ParameterEngineError {
    ParameterEngineError::one(ParameterIssue::new(
        ParameterIssueCode::UnsupportedMapping,
        Some(parameter_id.clone()),
        None,
        message,
    ))
}

fn map_reasoning(
    family: ApiFamily,
    settings: &ReasoningSettings,
    dialect: &ReasoningWireDialect,
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    validate_reasoning_shape(settings)?;

    if settings.mode == ReasoningMode::ProviderDefault
        && settings.effort.is_none()
        && settings.budget_tokens.is_none()
        && settings.summary == ReasoningSummaryMode::ProviderDefault
    {
        return Ok(());
    }

    match dialect {
        ReasoningWireDialect::Unsupported => Err(unsupported_reasoning(
            "the selected model route has no observed reasoning control",
        )),
        ReasoningWireDialect::OpenAiResponses {
            efforts,
            summaries,
            supports_disabled,
        } => map_openai_responses_reasoning(
            settings,
            efforts,
            summaries,
            *supports_disabled,
            patches,
        ),
        ReasoningWireDialect::OpenAiChatCompletions {
            efforts,
            supports_disabled,
        } => map_openai_chat_reasoning(settings, efforts, *supports_disabled, patches),
        ReasoningWireDialect::OpenRouter {
            style,
            supported_efforts,
            supports_max_tokens,
            mandatory,
            ..
        } => map_openrouter_reasoning(
            settings,
            *style,
            supported_efforts,
            *supports_max_tokens,
            *mandatory,
            patches,
        ),
        ReasoningWireDialect::AnthropicAdaptive {
            efforts,
            summaries,
            supports_disabled,
        } => map_anthropic_adaptive(settings, efforts, summaries, *supports_disabled, patches),
        ReasoningWireDialect::AnthropicExtended {
            minimum_budget_tokens,
            maximum_budget_tokens,
            efforts,
            summaries,
            supports_disabled,
        } => map_anthropic_extended(
            settings,
            *minimum_budget_tokens,
            *maximum_budget_tokens,
            efforts,
            summaries,
            *supports_disabled,
            patches,
        ),
        ReasoningWireDialect::GeminiThinkingLevel {
            efforts,
            supports_automatic,
            summaries,
        } => map_gemini_level(settings, efforts, *supports_automatic, summaries, patches),
        ReasoningWireDialect::GeminiThinkingBudget {
            minimum_budget_tokens,
            maximum_budget_tokens,
            supports_zero_to_disable,
            supports_automatic,
            summaries,
        } => map_gemini_budget(
            settings,
            *minimum_budget_tokens,
            *maximum_budget_tokens,
            *supports_zero_to_disable,
            *supports_automatic,
            summaries,
            patches,
        ),
        ReasoningWireDialect::OllamaBoolean => map_ollama_boolean(settings, patches),
        ReasoningWireDialect::OllamaLevel {
            efforts,
            supports_disabled,
        } => map_ollama_level(settings, efforts, *supports_disabled, patches),
    }
    .and_then(|()| {
        if family == ApiFamily::OpenAiChatCompletions
            && settings.summary != ReasoningSummaryMode::ProviderDefault
        {
            Err(unsupported_reasoning(
                "OpenAI chat completions has no common reasoning-summary field",
            ))
        } else {
            Ok(())
        }
    })
}

fn validate_reasoning_shape(settings: &ReasoningSettings) -> Result<(), ParameterEngineError> {
    if settings.mode == ReasoningMode::Disabled
        && (settings.effort.is_some()
            || settings.budget_tokens.is_some()
            || !matches!(
                settings.summary,
                ReasoningSummaryMode::ProviderDefault | ReasoningSummaryMode::Disabled
            ))
    {
        return Err(unsupported_reasoning(
            "disabled reasoning may not include effort, budget, or an enabled summary",
        ));
    }
    if settings.mode == ReasoningMode::ProviderDefault
        && (settings.effort.is_some()
            || settings.budget_tokens.is_some()
            || settings.summary != ReasoningSummaryMode::ProviderDefault)
    {
        return Err(unsupported_reasoning(
            "provider-default reasoning may not include effort, budget, or summary overrides",
        ));
    }
    Ok(())
}

fn require_effort(
    effort: Option<ReasoningEffort>,
    allowed: &[ReasoningEffort],
    required: bool,
) -> Result<Option<&'static str>, ParameterEngineError> {
    let Some(effort) = effort else {
        return if required {
            Err(unsupported_reasoning(
                "this exact reasoning mode requires an explicit effort",
            ))
        } else {
            Ok(None)
        };
    };
    if !allowed.contains(&effort) {
        return Err(unsupported_reasoning(
            "the selected model route does not support this reasoning effort",
        ));
    }
    Ok(Some(reasoning_effort_wire(effort)))
}

fn reasoning_effort_wire(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::ExtraHigh => "xhigh",
        ReasoningEffort::Maximum => "max",
    }
}

fn require_summary(
    summary: ReasoningSummaryMode,
    allowed: &[ReasoningSummaryMode],
) -> Result<(), ParameterEngineError> {
    if summary != ReasoningSummaryMode::ProviderDefault && !allowed.contains(&summary) {
        Err(unsupported_reasoning(
            "the selected model route does not support this reasoning summary mode",
        ))
    } else {
        Ok(())
    }
}

fn unsupported_reasoning(message: impl Into<String>) -> ParameterEngineError {
    ParameterEngineError::one(ParameterIssue::new(
        ParameterIssueCode::UnsupportedReasoning,
        None,
        None,
        message,
    ))
}

fn validate_opaque_reasoning_preservation(
    family: ApiFamily,
    settings: &ReasoningSettings,
) -> Result<(), ParameterEngineError> {
    if family == ApiFamily::GeminiGenerateContent && settings.preserve_opaque_state {
        Err(unsupported_reasoning(
            GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR,
        ))
    } else {
        Ok(())
    }
}

fn map_openai_responses_reasoning(
    settings: &ReasoningSettings,
    efforts: &[ReasoningEffort],
    summaries: &[ReasoningSummaryMode],
    supports_disabled: bool,
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    if settings.budget_tokens.is_some() {
        return Err(unsupported_reasoning(
            "OpenAI Responses reasoning does not accept a token budget",
        ));
    }
    match settings.mode {
        ReasoningMode::ProviderDefault => {
            if let Some(effort) = require_effort(settings.effort, efforts, false)? {
                insert_normalized_patch(patches, "reasoning.effort", json!(effort))?;
            }
        }
        ReasoningMode::Disabled if supports_disabled => {
            insert_normalized_patch(patches, "reasoning.effort", json!("none"))?;
        }
        ReasoningMode::Disabled => {
            return Err(unsupported_reasoning(
                "this OpenAI model cannot disable reasoning exactly",
            ));
        }
        ReasoningMode::Automatic => {
            return Err(unsupported_reasoning(
                "OpenAI Responses has no common automatic reasoning mode",
            ));
        }
        ReasoningMode::Enabled => {
            let effort = require_effort(settings.effort, efforts, true)?
                .expect("required effort was validated");
            insert_normalized_patch(patches, "reasoning.effort", json!(effort))?;
        }
    }

    require_summary(settings.summary, summaries)?;
    let summary = match settings.summary {
        ReasoningSummaryMode::ProviderDefault | ReasoningSummaryMode::Disabled => None,
        ReasoningSummaryMode::Automatic => Some("auto"),
        ReasoningSummaryMode::Concise => Some("concise"),
        ReasoningSummaryMode::Detailed => Some("detailed"),
    };
    if let Some(summary) = summary {
        insert_normalized_patch(patches, "reasoning.summary", json!(summary))?;
    }
    Ok(())
}

fn map_openai_chat_reasoning(
    settings: &ReasoningSettings,
    efforts: &[ReasoningEffort],
    supports_disabled: bool,
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    if settings.budget_tokens.is_some() {
        return Err(unsupported_reasoning(
            "OpenAI chat completions reasoning does not accept a token budget",
        ));
    }
    match settings.mode {
        ReasoningMode::ProviderDefault => {
            if let Some(effort) = require_effort(settings.effort, efforts, false)? {
                insert_normalized_patch(patches, "reasoning_effort", json!(effort))?;
            }
        }
        ReasoningMode::Disabled if supports_disabled => {
            insert_normalized_patch(patches, "reasoning_effort", json!("none"))?;
        }
        ReasoningMode::Disabled => {
            return Err(unsupported_reasoning(
                "this chat-completions model cannot disable reasoning exactly",
            ));
        }
        ReasoningMode::Automatic => {
            return Err(unsupported_reasoning(
                "chat completions has no common automatic reasoning mode",
            ));
        }
        ReasoningMode::Enabled => {
            let effort = require_effort(settings.effort, efforts, true)?
                .expect("required effort was validated");
            insert_normalized_patch(patches, "reasoning_effort", json!(effort))?;
        }
    }
    Ok(())
}

fn map_openrouter_reasoning(
    settings: &ReasoningSettings,
    style: OpenRouterReasoningWireStyle,
    supported_efforts: &OpenRouterReasoningEffortSupport,
    supports_max_tokens: Option<bool>,
    mandatory: Option<bool>,
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    validate_openrouter_reasoning_style(style, supported_efforts)?;
    if settings.budget_tokens.is_some() {
        let message = if supports_max_tokens == Some(true) {
            "OpenRouter advertises reasoning token budgets, but this model listing provides no \
             exact safe budget bounds"
        } else {
            "this OpenRouter model does not expose a bounded reasoning token budget"
        };
        return Err(unsupported_reasoning(message));
    }
    if settings.summary != ReasoningSummaryMode::ProviderDefault {
        return Err(unsupported_reasoning(
            "provider API metadata does not establish an exact OpenRouter reasoning exclusion control",
        ));
    }
    let efforts = openrouter_reasoning_efforts(supported_efforts);
    match settings.mode {
        ReasoningMode::ProviderDefault => {
            if settings.effort.is_some() {
                return Err(unsupported_reasoning(
                    "provider-default OpenRouter reasoning may not override effort",
                ));
            }
        }
        ReasoningMode::Disabled
            if openrouter_supports_disabled(style, supported_efforts, mandatory) =>
        {
            insert_openrouter_boolean_patch(style, patches, false)?;
        }
        ReasoningMode::Disabled => {
            return Err(unsupported_reasoning(
                "this OpenRouter model does not expose an exact disabled reasoning mode",
            ));
        }
        ReasoningMode::Automatic => {
            return Err(unsupported_reasoning(
                "OpenRouter model metadata does not expose an automatic reasoning mode",
            ));
        }
        ReasoningMode::Enabled => {
            if !openrouter_supports_enabled(style, supported_efforts) {
                return Err(unsupported_reasoning(
                    "this OpenRouter model exposes only a disabled reasoning effort",
                ));
            }
            if let Some(effort) = require_effort(settings.effort, &efforts, !efforts.is_empty())? {
                insert_normalized_patch(
                    patches,
                    openrouter_enabled_patch_path(style),
                    json!(effort),
                )?;
            } else {
                insert_openrouter_boolean_patch(style, patches, true)?;
            }
        }
    }
    Ok(())
}

fn map_anthropic_adaptive(
    settings: &ReasoningSettings,
    efforts: &[ReasoningEffort],
    summaries: &[ReasoningSummaryMode],
    supports_disabled: bool,
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    if settings.budget_tokens.is_some() {
        return Err(unsupported_reasoning(
            "Anthropic adaptive thinking does not accept budget_tokens",
        ));
    }
    if settings.mode == ReasoningMode::ProviderDefault
        && settings.summary != ReasoningSummaryMode::ProviderDefault
    {
        return Err(unsupported_reasoning(
            "provider-default Anthropic thinking may not override summary display",
        ));
    }
    if settings.mode == ReasoningMode::Disabled
        && (settings.effort.is_some() || settings.summary != ReasoningSummaryMode::ProviderDefault)
    {
        return Err(unsupported_reasoning(
            "disabled Anthropic thinking may not include hidden effort or summary values",
        ));
    }
    match settings.mode {
        ReasoningMode::ProviderDefault => {}
        ReasoningMode::Disabled if supports_disabled => {
            insert_normalized_patch(patches, "thinking.type", json!("disabled"))?;
            return Ok(());
        }
        ReasoningMode::Disabled => {
            return Err(unsupported_reasoning(
                "this Anthropic model does not allow thinking to be disabled",
            ));
        }
        ReasoningMode::Automatic => {
            insert_normalized_patch(patches, "thinking.type", json!("adaptive"))?;
        }
        ReasoningMode::Enabled => {
            return Err(unsupported_reasoning(
                "adaptive thinking may skip reasoning and cannot exactly mean always enabled",
            ));
        }
    }
    if let Some(effort) = require_effort(settings.effort, efforts, false)? {
        insert_normalized_patch(patches, "output_config.effort", json!(effort))?;
    }
    map_anthropic_summary(settings.summary, summaries, patches)
}

fn map_anthropic_extended(
    settings: &ReasoningSettings,
    minimum_budget_tokens: u32,
    maximum_budget_tokens: u32,
    efforts: &[ReasoningEffort],
    summaries: &[ReasoningSummaryMode],
    supports_disabled: bool,
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    if settings.mode == ReasoningMode::ProviderDefault
        && settings.summary != ReasoningSummaryMode::ProviderDefault
    {
        return Err(unsupported_reasoning(
            "provider-default Anthropic thinking may not override summary display",
        ));
    }
    if settings.mode == ReasoningMode::Disabled
        && (settings.effort.is_some() || settings.summary != ReasoningSummaryMode::ProviderDefault)
    {
        return Err(unsupported_reasoning(
            "disabled Anthropic thinking may not include hidden effort or summary values",
        ));
    }
    match settings.mode {
        ReasoningMode::ProviderDefault => {
            if settings.budget_tokens.is_some() {
                return Err(unsupported_reasoning(
                    "provider-default thinking may not include budget_tokens",
                ));
            }
        }
        ReasoningMode::Disabled if supports_disabled => {
            insert_normalized_patch(patches, "thinking.type", json!("disabled"))?;
            return Ok(());
        }
        ReasoningMode::Disabled => {
            return Err(unsupported_reasoning(
                "this Anthropic model does not allow thinking to be disabled",
            ));
        }
        ReasoningMode::Automatic => {
            return Err(unsupported_reasoning(
                "Anthropic extended thinking has no automatic mode",
            ));
        }
        ReasoningMode::Enabled => {
            let Some(budget) = settings.budget_tokens else {
                return Err(unsupported_reasoning(
                    "Anthropic extended thinking requires budget_tokens",
                ));
            };
            if budget < minimum_budget_tokens || budget > maximum_budget_tokens {
                return Err(unsupported_reasoning(
                    "Anthropic thinking budget is outside the observed model bounds",
                ));
            }
            insert_normalized_patch(patches, "thinking.type", json!("enabled"))?;
            insert_normalized_patch(patches, "thinking.budget_tokens", json!(budget))?;
        }
    }
    if let Some(effort) = require_effort(settings.effort, efforts, false)? {
        insert_normalized_patch(patches, "output_config.effort", json!(effort))?;
    }
    map_anthropic_summary(settings.summary, summaries, patches)
}

fn map_anthropic_summary(
    summary: ReasoningSummaryMode,
    summaries: &[ReasoningSummaryMode],
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    require_summary(summary, summaries)?;
    match summary {
        ReasoningSummaryMode::ProviderDefault => Ok(()),
        ReasoningSummaryMode::Disabled => {
            insert_normalized_patch(patches, "thinking.display", json!("omitted"))
        }
        ReasoningSummaryMode::Automatic => {
            insert_normalized_patch(patches, "thinking.display", json!("summarized"))
        }
        ReasoningSummaryMode::Concise | ReasoningSummaryMode::Detailed => Err(
            unsupported_reasoning("Anthropic does not expose concise/detailed summary levels"),
        ),
    }
}

fn map_gemini_level(
    settings: &ReasoningSettings,
    efforts: &[ReasoningEffort],
    supports_automatic: bool,
    summaries: &[ReasoningSummaryMode],
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    if settings.budget_tokens.is_some() {
        return Err(unsupported_reasoning(
            "this Gemini route uses thinkingLevel, not thinkingBudget",
        ));
    }
    match settings.mode {
        ReasoningMode::ProviderDefault => {
            if settings.effort.is_some() {
                return Err(unsupported_reasoning(
                    "provider-default Gemini thinkingLevel may not override effort",
                ));
            }
        }
        ReasoningMode::Disabled => {
            return Err(unsupported_reasoning(
                "Gemini thinkingLevel minimal does not guarantee disabled reasoning",
            ));
        }
        ReasoningMode::Automatic if supports_automatic => {
            let effort = require_effort(settings.effort, efforts, true)?
                .expect("required effort was validated");
            insert_normalized_patch(
                patches,
                "generationConfig.thinkingConfig.thinkingLevel",
                json!(effort),
            )?;
        }
        ReasoningMode::Automatic => {
            return Err(unsupported_reasoning(
                "this Gemini route has no observed automatic thinking mode",
            ));
        }
        ReasoningMode::Enabled => {
            return Err(unsupported_reasoning(
                "Gemini thinkingLevel controls dynamic effort, not guaranteed enablement",
            ));
        }
    }
    map_gemini_summary(settings.summary, summaries, patches)
}

#[allow(clippy::too_many_arguments)]
fn map_gemini_budget(
    settings: &ReasoningSettings,
    minimum_budget_tokens: u32,
    maximum_budget_tokens: u32,
    supports_zero_to_disable: bool,
    supports_automatic: bool,
    summaries: &[ReasoningSummaryMode],
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    if settings.effort.is_some() {
        return Err(unsupported_reasoning(
            "this Gemini route uses thinkingBudget, not thinkingLevel",
        ));
    }
    match settings.mode {
        ReasoningMode::ProviderDefault => {}
        ReasoningMode::Disabled if supports_zero_to_disable => {
            insert_normalized_patch(
                patches,
                "generationConfig.thinkingConfig.thinkingBudget",
                json!(0),
            )?;
        }
        ReasoningMode::Disabled => {
            return Err(unsupported_reasoning(
                "this Gemini model cannot disable thinking with a zero budget",
            ));
        }
        ReasoningMode::Automatic if supports_automatic => {
            if settings.budget_tokens.is_some() {
                return Err(unsupported_reasoning(
                    "automatic Gemini thinking may not include a fixed budget",
                ));
            }
            // Gemini's exact dynamic-thinking sentinel is -1. Omitting the
            // field would collapse Automatic into ProviderDefault and can
            // disable thinking on routes whose provider default is zero.
            insert_normalized_patch(
                patches,
                "generationConfig.thinkingConfig.thinkingBudget",
                json!(-1),
            )?;
        }
        ReasoningMode::Automatic => {
            return Err(unsupported_reasoning(
                "this Gemini route has no observed automatic thinking mode",
            ));
        }
        ReasoningMode::Enabled => {
            let Some(budget) = settings.budget_tokens else {
                return Err(unsupported_reasoning(
                    "this Gemini route requires a thinking budget to enable reasoning",
                ));
            };
            if budget < minimum_budget_tokens || budget > maximum_budget_tokens {
                return Err(unsupported_reasoning(
                    "Gemini thinking budget is outside the observed model bounds",
                ));
            }
            insert_normalized_patch(
                patches,
                "generationConfig.thinkingConfig.thinkingBudget",
                json!(budget),
            )?;
        }
    }
    map_gemini_summary(settings.summary, summaries, patches)
}

fn map_gemini_summary(
    summary: ReasoningSummaryMode,
    summaries: &[ReasoningSummaryMode],
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    require_summary(summary, summaries)?;
    match summary {
        ReasoningSummaryMode::ProviderDefault => Ok(()),
        ReasoningSummaryMode::Disabled => insert_normalized_patch(
            patches,
            "generationConfig.thinkingConfig.includeThoughts",
            json!(false),
        ),
        ReasoningSummaryMode::Automatic => insert_normalized_patch(
            patches,
            "generationConfig.thinkingConfig.includeThoughts",
            json!(true),
        ),
        ReasoningSummaryMode::Concise | ReasoningSummaryMode::Detailed => Err(
            unsupported_reasoning("Gemini exposes a summary toggle, not summary detail levels"),
        ),
    }
}

fn map_ollama_boolean(
    settings: &ReasoningSettings,
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    if settings.effort.is_some()
        || settings.budget_tokens.is_some()
        || settings.summary != ReasoningSummaryMode::ProviderDefault
    {
        return Err(unsupported_reasoning(
            "this Ollama model accepts only a boolean think setting",
        ));
    }
    match settings.mode {
        ReasoningMode::ProviderDefault => Ok(()),
        ReasoningMode::Disabled => insert_normalized_patch(patches, "think", json!(false)),
        ReasoningMode::Enabled => insert_normalized_patch(patches, "think", json!(true)),
        ReasoningMode::Automatic => Err(unsupported_reasoning(
            "Ollama has no common automatic think mode",
        )),
    }
}

fn map_ollama_level(
    settings: &ReasoningSettings,
    efforts: &[ReasoningEffort],
    supports_disabled: bool,
    patches: &mut BTreeMap<String, Value>,
) -> Result<(), ParameterEngineError> {
    if settings.budget_tokens.is_some() || settings.summary != ReasoningSummaryMode::ProviderDefault
    {
        return Err(unsupported_reasoning(
            "this Ollama model accepts think levels but no budget or summary control",
        ));
    }
    match settings.mode {
        ReasoningMode::ProviderDefault if settings.effort.is_none() => Ok(()),
        ReasoningMode::ProviderDefault => Err(unsupported_reasoning(
            "Ollama think effort requires enabled reasoning mode",
        )),
        ReasoningMode::Disabled if supports_disabled => {
            insert_normalized_patch(patches, "think", json!(false))
        }
        ReasoningMode::Disabled => Err(unsupported_reasoning(
            "this Ollama model does not allow thinking to be disabled",
        )),
        ReasoningMode::Automatic => Err(unsupported_reasoning(
            "Ollama has no common automatic think mode",
        )),
        ReasoningMode::Enabled => {
            let effort = require_effort(settings.effort, efforts, true)?
                .expect("required effort was validated");
            insert_normalized_patch(patches, "think", json!(effort))
        }
    }
}

#[allow(clippy::too_many_lines)]
fn map_prompt_cache(
    family: ApiFamily,
    settings: &PromptCacheSettings,
    dialect: PromptCacheWireDialect,
    patches: &mut BTreeMap<String, Value>,
) -> Result<PromptCacheDirective, ParameterEngineError> {
    validate_prompt_cache_shape(settings)?;
    if settings.mode == PromptCacheMode::ProviderDefault {
        return Ok(PromptCacheDirective::None);
    }

    match dialect {
        PromptCacheWireDialect::Unsupported => Err(unsupported_cache(
            "the selected model route has no provider prompt-cache control",
        )),
        PromptCacheWireDialect::OpenAiAutomatic {
            supports_24_hour_retention,
        } => {
            if !matches!(
                family,
                ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions
            ) {
                return Err(unsupported_cache(
                    "OpenAI cache dialect does not match the API family",
                ));
            }
            if settings.mode != PromptCacheMode::Automatic {
                return Err(unsupported_cache(
                    "OpenAI prompt caching is automatic and has no explicit context or disable control",
                ));
            }
            match settings.ttl {
                PromptCacheTtl::ProviderDefault | PromptCacheTtl::Short => {}
                PromptCacheTtl::Long if supports_24_hour_retention => {
                    insert_normalized_patch(patches, "prompt_cache_retention", json!("24h"))?;
                }
                PromptCacheTtl::Long => {
                    return Err(unsupported_cache(
                        "this OpenAI route has no observed 24-hour cache retention",
                    ));
                }
                PromptCacheTtl::CustomSeconds(_) => {
                    return Err(unsupported_cache(
                        "OpenAI prompt cache does not accept a custom TTL",
                    ));
                }
            }
            Ok(PromptCacheDirective::ProviderManagedAutomatic)
        }
        PromptCacheWireDialect::Anthropic {
            supports_automatic,
            supports_explicit_breakpoints,
            supports_one_hour_ttl,
        } => {
            if family != ApiFamily::AnthropicMessages {
                return Err(unsupported_cache(
                    "Anthropic cache dialect does not match the API family",
                ));
            }
            match settings.mode {
                PromptCacheMode::Automatic if supports_automatic => {
                    Ok(PromptCacheDirective::AnthropicAutomatic {
                        ttl: anthropic_cache_ttl(settings.ttl, supports_one_hour_ttl)?,
                    })
                }
                PromptCacheMode::Automatic => Err(unsupported_cache(
                    "this Anthropic route has no observed automatic cache mode",
                )),
                PromptCacheMode::ExplicitBreakpoints if supports_explicit_breakpoints => {
                    Ok(PromptCacheDirective::AnthropicExplicitBreakpoints {
                        ttl: anthropic_cache_ttl(settings.ttl, supports_one_hour_ttl)?,
                    })
                }
                PromptCacheMode::ExplicitBreakpoints => Err(unsupported_cache(
                    "this Anthropic route has no observed explicit cache breakpoints",
                )),
                PromptCacheMode::DisabledIfSupported
                    if settings.ttl == PromptCacheTtl::ProviderDefault =>
                {
                    Ok(PromptCacheDirective::AnthropicDisabled)
                }
                PromptCacheMode::DisabledIfSupported => Err(unsupported_cache(
                    "disabled Anthropic caching may not include a hidden TTL value",
                )),
                PromptCacheMode::ExplicitContext => Err(unsupported_cache(
                    "Anthropic Messages uses cache controls, not a cache resource reference",
                )),
                PromptCacheMode::ProviderDefault => unreachable!("handled above"),
            }
        }
        PromptCacheWireDialect::Gemini {
            supports_implicit,
            supports_explicit_context,
        } => {
            if family != ApiFamily::GeminiGenerateContent {
                return Err(unsupported_cache(
                    "Gemini cache dialect does not match the API family",
                ));
            }
            match settings.mode {
                PromptCacheMode::Automatic if supports_implicit => {
                    if settings.ttl != PromptCacheTtl::ProviderDefault {
                        return Err(unsupported_cache(
                            "Gemini implicit caching does not expose a per-request TTL",
                        ));
                    }
                    Ok(PromptCacheDirective::ProviderManagedAutomatic)
                }
                PromptCacheMode::Automatic => Err(unsupported_cache(
                    "this Gemini route has no observed implicit caching",
                )),
                PromptCacheMode::ExplicitContext if supports_explicit_context => {
                    if settings.ttl != PromptCacheTtl::ProviderDefault {
                        return Err(unsupported_cache(
                            "Gemini cached-content TTL is fixed when the cache resource is created and cannot be changed by generateContent",
                        ));
                    }
                    let reference = settings.context_reference.as_deref().ok_or_else(|| {
                        invalid_cache_reference(
                            "Gemini explicit context requires a cachedContents resource name",
                        )
                    })?;
                    validate_gemini_cache_reference(reference)?;
                    Ok(PromptCacheDirective::GeminiExplicitContext {
                        resource_name: reference.to_owned(),
                        requested_ttl: PromptCacheTtl::ProviderDefault,
                    })
                }
                PromptCacheMode::ExplicitContext => Err(unsupported_cache(
                    "this Gemini route has no observed explicit context caching",
                )),
                PromptCacheMode::ExplicitBreakpoints => Err(unsupported_cache(
                    "Gemini context caching does not use inline breakpoints",
                )),
                PromptCacheMode::DisabledIfSupported => Err(unsupported_cache(
                    "Gemini implicit caching cannot be disabled per request",
                )),
                PromptCacheMode::ProviderDefault => unreachable!("handled above"),
            }
        }
    }
}

fn validate_prompt_cache_shape(settings: &PromptCacheSettings) -> Result<(), ParameterEngineError> {
    if settings.mode != PromptCacheMode::ExplicitContext && settings.context_reference.is_some() {
        return Err(invalid_cache_reference(
            "a cache resource reference is valid only for explicit context mode",
        ));
    }
    if let PromptCacheTtl::CustomSeconds(seconds) = settings.ttl
        && (seconds == 0 || seconds > MAX_CUSTOM_CACHE_TTL_SECONDS)
    {
        return Err(unsupported_cache(format!(
            "custom prompt-cache TTL must be between 1 and {MAX_CUSTOM_CACHE_TTL_SECONDS} seconds"
        )));
    }
    if settings.mode == PromptCacheMode::ProviderDefault
        && settings.ttl != PromptCacheTtl::ProviderDefault
    {
        return Err(unsupported_cache(
            "provider-default cache mode may not override the TTL",
        ));
    }
    Ok(())
}

fn anthropic_cache_ttl(
    ttl: PromptCacheTtl,
    supports_one_hour_ttl: bool,
) -> Result<AnthropicCacheTtl, ParameterEngineError> {
    match ttl {
        PromptCacheTtl::ProviderDefault | PromptCacheTtl::Short => {
            Ok(AnthropicCacheTtl::FiveMinutes)
        }
        PromptCacheTtl::Long if supports_one_hour_ttl => Ok(AnthropicCacheTtl::OneHour),
        PromptCacheTtl::Long => Err(unsupported_cache(
            "this Anthropic route has no observed one-hour prompt cache",
        )),
        PromptCacheTtl::CustomSeconds(_) => Err(unsupported_cache(
            "Anthropic prompt cache supports only five-minute or one-hour TTLs",
        )),
    }
}

fn validate_gemini_cache_reference(reference: &str) -> Result<(), ParameterEngineError> {
    let suffix = reference.strip_prefix("cachedContents/").ok_or_else(|| {
        invalid_cache_reference("Gemini cache name must start with cachedContents/")
    })?;
    if suffix.is_empty()
        || suffix.len() > 128
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid_cache_reference(
            "Gemini cachedContents resource id is malformed",
        ));
    }
    Ok(())
}

fn unsupported_cache(message: impl Into<String>) -> ParameterEngineError {
    ParameterEngineError::one(ParameterIssue::new(
        ParameterIssueCode::UnsupportedPromptCache,
        None,
        None,
        message,
    ))
}

fn invalid_cache_reference(message: impl Into<String>) -> ParameterEngineError {
    ParameterEngineError::one(ParameterIssue::new(
        ParameterIssueCode::InvalidPromptCacheReference,
        None,
        None,
        message,
    ))
}

fn insert_json_path(
    body: &mut Value,
    path: &str,
    value: Value,
) -> Result<(), ParameterEngineError> {
    let segments = path.split('.').collect::<Vec<_>>();
    let mut current = body;
    for (index, segment) in segments.iter().enumerate() {
        let is_leaf = index + 1 == segments.len();
        let object = current.as_object_mut().ok_or_else(|| {
            ParameterEngineError::one(ParameterIssue::new(
                ParameterIssueCode::ConflictingRequestField,
                None,
                None,
                format!("request path {path} crosses a non-object field"),
            ))
        })?;
        if is_leaf {
            if object.contains_key(*segment) {
                return Err(ParameterEngineError::one(ParameterIssue::new(
                    ParameterIssueCode::ConflictingRequestField,
                    None,
                    None,
                    format!("request path {path} would overwrite an adapter-owned field"),
                )));
            }
            object.insert((*segment).to_owned(), value);
            return Ok(());
        }
        current = object
            .entry((*segment).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{
        ParameterChoice, ParameterCondition, ParameterConflict, ParameterConflictKind,
        ParameterType,
    };
    use serde_json::json;

    use super::*;

    fn typed_spec(
        id: &str,
        schema: ParameterSchema,
        level: UiParameterLevel,
        field: &str,
    ) -> TypedParameterSpec {
        TypedParameterSpec {
            id: ParameterId::from(id),
            label_key: format!("parameter.{id}"),
            description_key: None,
            schema,
            default_mode: ParameterDefaultMode::ProviderDefault,
            visibility: None,
            rules: Vec::new(),
            provider_mapping: ProviderParameterMapping {
                target: ProviderParameterTarget::RequestBody,
                field_name: field.to_owned(),
            },
            level,
        }
    }

    fn number_spec(id: &str, field: &str, level: UiParameterLevel) -> TypedParameterSpec {
        typed_spec(
            id,
            ParameterSchema::Number {
                minimum: Some(0.0),
                maximum: Some(2.0),
                step: Some(0.1),
                choices: Vec::new(),
            },
            level,
            field,
        )
    }

    fn integer_spec(id: &str, field: &str, level: UiParameterLevel) -> TypedParameterSpec {
        typed_spec(
            id,
            ParameterSchema::Integer {
                minimum: Some(1),
                maximum: Some(8192),
                step: Some(1),
                choices: Vec::new(),
            },
            level,
            field,
        )
    }

    fn value(id: &str, literal: ParameterLiteral) -> ParameterValue {
        ParameterValue {
            parameter_id: ParameterId::from(id),
            state: ParameterValueState::Explicit(literal),
        }
    }

    fn inherit(id: &str) -> ParameterValue {
        ParameterValue {
            parameter_id: ParameterId::from(id),
            state: ParameterValueState::InheritProviderDefault,
        }
    }

    fn validated(applied: Vec<AppliedParameter>) -> ValidatedParameterSet {
        ValidatedParameterSet {
            applied,
            omitted_provider_defaults: Vec::new(),
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn exact_dialect_metadata_rejects_unknown_fields_wrong_families_and_invalid_bounds() {
        let valid = json!({
            "dialect": "open_ai_chat_completions",
            "efforts": ["low", "high"],
            "supports_disabled": true,
        });
        assert_eq!(
            parse_reasoning_wire_dialect_metadata(ApiFamily::OpenAiChatCompletions, &valid,)
                .expect("valid exact dialect"),
            ReasoningWireDialect::OpenAiChatCompletions {
                efforts: vec![ReasoningEffort::Low, ReasoningEffort::High],
                supports_disabled: true,
            }
        );
        let mut unknown = valid.clone();
        unknown
            .as_object_mut()
            .expect("dialect object")
            .insert("api_key".to_owned(), json!("not-accepted"));
        assert!(
            parse_reasoning_wire_dialect_metadata(ApiFamily::OpenAiChatCompletions, &unknown,)
                .is_err()
        );
        assert!(
            parse_reasoning_wire_dialect_metadata(ApiFamily::AnthropicMessages, &valid).is_err()
        );
        for invalid in [
            json!({
                "dialect": "anthropic_adaptive",
                "efforts": ["minimal"],
                "summaries": [],
                "supports_disabled": false,
            }),
            json!({
                "dialect": "anthropic_adaptive",
                "efforts": ["low"],
                "summaries": ["concise"],
                "supports_disabled": false,
            }),
        ] {
            assert!(
                parse_reasoning_wire_dialect_metadata(ApiFamily::AnthropicMessages, &invalid)
                    .is_err()
            );
        }
        assert!(
            parse_reasoning_wire_dialect_metadata(
                ApiFamily::GeminiGenerateContent,
                &json!({
                    "dialect": "gemini_thinking_budget",
                    "minimum_budget_tokens": 4096,
                    "maximum_budget_tokens": 1024,
                    "supports_zero_to_disable": false,
                    "supports_automatic": false,
                    "summaries": [],
                }),
            )
            .is_err()
        );
        assert!(
            parse_reasoning_wire_dialect_metadata(
                ApiFamily::OpenAiChatCompletions,
                &json!({
                    "dialect": "open_ai_chat_completions",
                    "efforts": ["high", "high"],
                    "supports_disabled": false,
                }),
            )
            .is_err()
        );
        assert!(
            parse_reasoning_wire_dialect_metadata(
                ApiFamily::OllamaNative,
                &json!({
                    "dialect": "ollama_level",
                    "efforts": [],
                    "supports_disabled": false,
                }),
            )
            .is_err()
        );
        assert_eq!(
            parse_reasoning_wire_dialect_metadata(
                ApiFamily::OllamaNative,
                &json!({
                    "dialect": "ollama_level",
                    "efforts": ["low", "medium", "high", "maximum"],
                    "supports_disabled": true,
                }),
            )
            .expect("official Ollama native levels"),
            ReasoningWireDialect::OllamaLevel {
                efforts: vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::Maximum,
                ],
                supports_disabled: true,
            }
        );
        for unsupported in ["minimal", "extra_high"] {
            assert!(
                parse_reasoning_wire_dialect_metadata(
                    ApiFamily::OllamaNative,
                    &json!({
                        "dialect": "ollama_level",
                        "efforts": [unsupported],
                        "supports_disabled": false,
                    }),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn empty_ollama_level_contract_is_rejected_before_ui_and_request() {
        let dialect = ReasoningWireDialect::OllamaLevel {
            efforts: Vec::new(),
            supports_disabled: false,
        };

        let control =
            render_reasoning_control(ApiFamily::OllamaNative, &dialect, &default_reasoning());
        assert_eq!(control.state, UiControlState::Invalid);
        assert_eq!(
            control.issues[0].code,
            ParameterIssueCode::UnsupportedReasoning
        );

        let error = build_provider_request_plan(
            ApiFamily::OllamaNative,
            &validated(Vec::new()),
            &default_reasoning(),
            &dialect,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect_err("empty Ollama effort metadata must not reach request construction");
        assert_eq!(
            error.issues[0].code,
            ParameterIssueCode::UnsupportedReasoning
        );
    }

    #[test]
    fn exact_prompt_cache_metadata_is_closed_and_family_bound() {
        let value = json!({
            "dialect": "anthropic",
            "supports_automatic": true,
            "supports_explicit_breakpoints": true,
            "supports_one_hour_ttl": false,
        });
        assert_eq!(
            parse_prompt_cache_wire_dialect_metadata(ApiFamily::AnthropicMessages, &value)
                .expect("Anthropic cache dialect"),
            PromptCacheWireDialect::Anthropic {
                supports_automatic: true,
                supports_explicit_breakpoints: true,
                supports_one_hour_ttl: false,
            }
        );
        assert!(
            parse_prompt_cache_wire_dialect_metadata(ApiFamily::OpenAiResponses, &value).is_err()
        );
        let mut unknown = value;
        unknown
            .as_object_mut()
            .expect("cache dialect object")
            .insert("credential".to_owned(), json!("not-accepted"));
        assert!(
            parse_prompt_cache_wire_dialect_metadata(ApiFamily::AnthropicMessages, &unknown)
                .is_err()
        );
    }

    fn applied(id: &str, field: &str, value: ParameterLiteral) -> AppliedParameter {
        AppliedParameter {
            parameter_id: ParameterId::from(id),
            mapping: ProviderParameterMapping {
                target: ProviderParameterTarget::RequestBody,
                field_name: field.to_owned(),
            },
            value,
        }
    }

    fn default_reasoning() -> ReasoningSettings {
        ReasoningSettings {
            preserve_opaque_state: false,
            ..ReasoningSettings::default()
        }
    }

    fn default_cache() -> PromptCacheSettings {
        PromptCacheSettings::default()
    }

    fn openrouter_dialect(
        style: OpenRouterReasoningWireStyle,
        supported_efforts: OpenRouterReasoningEffortSupport,
        default_effort: Option<OpenRouterReasoningEffort>,
        supports_max_tokens: Option<bool>,
        mandatory: Option<bool>,
    ) -> ReasoningWireDialect {
        ReasoningWireDialect::OpenRouter {
            style,
            supported_efforts,
            default_effort,
            default_enabled: None,
            supports_max_tokens,
            mandatory,
        }
    }

    #[test]
    fn provider_reasoning_defaults_do_not_opt_into_opaque_state() {
        assert!(!ReasoningSettings::default().preserve_opaque_state);
    }

    #[test]
    fn safe_schema_converts_legacy_flat_specs_and_rejects_impossible_shapes() {
        let valid = ParameterSpec {
            id: ParameterId::from("temperature"),
            label_key: "temperature".to_owned(),
            description_key: None,
            value_type: ParameterType::Number,
            allowed_values: vec![ParameterChoice {
                value: ParameterLiteral::Number(0.5),
                label_key: "balanced".to_owned(),
            }],
            minimum: Some(0.0),
            maximum: Some(2.0),
            step: Some(0.1),
            default_mode: ParameterDefaultMode::ProviderDefault,
            visibility: None,
            conflicts: Vec::new(),
            provider_mapping: ProviderParameterMapping {
                target: ProviderParameterTarget::RequestBody,
                field_name: "temperature".to_owned(),
            },
            level: UiParameterLevel::Basic,
        };
        let typed = TypedParameterSpec::try_from(&valid).expect("compile legacy spec");
        assert!(matches!(
            typed.schema,
            ParameterSchema::Number {
                minimum: Some(0.0),
                maximum: Some(2.0),
                ..
            }
        ));
        ParameterEngine::from_manifest_specs_for_family(
            ApiFamily::OpenAiResponses,
            std::slice::from_ref(&valid),
        )
        .expect("family-supported manifest mapping");

        let mut unsupported_mapping = valid.clone();
        unsupported_mapping.provider_mapping.field_name = "invented_field".to_owned();
        let error = ParameterEngine::from_manifest_specs_for_family(
            ApiFamily::OpenAiResponses,
            &[unsupported_mapping],
        )
        .expect_err("family constructor rejects unsupported mappings");
        assert_eq!(error.issues[0].code, ParameterIssueCode::UnsupportedMapping);

        let mut invalid = valid;
        invalid.value_type = ParameterType::Boolean;
        assert!(TypedParameterSpec::try_from(&invalid).is_err());
    }

    #[test]
    fn untouched_and_inherited_parameters_are_omitted() {
        let engine = ParameterEngine::new(vec![
            number_spec("temperature", "temperature", UiParameterLevel::Basic),
            integer_spec(
                "max_output_tokens",
                "max_output_tokens",
                UiParameterLevel::Basic,
            ),
        ])
        .expect("valid engine");

        for values in [Vec::new(), vec![inherit("temperature")]] {
            let validated = engine
                .validate_for_request(&values)
                .expect("provider defaults are valid");
            assert!(validated.applied().is_empty());
            assert_eq!(validated.omitted_provider_defaults().len(), 2);
        }
    }

    #[test]
    fn legacy_defaults_are_explicit_but_new_presets_are_empty() {
        let legacy = legacy_generation_preset_values();
        assert_eq!(legacy.len(), 2);
        assert_eq!(
            legacy[0].state,
            ParameterValueState::Explicit(ParameterLiteral::Number(1.0))
        );
        assert_eq!(
            legacy[1].state,
            ParameterValueState::Explicit(ParameterLiteral::Integer(4096))
        );
        assert!(new_generation_preset_values().is_empty());
    }

    #[test]
    fn persisted_domain_settings_convert_without_loss() {
        let reasoning = ReasoningSettings::from(&GenerationReasoningSettings {
            mode: GenerationReasoningMode::Enabled,
            effort: Some(GenerationReasoningEffort::ExtraHigh),
            budget_tokens: Some(2048),
            summary: GenerationReasoningSummary::Concise,
            preserve_opaque_state: false,
        });
        assert_eq!(reasoning.mode, ReasoningMode::Enabled);
        assert_eq!(reasoning.effort, Some(ReasoningEffort::ExtraHigh));
        assert_eq!(reasoning.budget_tokens, Some(2048));
        assert_eq!(reasoning.summary, ReasoningSummaryMode::Concise);
        assert!(!reasoning.preserve_opaque_state);

        let cache = PromptCacheSettings::from(&GenerationPromptCacheSettings {
            mode: GenerationPromptCacheMode::ExplicitContext,
            ttl: GenerationPromptCacheTtl::CustomSeconds(3600),
            context_reference: Some("cachedContents/cache_1".to_owned()),
        });
        assert_eq!(cache.mode, PromptCacheMode::ExplicitContext);
        assert_eq!(cache.ttl, PromptCacheTtl::CustomSeconds(3600));
        assert_eq!(
            cache.context_reference.as_deref(),
            Some("cachedContents/cache_1")
        );
    }

    #[test]
    fn numeric_bounds_and_steps_hold_across_a_value_grid() {
        let engine = ParameterEngine::new(vec![
            number_spec("temperature", "temperature", UiParameterLevel::Basic),
            TypedParameterSpec {
                id: ParameterId::from("count"),
                label_key: "count".to_owned(),
                description_key: None,
                schema: ParameterSchema::Integer {
                    minimum: Some(-4),
                    maximum: Some(4),
                    step: Some(2),
                    choices: Vec::new(),
                },
                default_mode: ParameterDefaultMode::ProviderDefault,
                visibility: None,
                rules: Vec::new(),
                provider_mapping: ProviderParameterMapping {
                    target: ProviderParameterTarget::RequestBody,
                    field_name: "seed".to_owned(),
                },
                level: UiParameterLevel::Advanced,
            },
        ])
        .expect("valid engine");

        for tenth in -10..=30 {
            let candidate = f64::from(tenth) / 10.0;
            let result = engine
                .validate_for_request(&[value("temperature", ParameterLiteral::Number(candidate))]);
            assert_eq!(
                result.is_ok(),
                (0..=20).contains(&tenth),
                "temperature grid value {candidate}"
            );
        }

        for candidate in -8..=8 {
            let result = engine
                .validate_for_request(&[value("count", ParameterLiteral::Integer(candidate))]);
            assert_eq!(
                result.is_ok(),
                (-4..=4).contains(&candidate) && (candidate + 4) % 2 == 0,
                "integer grid value {candidate}"
            );
        }
    }

    #[test]
    fn floating_step_validation_remains_strict_at_large_magnitudes() {
        let engine = ParameterEngine::new(vec![typed_spec(
            "large_grid",
            ParameterSchema::Number {
                minimum: Some(0.0),
                maximum: Some(2_000_000_000_000.0),
                step: Some(0.25),
                choices: Vec::new(),
            },
            UiParameterLevel::Advanced,
            "temperature",
        )])
        .expect("valid large numeric grid");

        engine
            .validate_for_request(&[value(
                "large_grid",
                ParameterLiteral::Number(1_000_000_000_000.25),
            )])
            .expect("on-grid value");
        let error = engine
            .validate_for_request(&[value(
                "large_grid",
                ParameterLiteral::Number(1_000_000_000_000.125),
            )])
            .expect_err("half-step value must not be absorbed by tolerance");
        assert_eq!(error.issues[0].code, ParameterIssueCode::InvalidStep);
    }

    #[test]
    fn duplicate_choices_are_invalid_parameter_definitions() {
        let cases = [
            ParameterSchema::Integer {
                minimum: None,
                maximum: None,
                step: None,
                choices: vec![
                    IntegerChoice {
                        value: 1,
                        label_key: "one".to_owned(),
                    },
                    IntegerChoice {
                        value: 1,
                        label_key: "one_again".to_owned(),
                    },
                ],
            },
            ParameterSchema::Number {
                minimum: None,
                maximum: None,
                step: None,
                choices: vec![
                    NumberChoice {
                        value: -0.0,
                        label_key: "zero".to_owned(),
                    },
                    NumberChoice {
                        value: 0.0,
                        label_key: "zero_again".to_owned(),
                    },
                ],
            },
            ParameterSchema::String {
                min_bytes: 0,
                max_bytes: 32,
                choices: vec![
                    StringChoice {
                        value: "same".to_owned(),
                        label_key: "same".to_owned(),
                    },
                    StringChoice {
                        value: "same".to_owned(),
                        label_key: "same_again".to_owned(),
                    },
                ],
            },
        ];

        for schema in cases {
            let error = ParameterEngine::new(vec![typed_spec(
                "choice",
                schema,
                UiParameterLevel::Basic,
                "service_tier",
            )])
            .expect_err("duplicate choices must fail compilation");
            assert!(
                error
                    .issues
                    .iter()
                    .any(|issue| issue.code == ParameterIssueCode::InvalidDefinition)
            );
        }
    }

    #[test]
    fn enum_lists_and_json_schema_are_validated_by_their_own_variants() {
        let engine = ParameterEngine::new(vec![
            typed_spec(
                "tier",
                ParameterSchema::Enum {
                    choices: vec![
                        StringChoice {
                            value: "auto".to_owned(),
                            label_key: "auto".to_owned(),
                        },
                        StringChoice {
                            value: "flex".to_owned(),
                            label_key: "flex".to_owned(),
                        },
                    ],
                },
                UiParameterLevel::Advanced,
                "service_tier",
            ),
            typed_spec(
                "stops",
                ParameterSchema::StopSequenceList {
                    max_items: 2,
                    max_item_bytes: 8,
                },
                UiParameterLevel::Advanced,
                "stop",
            ),
            typed_spec(
                "schema",
                ParameterSchema::JsonSchema { max_bytes: 128 },
                UiParameterLevel::Advanced,
                "generationConfig.responseJsonSchema",
            ),
        ])
        .expect("valid engine");

        let valid = [
            value("tier", ParameterLiteral::Enum("flex".to_owned())),
            value(
                "stops",
                ParameterLiteral::StopSequenceList(vec!["END".to_owned()]),
            ),
            value(
                "schema",
                ParameterLiteral::JsonSchema(r#"{"type":"object"}"#.to_owned()),
            ),
        ];
        assert!(engine.validate_for_request(&valid).is_ok());

        let invalid = [
            value("tier", ParameterLiteral::Enum("unknown".to_owned())),
            value(
                "stops",
                ParameterLiteral::StopSequenceList(vec![String::new(), String::new()]),
            ),
            value("schema", ParameterLiteral::JsonSchema("[]".to_owned())),
        ];
        let error = engine
            .validate_for_request(&invalid)
            .expect_err("all literals are invalid");
        assert_eq!(error.issues.len(), 3);
    }

    #[test]
    fn conditions_and_rules_create_render_ready_groups() {
        let mode = typed_spec(
            "mode",
            ParameterSchema::Enum {
                choices: vec![
                    StringChoice {
                        value: "text".to_owned(),
                        label_key: "text".to_owned(),
                    },
                    StringChoice {
                        value: "json".to_owned(),
                        label_key: "json".to_owned(),
                    },
                ],
            },
            UiParameterLevel::Basic,
            "service_tier",
        );
        let mut schema = typed_spec(
            "schema",
            ParameterSchema::JsonSchema { max_bytes: 128 },
            UiParameterLevel::Advanced,
            "generationConfig.responseJsonSchema",
        );
        schema.visibility = Some(ParameterConditionExpr::Equals {
            parameter_id: ParameterId::from("mode"),
            value: ParameterLiteral::Enum("json".to_owned()),
        });
        let mut first = typed_spec(
            "first",
            ParameterSchema::Boolean,
            UiParameterLevel::Expert,
            "logprobs",
        );
        first.rules = vec![
            ParameterRule::MutuallyExclusive {
                parameter_id: ParameterId::from("second"),
                message_key: "choose_one".to_owned(),
            },
            ParameterRule::Requires {
                parameter_id: ParameterId::from("mode"),
                message_key: "mode_required".to_owned(),
            },
        ];
        let second = typed_spec(
            "second",
            ParameterSchema::Boolean,
            UiParameterLevel::Expert,
            "parallel_tool_calls",
        );
        let engine = ParameterEngine::new(vec![mode, schema, first, second]).expect("valid engine");

        let text = engine.evaluate(&[value("mode", ParameterLiteral::Enum("text".to_owned()))]);
        assert!(text.is_valid());
        assert_eq!(text.editor.basic.len(), 1);
        assert!(text.editor.advanced.is_empty());
        assert_eq!(text.editor.expert.len(), 2);

        let hidden_value = engine.evaluate(&[
            value("mode", ParameterLiteral::Enum("text".to_owned())),
            value(
                "schema",
                ParameterLiteral::JsonSchema(r#"{"type":"object"}"#.to_owned()),
            ),
        ]);
        assert!(
            hidden_value
                .issues
                .iter()
                .any(|issue| issue.code == ParameterIssueCode::HiddenValue)
        );

        let conflict = engine.evaluate(&[
            value("first", ParameterLiteral::Boolean(true)),
            value("second", ParameterLiteral::Boolean(true)),
        ]);
        assert!(
            conflict
                .issues
                .iter()
                .any(|issue| issue.code == ParameterIssueCode::MutuallyExclusive)
        );
        assert!(
            conflict
                .issues
                .iter()
                .any(|issue| issue.code == ParameterIssueCode::MissingRequirement)
        );
    }

    #[test]
    fn duplicate_unknown_and_required_values_fail_before_mapping() {
        let mut spec = number_spec("temperature", "temperature", UiParameterLevel::Basic);
        spec.default_mode = ParameterDefaultMode::ExplicitRequired;
        let engine = ParameterEngine::new(vec![spec]).expect("valid engine");

        assert_eq!(
            engine
                .validate_for_request(&[])
                .expect_err("required")
                .issues[0]
                .code,
            ParameterIssueCode::RequiredValueMissing
        );
        let error = engine
            .validate_for_request(&[
                value("temperature", ParameterLiteral::Number(0.4)),
                value("TEMPERATURE", ParameterLiteral::Number(0.5)),
                value("unknown", ParameterLiteral::Boolean(true)),
            ])
            .expect_err("duplicate and unknown");
        assert!(
            error
                .issues
                .iter()
                .any(|issue| issue.code == ParameterIssueCode::DuplicateValue)
        );
        assert!(
            error
                .issues
                .iter()
                .any(|issue| issue.code == ParameterIssueCode::UnknownParameter)
        );
    }

    #[test]
    fn atomic_request_gate_rejects_every_invalid_preset_class() {
        let mut first = typed_spec(
            "first",
            ParameterSchema::Boolean,
            UiParameterLevel::Advanced,
            "logprobs",
        );
        first.rules.push(ParameterRule::MutuallyExclusive {
            parameter_id: ParameterId::from("second"),
            message_key: "choose_one".to_owned(),
        });
        let engine = ParameterEngine::new(vec![
            number_spec("temperature", "temperature", UiParameterLevel::Basic),
            typed_spec(
                "tier",
                ParameterSchema::Enum {
                    choices: vec![StringChoice {
                        value: "auto".to_owned(),
                        label_key: "auto".to_owned(),
                    }],
                },
                UiParameterLevel::Advanced,
                "service_tier",
            ),
            first,
            typed_spec(
                "second",
                ParameterSchema::Boolean,
                UiParameterLevel::Advanced,
                "parallel_tool_calls",
            ),
        ])
        .expect("valid family-bound schema");

        let cases = [
            (
                vec![value("unknown", ParameterLiteral::Boolean(true))],
                ParameterIssueCode::UnknownParameter,
            ),
            (
                vec![
                    value("temperature", ParameterLiteral::Number(0.5)),
                    value("TEMPERATURE", ParameterLiteral::Number(0.6)),
                ],
                ParameterIssueCode::DuplicateValue,
            ),
            (
                vec![value("temperature", ParameterLiteral::Integer(1))],
                ParameterIssueCode::TypeMismatch,
            ),
            (
                vec![value("temperature", ParameterLiteral::Number(3.0))],
                ParameterIssueCode::OutOfBounds,
            ),
            (
                vec![value("temperature", ParameterLiteral::Number(0.15))],
                ParameterIssueCode::InvalidStep,
            ),
            (
                vec![value("tier", ParameterLiteral::Enum("flex".to_owned()))],
                ParameterIssueCode::InvalidChoice,
            ),
            (
                vec![
                    value("first", ParameterLiteral::Boolean(true)),
                    value("second", ParameterLiteral::Boolean(true)),
                ],
                ParameterIssueCode::MutuallyExclusive,
            ),
        ];

        for (values, expected) in cases {
            let error = validate_and_build_provider_request_plan(
                &engine,
                ApiFamily::OpenAiChatCompletions,
                &values,
                &default_reasoning(),
                &ReasoningWireDialect::Unsupported,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect_err("invalid preset must not produce a request plan");
            assert!(
                error.issues.iter().any(|issue| issue.code == expected),
                "missing expected issue {expected:?}: {:?}",
                error.issues
            );
        }
    }

    #[test]
    fn family_validation_rejects_unsupported_mappings_even_when_untouched() {
        let unsupported = ParameterEngine::new(vec![typed_spec(
            "unsafe",
            ParameterSchema::String {
                min_bytes: 0,
                max_bytes: 32,
                choices: Vec::new(),
            },
            UiParameterLevel::Expert,
            "arbitrary_field",
        )])
        .expect("structurally valid schema");
        let error = validate_and_build_provider_request_plan(
            &unsupported,
            ApiFamily::OpenAiResponses,
            &[],
            &default_reasoning(),
            &ReasoningWireDialect::Unsupported,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect_err("untouched unsupported mapping must fail the route contract");
        assert_eq!(error.issues[0].code, ParameterIssueCode::UnsupportedMapping);

        let type_mismatch = ParameterEngine::new(vec![integer_spec(
            "temperature",
            "temperature",
            UiParameterLevel::Basic,
        )])
        .expect("structurally valid schema");
        let error = type_mismatch
            .validate_mappings_for_family(ApiFamily::OpenAiResponses)
            .expect_err("integer cannot map losslessly to a number field");
        assert_eq!(error.issues[0].code, ParameterIssueCode::UnsupportedMapping);

        let mut header = typed_spec(
            "header",
            ParameterSchema::String {
                min_bytes: 0,
                max_bytes: 32,
                choices: Vec::new(),
            },
            UiParameterLevel::Expert,
            "x-custom",
        );
        header.provider_mapping.target = ProviderParameterTarget::RequestHeader;
        let error = ParameterEngine::new(vec![header])
            .expect("header syntax is structurally valid")
            .validate_mappings_for_family(ApiFamily::OpenAiResponses)
            .expect_err("request-header parameter mappings are closed");
        assert_eq!(error.issues[0].code, ParameterIssueCode::UnsupportedMapping);

        let ollama_logprobs = ParameterEngine::new(vec![typed_spec(
            "logprobs",
            ParameterSchema::Boolean,
            UiParameterLevel::Expert,
            "logprobs",
        )])
        .expect("structurally valid schema");
        let error = ollama_logprobs
            .validate_mappings_for_family(ApiFamily::OllamaNative)
            .expect_err("Ollama logprobs are not exposed until responses preserve them");
        assert_eq!(error.issues[0].code, ParameterIssueCode::UnsupportedMapping);

        let ollama_top_logprobs = ParameterEngine::new(vec![integer_spec(
            "top_logprobs",
            "top_logprobs",
            UiParameterLevel::Expert,
        )])
        .expect("structurally valid schema");
        let error = ollama_top_logprobs
            .validate_mappings_for_family(ApiFamily::OllamaNative)
            .expect_err("Ollama top_logprobs are not exposed until responses preserve them");
        assert_eq!(error.issues[0].code, ParameterIssueCode::UnsupportedMapping);
    }

    #[test]
    fn structured_output_schema_uses_exact_anthropic_and_ollama_wire_shapes() {
        let schema = r#"{"type":"object","properties":{"name":{"type":"string"}}}"#;
        for (family, field, expected) in [
            (
                ApiFamily::AnthropicMessages,
                "output_config.format",
                json!({
                    "type": "json_schema",
                    "schema": {
                        "type": "object",
                        "properties": {"name": {"type": "string"}}
                    }
                }),
            ),
            (
                ApiFamily::OllamaNative,
                "format",
                json!({
                    "type": "object",
                    "properties": {"name": {"type": "string"}}
                }),
            ),
        ] {
            let engine = ParameterEngine::new(vec![typed_spec(
                "schema",
                ParameterSchema::JsonSchema { max_bytes: 256 },
                UiParameterLevel::Advanced,
                field,
            )])
            .expect("structured-output schema");
            let plan = validate_and_build_provider_request_plan(
                &engine,
                family,
                &[value(
                    "schema",
                    ParameterLiteral::JsonSchema(schema.to_owned()),
                )],
                &default_reasoning(),
                &ReasoningWireDialect::Unsupported,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect("exact structured-output mapping");
            assert_eq!(
                plan.body_patches(),
                &[RequestBodyPatch {
                    path: field.to_owned(),
                    value: expected,
                }]
            );
        }
    }

    #[test]
    fn atomic_request_gate_omits_untouched_and_inherited_values() {
        let engine = ParameterEngine::new(vec![number_spec(
            "temperature",
            "temperature",
            UiParameterLevel::Basic,
        )])
        .expect("valid engine");
        for values in [Vec::new(), vec![inherit("temperature")]] {
            let plan = validate_and_build_provider_request_plan(
                &engine,
                ApiFamily::OpenAiResponses,
                &values,
                &default_reasoning(),
                &ReasoningWireDialect::Unsupported,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect("provider default plan");
            assert!(plan.body_patches().is_empty());
        }
    }

    #[test]
    fn legacy_conditions_and_conflicts_compile() {
        let gate = ParameterSpec {
            id: ParameterId::from("gate"),
            label_key: "gate".to_owned(),
            description_key: None,
            value_type: ParameterType::Boolean,
            allowed_values: Vec::new(),
            minimum: None,
            maximum: None,
            step: None,
            default_mode: ParameterDefaultMode::ProviderDefault,
            visibility: None,
            conflicts: Vec::new(),
            provider_mapping: ProviderParameterMapping {
                target: ProviderParameterTarget::RequestBody,
                field_name: "logprobs".to_owned(),
            },
            level: UiParameterLevel::Advanced,
        };
        let mut dependent = gate.clone();
        dependent.id = ParameterId::from("dependent");
        dependent.provider_mapping.field_name = "parallel_tool_calls".to_owned();
        dependent.visibility = Some(ParameterCondition {
            parameter_id: ParameterId::from("gate"),
            operator: ParameterConditionOperator::Equals,
            value: ParameterLiteral::Boolean(true),
        });
        dependent.conflicts = vec![ParameterConflict {
            parameter_id: ParameterId::from("gate"),
            kind: ParameterConflictKind::Requires,
            message_key: "requires_gate".to_owned(),
        }];
        let engine =
            ParameterEngine::from_manifest_specs(&[gate, dependent]).expect("compile manifest");
        assert!(
            engine
                .validate_for_request(&[value("gate", ParameterLiteral::Boolean(true))])
                .is_ok()
        );
    }

    #[test]
    fn allowlisted_parameter_mapping_is_exact_for_every_family() {
        let cases = [
            (
                ApiFamily::OpenAiResponses,
                applied("max", "max_output_tokens", ParameterLiteral::Integer(123)),
                "max_output_tokens",
                json!(123),
            ),
            (
                ApiFamily::OpenAiChatCompletions,
                applied(
                    "stop",
                    "stop",
                    ParameterLiteral::StopSequenceList(vec!["END".to_owned()]),
                ),
                "stop",
                json!(["END"]),
            ),
            (
                ApiFamily::AnthropicMessages,
                applied(
                    "tool",
                    "tool_choice",
                    ParameterLiteral::ToolPolicy(ToolPolicy::Required),
                ),
                "tool_choice",
                json!({"type":"any"}),
            ),
            (
                ApiFamily::GeminiGenerateContent,
                applied(
                    "top_k",
                    "generationConfig.topK",
                    ParameterLiteral::Integer(20),
                ),
                "generationConfig.topK",
                json!(20),
            ),
            (
                ApiFamily::OllamaNative,
                applied(
                    "temperature",
                    "options.temperature",
                    ParameterLiteral::Number(0.7),
                ),
                "options.temperature",
                json!(0.7),
            ),
        ];

        for (family, parameter, path, expected) in cases {
            let plan = build_provider_request_plan(
                family,
                &validated(vec![parameter]),
                &default_reasoning(),
                &ReasoningWireDialect::Unsupported,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect("allowlisted mapping");
            assert_eq!(
                plan.body_patches,
                vec![RequestBodyPatch {
                    path: path.to_owned(),
                    value: expected
                }]
            );
        }
    }

    #[test]
    fn headers_reserved_fields_and_unknown_fields_are_rejected() {
        let cases = [
            AppliedParameter {
                parameter_id: ParameterId::from("header"),
                mapping: ProviderParameterMapping {
                    target: ProviderParameterTarget::RequestHeader,
                    field_name: "x-custom".to_owned(),
                },
                value: ParameterLiteral::String("value".to_owned()),
            },
            applied(
                "model",
                "model",
                ParameterLiteral::String("other".to_owned()),
            ),
            applied(
                "unknown",
                "arbitrary_field",
                ParameterLiteral::String("value".to_owned()),
            ),
        ];
        for parameter in cases {
            let error = build_provider_request_plan(
                ApiFamily::OpenAiResponses,
                &validated(vec![parameter]),
                &default_reasoning(),
                &ReasoningWireDialect::Unsupported,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect_err("unsafe mapping");
            assert_eq!(error.issues[0].code, ParameterIssueCode::UnsupportedMapping);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn reasoning_mapping_covers_every_built_in_family_without_approximation() {
        let cases = [
            (
                ApiFamily::OpenAiResponses,
                ReasoningSettings {
                    mode: ReasoningMode::Enabled,
                    effort: Some(ReasoningEffort::ExtraHigh),
                    budget_tokens: None,
                    summary: ReasoningSummaryMode::Concise,
                    preserve_opaque_state: true,
                },
                ReasoningWireDialect::OpenAiResponses {
                    efforts: vec![ReasoningEffort::ExtraHigh],
                    summaries: vec![ReasoningSummaryMode::Concise],
                    supports_disabled: true,
                },
                vec![
                    ("reasoning.effort", json!("xhigh")),
                    ("reasoning.summary", json!("concise")),
                ],
            ),
            (
                ApiFamily::OpenAiChatCompletions,
                ReasoningSettings {
                    mode: ReasoningMode::Enabled,
                    effort: Some(ReasoningEffort::High),
                    ..default_reasoning()
                },
                ReasoningWireDialect::OpenAiChatCompletions {
                    efforts: vec![ReasoningEffort::High],
                    supports_disabled: false,
                },
                vec![("reasoning_effort", json!("high"))],
            ),
            (
                ApiFamily::AnthropicMessages,
                ReasoningSettings {
                    mode: ReasoningMode::Automatic,
                    effort: Some(ReasoningEffort::Maximum),
                    summary: ReasoningSummaryMode::Disabled,
                    ..default_reasoning()
                },
                ReasoningWireDialect::AnthropicAdaptive {
                    efforts: vec![ReasoningEffort::Maximum],
                    summaries: vec![ReasoningSummaryMode::Disabled],
                    supports_disabled: true,
                },
                vec![
                    ("output_config.effort", json!("max")),
                    ("thinking.display", json!("omitted")),
                    ("thinking.type", json!("adaptive")),
                ],
            ),
            (
                ApiFamily::GeminiGenerateContent,
                ReasoningSettings {
                    mode: ReasoningMode::Automatic,
                    effort: Some(ReasoningEffort::Minimal),
                    summary: ReasoningSummaryMode::Automatic,
                    ..default_reasoning()
                },
                ReasoningWireDialect::GeminiThinkingLevel {
                    efforts: vec![ReasoningEffort::Minimal],
                    supports_automatic: true,
                    summaries: vec![ReasoningSummaryMode::Automatic],
                },
                vec![
                    (
                        "generationConfig.thinkingConfig.includeThoughts",
                        json!(true),
                    ),
                    (
                        "generationConfig.thinkingConfig.thinkingLevel",
                        json!("minimal"),
                    ),
                ],
            ),
            (
                ApiFamily::OllamaNative,
                ReasoningSettings {
                    mode: ReasoningMode::Enabled,
                    effort: Some(ReasoningEffort::Low),
                    ..default_reasoning()
                },
                ReasoningWireDialect::OllamaLevel {
                    efforts: vec![ReasoningEffort::Low],
                    supports_disabled: false,
                },
                vec![("think", json!("low"))],
            ),
        ];

        for (family, settings, dialect, expected) in cases {
            let plan = build_provider_request_plan(
                family,
                &validated(Vec::new()),
                &settings,
                &dialect,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect("supported exact reasoning mapping");
            assert_eq!(
                plan.body_patches,
                expected
                    .into_iter()
                    .map(|(path, value)| RequestBodyPatch {
                        path: path.to_owned(),
                        value
                    })
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn reasoning_controls_are_render_ready_and_model_specific() {
        let openai_dialect = ReasoningWireDialect::OpenAiResponses {
            efforts: vec![ReasoningEffort::Low, ReasoningEffort::High],
            summaries: vec![
                ReasoningSummaryMode::Automatic,
                ReasoningSummaryMode::Concise,
            ],
            supports_disabled: true,
        };
        let openai = render_reasoning_control(
            ApiFamily::OpenAiResponses,
            &openai_dialect,
            &ReasoningSettings {
                mode: ReasoningMode::Enabled,
                effort: Some(ReasoningEffort::Low),
                summary: ReasoningSummaryMode::Concise,
                ..default_reasoning()
            },
        );
        assert_eq!(openai.state, UiControlState::Ready);
        assert_eq!(openai.effort_field, UiFieldState::Enabled);
        assert_eq!(openai.summary_field, UiFieldState::Enabled);
        assert_eq!(openai.budget_field, UiFieldState::Hidden);
        assert!(openai.allowed_modes.contains(&ReasoningMode::Disabled));

        let anthropic = render_reasoning_control(
            ApiFamily::AnthropicMessages,
            &ReasoningWireDialect::AnthropicAdaptive {
                efforts: vec![ReasoningEffort::Low],
                summaries: Vec::new(),
                supports_disabled: false,
            },
            &default_reasoning(),
        );
        assert!(!anthropic.allowed_modes.contains(&ReasoningMode::Disabled));

        let invalid_ollama = render_reasoning_control(
            ApiFamily::OllamaNative,
            &ReasoningWireDialect::OllamaLevel {
                efforts: vec![ReasoningEffort::Low],
                supports_disabled: false,
            },
            &ReasoningSettings {
                effort: Some(ReasoningEffort::Low),
                ..default_reasoning()
            },
        );
        assert_eq!(invalid_ollama.state, UiControlState::Invalid);
        assert_eq!(invalid_ollama.effort_field, UiFieldState::Hidden);
        assert_eq!(
            invalid_ollama.issues[0].code,
            ParameterIssueCode::UnsupportedReasoning
        );
    }

    #[test]
    fn openrouter_provider_default_omits_every_reasoning_field() {
        for dialect in [
            openrouter_dialect(
                OpenRouterReasoningWireStyle::Unified,
                OpenRouterReasoningEffortSupport::NotExposed,
                None,
                None,
                None,
            ),
            openrouter_dialect(
                OpenRouterReasoningWireStyle::LegacyReasoningEffort,
                OpenRouterReasoningEffortSupport::AllGateway,
                Some(OpenRouterReasoningEffort::Medium),
                None,
                None,
            ),
        ] {
            let plan = build_provider_request_plan(
                ApiFamily::OpenAiChatCompletions,
                &validated(Vec::new()),
                &default_reasoning(),
                &dialect,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect("untouched OpenRouter reasoning defaults");
            assert!(plan.body_patches().is_empty());

            let error = build_provider_request_plan(
                ApiFamily::OpenAiChatCompletions,
                &validated(Vec::new()),
                &ReasoningSettings {
                    effort: Some(ReasoningEffort::Medium),
                    ..default_reasoning()
                },
                &dialect,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect_err("provider-default mode cannot smuggle an effort override");
            assert_eq!(
                error.issues[0].code,
                ParameterIssueCode::UnsupportedReasoning
            );
        }
    }

    #[test]
    fn openrouter_provider_default_rejects_stale_budget_and_summary_fields() {
        let dialect = openrouter_dialect(
            OpenRouterReasoningWireStyle::Unified,
            OpenRouterReasoningEffortSupport::Exact(vec![OpenRouterReasoningEffort::High]),
            Some(OpenRouterReasoningEffort::High),
            Some(true),
            Some(false),
        );
        let mut stale_settings = vec![ReasoningSettings {
            budget_tokens: Some(128),
            ..default_reasoning()
        }];
        stale_settings.extend(
            [
                ReasoningSummaryMode::Disabled,
                ReasoningSummaryMode::Automatic,
                ReasoningSummaryMode::Concise,
                ReasoningSummaryMode::Detailed,
            ]
            .map(|summary| ReasoningSettings {
                summary,
                ..default_reasoning()
            }),
        );

        for settings in stale_settings {
            let error = build_provider_request_plan(
                ApiFamily::OpenAiChatCompletions,
                &validated(Vec::new()),
                &settings,
                &dialect,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect_err("provider-default mode must reject stale hidden reasoning values");
            assert_eq!(
                error.issues[0].code,
                ParameterIssueCode::UnsupportedReasoning
            );
        }
    }

    #[test]
    fn openrouter_mandatory_reasoning_hides_and_rejects_disabled_mode() {
        for style in [
            OpenRouterReasoningWireStyle::Unified,
            OpenRouterReasoningWireStyle::LegacyReasoningEffort,
        ] {
            let dialect = openrouter_dialect(
                style,
                OpenRouterReasoningEffortSupport::Exact(vec![OpenRouterReasoningEffort::High]),
                Some(OpenRouterReasoningEffort::High),
                None,
                Some(true),
            );
            let disabled = ReasoningSettings {
                mode: ReasoningMode::Disabled,
                ..default_reasoning()
            };
            let control =
                render_reasoning_control(ApiFamily::OpenAiChatCompletions, &dialect, &disabled);
            assert!(!control.allowed_modes.contains(&ReasoningMode::Disabled));
            assert_eq!(control.state, UiControlState::Invalid);

            let error = build_provider_request_plan(
                ApiFamily::OpenAiChatCompletions,
                &validated(Vec::new()),
                &disabled,
                &dialect,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect_err("mandatory OpenRouter reasoning cannot be disabled");
            assert_eq!(
                error.issues[0].code,
                ParameterIssueCode::UnsupportedReasoning
            );
        }
    }

    #[test]
    fn openrouter_enabled_default_is_render_only_until_native_adopts_it() {
        let dialect = openrouter_dialect(
            OpenRouterReasoningWireStyle::Unified,
            OpenRouterReasoningEffortSupport::Exact(vec![
                OpenRouterReasoningEffort::High,
                OpenRouterReasoningEffort::Low,
            ]),
            Some(OpenRouterReasoningEffort::High),
            Some(false),
            Some(false),
        );
        let enabled_without_effort = ReasoningSettings {
            mode: ReasoningMode::Enabled,
            ..default_reasoning()
        };
        let control = render_reasoning_control(
            ApiFamily::OpenAiChatCompletions,
            &dialect,
            &enabled_without_effort,
        );
        assert_eq!(control.state, UiControlState::Ready);
        assert_eq!(control.effort_field, UiFieldState::Required);
        assert_eq!(control.settings.effort, Some(ReasoningEffort::High));
        assert_eq!(control.budget_field, UiFieldState::Hidden);

        build_provider_request_plan(
            ApiFamily::OpenAiChatCompletions,
            &validated(Vec::new()),
            &enabled_without_effort,
            &dialect,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect_err("rendered default must not become an implicit wire value");

        let plan = build_provider_request_plan(
            ApiFamily::OpenAiChatCompletions,
            &validated(Vec::new()),
            &control.settings,
            &dialect,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("native-adopted explicit OpenRouter effort");
        assert_eq!(
            plan.body_patches(),
            &[RequestBodyPatch {
                path: "reasoning.effort".to_owned(),
                value: json!("high"),
            }]
        );
    }

    #[test]
    fn openrouter_default_none_is_never_preselected_for_enabled_mode() {
        let dialect = openrouter_dialect(
            OpenRouterReasoningWireStyle::Unified,
            OpenRouterReasoningEffortSupport::Exact(vec![
                OpenRouterReasoningEffort::High,
                OpenRouterReasoningEffort::None,
            ]),
            Some(OpenRouterReasoningEffort::None),
            None,
            Some(false),
        );
        let control = render_reasoning_control(
            ApiFamily::OpenAiChatCompletions,
            &dialect,
            &ReasoningSettings {
                mode: ReasoningMode::Enabled,
                ..default_reasoning()
            },
        );
        assert_eq!(control.settings.effort, None);
        assert_eq!(control.state, UiControlState::Invalid);
        assert_eq!(control.effort_field, UiFieldState::Required);
    }

    #[test]
    fn openrouter_unified_exact_empty_and_none_have_distinct_controls() {
        let exact_empty = openrouter_dialect(
            OpenRouterReasoningWireStyle::Unified,
            OpenRouterReasoningEffortSupport::Exact(Vec::new()),
            None,
            None,
            None,
        );
        let enabled = ReasoningSettings {
            mode: ReasoningMode::Enabled,
            ..default_reasoning()
        };
        let control =
            render_reasoning_control(ApiFamily::OpenAiChatCompletions, &exact_empty, &enabled);
        assert_eq!(control.state, UiControlState::Ready);
        assert_eq!(control.effort_field, UiFieldState::Hidden);
        assert!(control.allowed_modes.contains(&ReasoningMode::Enabled));
        let plan = build_provider_request_plan(
            ApiFamily::OpenAiChatCompletions,
            &validated(Vec::new()),
            &enabled,
            &exact_empty,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("unified reasoning can enable without an effort selector");
        assert_eq!(plan.body_patches()[0].path, "reasoning.enabled");
        assert_eq!(plan.body_patches()[0].value, json!(true));

        let exact_none = openrouter_dialect(
            OpenRouterReasoningWireStyle::Unified,
            OpenRouterReasoningEffortSupport::Exact(vec![OpenRouterReasoningEffort::None]),
            None,
            None,
            Some(false),
        );
        let disabled = ReasoningSettings {
            mode: ReasoningMode::Disabled,
            ..default_reasoning()
        };
        let control =
            render_reasoning_control(ApiFamily::OpenAiChatCompletions, &exact_none, &disabled);
        assert!(!control.allowed_modes.contains(&ReasoningMode::Enabled));
        assert!(control.allowed_modes.contains(&ReasoningMode::Disabled));
        let plan = build_provider_request_plan(
            ApiFamily::OpenAiChatCompletions,
            &validated(Vec::new()),
            &disabled,
            &exact_none,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("exact none disables unified reasoning");
        assert_eq!(plan.body_patches()[0].path, "reasoning.enabled");
        assert_eq!(plan.body_patches()[0].value, json!(false));
        assert!(
            build_provider_request_plan(
                ApiFamily::OpenAiChatCompletions,
                &validated(Vec::new()),
                &enabled,
                &exact_none,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .is_err()
        );
    }

    #[test]
    fn openrouter_legacy_all_gateway_maps_only_explicit_top_level_effort() {
        let dialect = openrouter_dialect(
            OpenRouterReasoningWireStyle::LegacyReasoningEffort,
            OpenRouterReasoningEffortSupport::AllGateway,
            Some(OpenRouterReasoningEffort::Medium),
            None,
            None,
        );
        let enabled_without_effort = ReasoningSettings {
            mode: ReasoningMode::Enabled,
            ..default_reasoning()
        };
        let control = render_reasoning_control(
            ApiFamily::OpenAiChatCompletions,
            &dialect,
            &enabled_without_effort,
        );
        assert_eq!(control.state, UiControlState::Ready);
        assert_eq!(control.settings.effort, Some(ReasoningEffort::Medium));
        assert_eq!(control.effort_field, UiFieldState::Required);
        assert!(control.allowed_modes.contains(&ReasoningMode::Enabled));
        assert_eq!(control.allowed_efforts.len(), 6);
        assert!(
            build_provider_request_plan(
                ApiFamily::OpenAiChatCompletions,
                &validated(Vec::new()),
                &enabled_without_effort,
                &dialect,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .is_err()
        );
        let plan = build_provider_request_plan(
            ApiFamily::OpenAiChatCompletions,
            &validated(Vec::new()),
            &control.settings,
            &dialect,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("explicit adopted legacy effort");
        assert_eq!(plan.body_patches()[0].path, "reasoning_effort");
        assert_eq!(plan.body_patches()[0].value, json!("medium"));
    }

    #[test]
    fn openrouter_legacy_exact_empty_none_and_budget_fail_closed() {
        let enabled = ReasoningSettings {
            mode: ReasoningMode::Enabled,
            ..default_reasoning()
        };
        let exact_empty = openrouter_dialect(
            OpenRouterReasoningWireStyle::LegacyReasoningEffort,
            OpenRouterReasoningEffortSupport::Exact(Vec::new()),
            None,
            None,
            None,
        );
        let control =
            render_reasoning_control(ApiFamily::OpenAiChatCompletions, &exact_empty, &enabled);
        assert!(!control.allowed_modes.contains(&ReasoningMode::Enabled));
        assert!(
            build_provider_request_plan(
                ApiFamily::OpenAiChatCompletions,
                &validated(Vec::new()),
                &enabled,
                &exact_empty,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .is_err()
        );

        let exact_none = openrouter_dialect(
            OpenRouterReasoningWireStyle::LegacyReasoningEffort,
            OpenRouterReasoningEffortSupport::Exact(vec![OpenRouterReasoningEffort::None]),
            None,
            None,
            Some(false),
        );
        let plan = build_provider_request_plan(
            ApiFamily::OpenAiChatCompletions,
            &validated(Vec::new()),
            &ReasoningSettings {
                mode: ReasoningMode::Disabled,
                ..default_reasoning()
            },
            &exact_none,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("legacy none is an exact disable value");
        assert_eq!(plan.body_patches()[0].path, "reasoning_effort");
        assert_eq!(plan.body_patches()[0].value, json!("none"));

        let unbounded_budget = openrouter_dialect(
            OpenRouterReasoningWireStyle::Unified,
            OpenRouterReasoningEffortSupport::Exact(Vec::new()),
            None,
            Some(true),
            None,
        );
        let budget = ReasoningSettings {
            mode: ReasoningMode::Enabled,
            budget_tokens: Some(128),
            ..default_reasoning()
        };
        let control =
            render_reasoning_control(ApiFamily::OpenAiChatCompletions, &unbounded_budget, &budget);
        assert_eq!(control.budget_field, UiFieldState::Hidden);
        assert_eq!(control.state, UiControlState::Invalid);
        assert!(
            build_provider_request_plan(
                ApiFamily::OpenAiChatCompletions,
                &validated(Vec::new()),
                &budget,
                &unbounded_budget,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .is_err()
        );
    }

    #[test]
    fn cache_controls_evaluate_mode_dependent_fields_in_rust() {
        let dialect = PromptCacheWireDialect::Anthropic {
            supports_automatic: true,
            supports_explicit_breakpoints: true,
            supports_one_hour_ttl: true,
        };
        let inherited =
            render_prompt_cache_control(ApiFamily::AnthropicMessages, dialect, &default_cache());
        assert_eq!(inherited.state, UiControlState::Ready);
        assert_eq!(inherited.ttl_field, UiFieldState::Hidden);
        assert_eq!(
            inherited.allowed_ttls,
            vec![PromptCacheTtl::ProviderDefault]
        );
        assert!(!inherited.supports_custom_ttl);

        let automatic = render_prompt_cache_control(
            ApiFamily::AnthropicMessages,
            dialect,
            &PromptCacheSettings {
                mode: PromptCacheMode::Automatic,
                ttl: PromptCacheTtl::Long,
                context_reference: None,
            },
        );
        assert_eq!(automatic.ttl_field, UiFieldState::Enabled);
        assert!(automatic.allowed_ttls.contains(&PromptCacheTtl::Long));
        assert!(!automatic.supports_custom_ttl);

        let missing_context = render_prompt_cache_control(
            ApiFamily::GeminiGenerateContent,
            PromptCacheWireDialect::Gemini {
                supports_implicit: true,
                supports_explicit_context: true,
            },
            &PromptCacheSettings {
                mode: PromptCacheMode::ExplicitContext,
                ttl: PromptCacheTtl::ProviderDefault,
                context_reference: None,
            },
        );
        assert_eq!(missing_context.state, UiControlState::Invalid);
        assert_eq!(missing_context.ttl_field, UiFieldState::Hidden);
        assert_eq!(
            missing_context.context_reference_field,
            UiFieldState::Required
        );
        assert!(!missing_context.supports_custom_ttl);
        assert!(missing_context.custom_ttl_bounds.is_none());
        assert_eq!(
            missing_context.allowed_ttls,
            vec![PromptCacheTtl::ProviderDefault]
        );

        let unsupported_ttl = render_prompt_cache_control(
            ApiFamily::GeminiGenerateContent,
            PromptCacheWireDialect::Gemini {
                supports_implicit: true,
                supports_explicit_context: true,
            },
            &PromptCacheSettings {
                mode: PromptCacheMode::ExplicitContext,
                ttl: PromptCacheTtl::CustomSeconds(3600),
                context_reference: Some("cachedContents/cache_1".to_owned()),
            },
        );
        assert_eq!(unsupported_ttl.state, UiControlState::Invalid);
        assert_eq!(unsupported_ttl.ttl_field, UiFieldState::Hidden);
        assert!(!unsupported_ttl.supports_custom_ttl);
        assert!(unsupported_ttl.custom_ttl_bounds.is_none());
    }

    #[test]
    fn budget_reasoning_dialects_enforce_route_bounds() {
        let anthropic = ReasoningWireDialect::AnthropicExtended {
            minimum_budget_tokens: 1024,
            maximum_budget_tokens: 32_000,
            efforts: Vec::new(),
            summaries: vec![ReasoningSummaryMode::Automatic],
            supports_disabled: true,
        };
        let settings = ReasoningSettings {
            mode: ReasoningMode::Enabled,
            budget_tokens: Some(4096),
            summary: ReasoningSummaryMode::Automatic,
            ..default_reasoning()
        };
        let plan = build_provider_request_plan(
            ApiFamily::AnthropicMessages,
            &validated(Vec::new()),
            &settings,
            &anthropic,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("Anthropic budget");
        assert!(plan.body_patches.contains(&RequestBodyPatch {
            path: "thinking.budget_tokens".to_owned(),
            value: json!(4096)
        }));

        let gemini = ReasoningWireDialect::GeminiThinkingBudget {
            minimum_budget_tokens: 1,
            maximum_budget_tokens: 8192,
            supports_zero_to_disable: true,
            supports_automatic: true,
            summaries: vec![ReasoningSummaryMode::Disabled],
        };
        let disabled = ReasoningSettings {
            mode: ReasoningMode::Disabled,
            summary: ReasoningSummaryMode::Disabled,
            ..default_reasoning()
        };
        let plan = build_provider_request_plan(
            ApiFamily::GeminiGenerateContent,
            &validated(Vec::new()),
            &disabled,
            &gemini,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("Gemini zero disables");
        assert_eq!(
            plan.body_patches,
            vec![
                RequestBodyPatch {
                    path: "generationConfig.thinkingConfig.includeThoughts".to_owned(),
                    value: json!(false)
                },
                RequestBodyPatch {
                    path: "generationConfig.thinkingConfig.thinkingBudget".to_owned(),
                    value: json!(0)
                }
            ]
        );

        let automatic = ReasoningSettings {
            mode: ReasoningMode::Automatic,
            summary: ReasoningSummaryMode::Disabled,
            ..default_reasoning()
        };
        let plan = build_provider_request_plan(
            ApiFamily::GeminiGenerateContent,
            &validated(Vec::new()),
            &automatic,
            &gemini,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("Gemini dynamic-thinking sentinel");
        assert!(plan.body_patches.contains(&RequestBodyPatch {
            path: "generationConfig.thinkingConfig.thinkingBudget".to_owned(),
            value: json!(-1)
        }));
    }

    #[test]
    fn every_gemini_route_rejects_opaque_continuity_before_request_planning() {
        let requested = ReasoningSettings {
            preserve_opaque_state: true,
            ..default_reasoning()
        };
        let dialects = [
            ReasoningWireDialect::Unsupported,
            ReasoningWireDialect::GeminiThinkingLevel {
                efforts: vec![ReasoningEffort::Minimal],
                supports_automatic: true,
                summaries: Vec::new(),
            },
            ReasoningWireDialect::GeminiThinkingBudget {
                minimum_budget_tokens: 1,
                maximum_budget_tokens: 8192,
                supports_zero_to_disable: true,
                supports_automatic: true,
                summaries: Vec::new(),
            },
        ];

        for dialect in dialects {
            let control =
                render_reasoning_control(ApiFamily::GeminiGenerateContent, &dialect, &requested);
            assert_eq!(
                control.state,
                if dialect == ReasoningWireDialect::Unsupported {
                    UiControlState::Hidden
                } else {
                    UiControlState::Invalid
                }
            );
            assert!(!control.settings.preserve_opaque_state);
            assert_eq!(
                control.issues[0].message,
                GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR
            );

            let error = build_provider_request_plan(
                ApiFamily::GeminiGenerateContent,
                &validated(Vec::new()),
                &requested,
                &dialect,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect_err("Gemini opaque continuity must fail before a request plan exists");
            assert_eq!(
                error.issues[0].code,
                ParameterIssueCode::UnsupportedReasoning
            );
            assert_eq!(
                error.issues[0].message,
                GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR
            );
        }

        let plan = build_provider_request_plan(
            ApiFamily::GeminiGenerateContent,
            &validated(Vec::new()),
            &default_reasoning(),
            &ReasoningWireDialect::Unsupported,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("Gemini preset with opaque continuity disabled");
        assert!(!plan.preserves_opaque_reasoning_state());
    }

    #[test]
    fn gemini_thinking_level_modes_are_exact() {
        let gemini_level = ReasoningWireDialect::GeminiThinkingLevel {
            efforts: vec![ReasoningEffort::Minimal],
            supports_automatic: true,
            summaries: Vec::new(),
        };
        let disabled = ReasoningSettings {
            mode: ReasoningMode::Disabled,
            summary: ReasoningSummaryMode::Disabled,
            ..default_reasoning()
        };
        let error = build_provider_request_plan(
            ApiFamily::GeminiGenerateContent,
            &validated(Vec::new()),
            &disabled,
            &gemini_level,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect_err("minimal is not disabled");
        assert_eq!(
            error.issues[0].code,
            ParameterIssueCode::UnsupportedReasoning
        );

        let automatic_without_level = ReasoningSettings {
            mode: ReasoningMode::Automatic,
            ..default_reasoning()
        };
        let error = build_provider_request_plan(
            ApiFamily::GeminiGenerateContent,
            &validated(Vec::new()),
            &automatic_without_level,
            &gemini_level,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect_err("automatic thinkingLevel requires an explicit observed level");
        assert_eq!(
            error.issues[0].code,
            ParameterIssueCode::UnsupportedReasoning
        );

        let provider_default_with_level = ReasoningSettings {
            effort: Some(ReasoningEffort::Minimal),
            ..default_reasoning()
        };
        let error = build_provider_request_plan(
            ApiFamily::GeminiGenerateContent,
            &validated(Vec::new()),
            &provider_default_with_level,
            &gemini_level,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect_err("provider-default thinkingLevel cannot carry an explicit level");
        assert_eq!(
            error.issues[0].code,
            ParameterIssueCode::UnsupportedReasoning
        );
        let control = render_reasoning_control(
            ApiFamily::GeminiGenerateContent,
            &gemini_level,
            &provider_default_with_level,
        );
        assert_eq!(control.state, UiControlState::Invalid);
        assert_eq!(control.effort_field, UiFieldState::Hidden);

        let wrong_family = build_provider_request_plan(
            ApiFamily::OllamaNative,
            &validated(Vec::new()),
            &default_reasoning(),
            &gemini_level,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect_err("dialect mismatch");
        assert_eq!(
            wrong_family.issues[0].code,
            ParameterIssueCode::UnsupportedReasoning
        );
    }

    #[test]
    fn anthropic_provider_default_summary_is_rejected() {
        for dialect in [
            ReasoningWireDialect::AnthropicAdaptive {
                efforts: Vec::new(),
                summaries: vec![ReasoningSummaryMode::Automatic],
                supports_disabled: true,
            },
            ReasoningWireDialect::AnthropicExtended {
                minimum_budget_tokens: 1024,
                maximum_budget_tokens: 8192,
                efforts: Vec::new(),
                summaries: vec![ReasoningSummaryMode::Automatic],
                supports_disabled: true,
            },
        ] {
            let provider_default_with_summary = ReasoningSettings {
                summary: ReasoningSummaryMode::Automatic,
                ..default_reasoning()
            };
            let error = build_provider_request_plan(
                ApiFamily::AnthropicMessages,
                &validated(Vec::new()),
                &provider_default_with_summary,
                &dialect,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect_err("provider-default Anthropic thinking cannot emit summary alone");
            assert_eq!(
                error.issues[0].code,
                ParameterIssueCode::UnsupportedReasoning
            );

            let control = render_reasoning_control(
                ApiFamily::AnthropicMessages,
                &dialect,
                &provider_default_with_summary,
            );
            assert_eq!(control.state, UiControlState::Invalid);
            assert_eq!(control.summary_field, UiFieldState::Hidden);
        }
    }

    #[test]
    fn unsupported_reasoning_is_rejected_instead_of_approximated() {
        let disabled = ReasoningSettings {
            mode: ReasoningMode::Disabled,
            summary: ReasoningSummaryMode::Disabled,
            ..default_reasoning()
        };
        let anthropic_always_on = ReasoningWireDialect::AnthropicAdaptive {
            efforts: vec![ReasoningEffort::Low],
            summaries: Vec::new(),
            supports_disabled: false,
        };
        let error = build_provider_request_plan(
            ApiFamily::AnthropicMessages,
            &validated(Vec::new()),
            &disabled,
            &anthropic_always_on,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect_err("always-on Anthropic thinking");
        assert_eq!(
            error.issues[0].code,
            ParameterIssueCode::UnsupportedReasoning
        );

        let ollama_always_on = ReasoningWireDialect::OllamaLevel {
            efforts: vec![ReasoningEffort::Low],
            supports_disabled: false,
        };
        let error = build_provider_request_plan(
            ApiFamily::OllamaNative,
            &validated(Vec::new()),
            &disabled,
            &ollama_always_on,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect_err("always-on Ollama thinking");
        assert_eq!(
            error.issues[0].code,
            ParameterIssueCode::UnsupportedReasoning
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn cross_feature_constraints_reject_invalid_provider_combinations() {
        let manual = ReasoningWireDialect::AnthropicExtended {
            minimum_budget_tokens: 1024,
            maximum_budget_tokens: 8192,
            efforts: Vec::new(),
            summaries: Vec::new(),
            supports_disabled: true,
        };
        let reasoning = ReasoningSettings {
            mode: ReasoningMode::Enabled,
            budget_tokens: Some(4096),
            ..default_reasoning()
        };
        for parameters in [
            vec![applied(
                "max",
                "max_tokens",
                ParameterLiteral::Integer(4096),
            )],
            vec![
                applied("max", "max_tokens", ParameterLiteral::Integer(8192)),
                applied(
                    "tools",
                    "tool_choice",
                    ParameterLiteral::ToolPolicy(ToolPolicy::Required),
                ),
            ],
        ] {
            let error = build_provider_request_plan(
                ApiFamily::AnthropicMessages,
                &validated(parameters),
                &reasoning,
                &manual,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect_err("invalid manual-thinking combination");
            assert_eq!(
                error.issues[0].code,
                ParameterIssueCode::UnsupportedReasoning
            );
        }

        let adaptive = ReasoningWireDialect::AnthropicAdaptive {
            efforts: Vec::new(),
            summaries: Vec::new(),
            supports_disabled: true,
        };
        let automatic = ReasoningSettings {
            mode: ReasoningMode::Automatic,
            ..default_reasoning()
        };
        let adaptive_forced_tool = build_provider_request_plan(
            ApiFamily::AnthropicMessages,
            &validated(vec![applied(
                "tools",
                "tool_choice",
                ParameterLiteral::ToolPolicy(ToolPolicy::Required),
            )]),
            &automatic,
            &adaptive,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("adaptive thinking supports forced tool choice");
        assert!(
            adaptive_forced_tool
                .body_patches()
                .iter()
                .any(|patch| patch.path == "tool_choice")
        );
        for parameter in [
            applied("temperature", "temperature", ParameterLiteral::Number(0.7)),
            applied("top_k", "top_k", ParameterLiteral::Integer(40)),
            applied("top_p", "top_p", ParameterLiteral::Number(0.9)),
        ] {
            let error = build_provider_request_plan(
                ApiFamily::AnthropicMessages,
                &validated(vec![parameter]),
                &automatic,
                &adaptive,
                &default_cache(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect_err("incompatible adaptive-thinking sampling control");
            assert_eq!(
                error.issues[0].code,
                ParameterIssueCode::UnsupportedReasoning
            );
        }

        let gemini_schema_only = validated(vec![applied(
            "schema",
            "generationConfig.responseJsonSchema",
            ParameterLiteral::JsonSchema(r#"{"type":"object"}"#.to_owned()),
        )]);
        let error = build_provider_request_plan(
            ApiFamily::GeminiGenerateContent,
            &gemini_schema_only,
            &default_reasoning(),
            &ReasoningWireDialect::Unsupported,
            &default_cache(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect_err("schema requires JSON MIME type");
        assert_eq!(error.issues[0].code, ParameterIssueCode::UnsupportedMapping);
    }

    #[test]
    fn prompt_cache_mapping_covers_provider_specific_semantics() {
        let openai = build_provider_request_plan(
            ApiFamily::OpenAiResponses,
            &validated(Vec::new()),
            &default_reasoning(),
            &ReasoningWireDialect::Unsupported,
            &PromptCacheSettings {
                mode: PromptCacheMode::Automatic,
                ttl: PromptCacheTtl::Long,
                context_reference: None,
            },
            PromptCacheWireDialect::OpenAiAutomatic {
                supports_24_hour_retention: true,
            },
        )
        .expect("OpenAI automatic cache");
        assert_eq!(
            openai.body_patches,
            vec![RequestBodyPatch {
                path: "prompt_cache_retention".to_owned(),
                value: json!("24h")
            }]
        );
        assert_eq!(
            openai.prompt_cache,
            PromptCacheDirective::ProviderManagedAutomatic
        );

        for mode in [
            PromptCacheMode::Automatic,
            PromptCacheMode::ExplicitBreakpoints,
        ] {
            let anthropic = build_provider_request_plan(
                ApiFamily::AnthropicMessages,
                &validated(Vec::new()),
                &default_reasoning(),
                &ReasoningWireDialect::Unsupported,
                &PromptCacheSettings {
                    mode,
                    ttl: PromptCacheTtl::Long,
                    context_reference: None,
                },
                PromptCacheWireDialect::Anthropic {
                    supports_automatic: true,
                    supports_explicit_breakpoints: true,
                    supports_one_hour_ttl: true,
                },
            )
            .expect("Anthropic cache mode");
            assert!(matches!(
                anthropic.prompt_cache,
                PromptCacheDirective::AnthropicAutomatic {
                    ttl: AnthropicCacheTtl::OneHour
                } | PromptCacheDirective::AnthropicExplicitBreakpoints {
                    ttl: AnthropicCacheTtl::OneHour
                }
            ));
        }

        let disabled = build_provider_request_plan(
            ApiFamily::AnthropicMessages,
            &validated(Vec::new()),
            &default_reasoning(),
            &ReasoningWireDialect::Unsupported,
            &PromptCacheSettings {
                mode: PromptCacheMode::DisabledIfSupported,
                ttl: PromptCacheTtl::ProviderDefault,
                context_reference: None,
            },
            PromptCacheWireDialect::Anthropic {
                supports_automatic: true,
                supports_explicit_breakpoints: true,
                supports_one_hour_ttl: true,
            },
        )
        .expect("Anthropic cache disable");
        assert_eq!(
            disabled.prompt_cache,
            PromptCacheDirective::AnthropicDisabled
        );

        let gemini = build_provider_request_plan(
            ApiFamily::GeminiGenerateContent,
            &validated(Vec::new()),
            &default_reasoning(),
            &ReasoningWireDialect::Unsupported,
            &PromptCacheSettings {
                mode: PromptCacheMode::ExplicitContext,
                ttl: PromptCacheTtl::ProviderDefault,
                context_reference: Some("cachedContents/cache_123".to_owned()),
            },
            PromptCacheWireDialect::Gemini {
                supports_implicit: true,
                supports_explicit_context: true,
            },
        )
        .expect("Gemini explicit cache");
        assert_eq!(
            gemini.prompt_cache,
            PromptCacheDirective::GeminiExplicitContext {
                resource_name: "cachedContents/cache_123".to_owned(),
                requested_ttl: PromptCacheTtl::ProviderDefault
            }
        );
    }

    #[test]
    fn unsupported_cache_combinations_fail_before_request_construction() {
        let cases = [
            (
                ApiFamily::OpenAiResponses,
                PromptCacheSettings {
                    mode: PromptCacheMode::DisabledIfSupported,
                    ttl: PromptCacheTtl::ProviderDefault,
                    context_reference: None,
                },
                PromptCacheWireDialect::OpenAiAutomatic {
                    supports_24_hour_retention: true,
                },
            ),
            (
                ApiFamily::GeminiGenerateContent,
                PromptCacheSettings {
                    mode: PromptCacheMode::Automatic,
                    ttl: PromptCacheTtl::Long,
                    context_reference: None,
                },
                PromptCacheWireDialect::Gemini {
                    supports_implicit: true,
                    supports_explicit_context: true,
                },
            ),
            (
                ApiFamily::AnthropicMessages,
                PromptCacheSettings {
                    mode: PromptCacheMode::DisabledIfSupported,
                    ttl: PromptCacheTtl::Long,
                    context_reference: None,
                },
                PromptCacheWireDialect::Anthropic {
                    supports_automatic: true,
                    supports_explicit_breakpoints: true,
                    supports_one_hour_ttl: true,
                },
            ),
            (
                ApiFamily::OllamaNative,
                PromptCacheSettings {
                    mode: PromptCacheMode::Automatic,
                    ttl: PromptCacheTtl::ProviderDefault,
                    context_reference: None,
                },
                PromptCacheWireDialect::Unsupported,
            ),
        ];
        for (family, settings, dialect) in cases {
            let error = build_provider_request_plan(
                family,
                &validated(Vec::new()),
                &default_reasoning(),
                &ReasoningWireDialect::Unsupported,
                &settings,
                dialect,
            )
            .expect_err("unsupported cache combination");
            assert!(matches!(
                error.issues[0].code,
                ParameterIssueCode::UnsupportedPromptCache
                    | ParameterIssueCode::InvalidPromptCacheReference
            ));
        }
    }

    #[test]
    fn applying_plan_never_overwrites_adapter_owned_fields() {
        let plan = ProviderRequestPlan {
            family: ApiFamily::OpenAiResponses,
            body_patches: vec![RequestBodyPatch {
                path: "reasoning.effort".to_owned(),
                value: json!("high"),
            }],
            prompt_cache: PromptCacheDirective::None,
            cache_dialect: PromptCacheWireDialect::Unsupported,
            preserve_opaque_reasoning_state: true,
        };
        let mut body = json!({"model":"gpt-test","reasoning":{"summary":"auto"}});
        plan.apply_to_body(&mut body)
            .expect("distinct nested field");
        assert_eq!(body["reasoning"]["effort"], "high");

        let error = plan
            .apply_to_body(&mut body)
            .expect_err("leaf already exists");
        assert_eq!(
            error.issues[0].code,
            ParameterIssueCode::ConflictingRequestField
        );

        let atomic_plan = ProviderRequestPlan {
            family: ApiFamily::OpenAiResponses,
            body_patches: vec![
                RequestBodyPatch {
                    path: "temperature".to_owned(),
                    value: json!(0.4),
                },
                RequestBodyPatch {
                    path: "reasoning.summary".to_owned(),
                    value: json!("concise"),
                },
            ],
            prompt_cache: PromptCacheDirective::None,
            cache_dialect: PromptCacheWireDialect::Unsupported,
            preserve_opaque_reasoning_state: false,
        };
        let mut body = json!({"model":"gpt-test","reasoning":{"summary":"auto"}});
        let original = body.clone();
        atomic_plan
            .apply_to_body(&mut body)
            .expect_err("a later conflicting patch rejects the whole plan");
        assert_eq!(body, original, "failed application must be atomic");
    }
}
