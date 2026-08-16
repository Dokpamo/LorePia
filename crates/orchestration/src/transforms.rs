use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use lorepia_domain::{
    CapabilityKey, SourceKind, TransformPhase, TransformRule, TransformSet, TransformSetId,
    TransformTrace, VariableMap,
};
use regex::{Captures, Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::interactions::{ConditionContext, InteractionLimits, evaluate_condition_ast};

pub const DEFAULT_MAX_TRANSFORM_SETS: usize = 64;
pub const DEFAULT_MAX_TRANSFORM_RULES: usize = 512;
pub const DEFAULT_MAX_RULES_PER_PHASE: usize = 128;
pub const DEFAULT_MAX_INPUT_CHARS: usize = 256 * 1_024;
pub const DEFAULT_MAX_OUTPUT_CHARS: usize = 256 * 1_024;
pub const DEFAULT_MAX_PATTERN_CHARS: usize = 4 * 1_024;
pub const DEFAULT_MAX_REPLACEMENT_CHARS: usize = 8 * 1_024;
pub const DEFAULT_MAX_REPLACEMENTS_PER_RULE: u32 = 10_000;
pub const DEFAULT_REGEX_SIZE_LIMIT_BYTES: usize = 4 * 1_024 * 1_024;
pub const DEFAULT_REGEX_DFA_SIZE_LIMIT_BYTES: usize = 2 * 1_024 * 1_024;
pub const DEFAULT_MAX_DIFF_CHARS: usize = 8 * 1_024;
pub const DEFAULT_MAX_TRANSFORM_CONDITION_DEPTH: usize = 16;
pub const DEFAULT_MAX_TRANSFORM_CONDITION_NODES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransformLimits {
    pub max_sets: usize,
    pub max_rules: usize,
    pub max_rules_per_phase: usize,
    pub max_input_chars: usize,
    pub max_output_chars: usize,
    pub max_pattern_chars: usize,
    pub max_replacement_chars: usize,
    pub max_replacements_per_rule: u32,
    pub regex_size_limit_bytes: usize,
    pub regex_dfa_size_limit_bytes: usize,
    pub max_diff_chars: usize,
    pub max_condition_depth: usize,
    pub max_condition_nodes: usize,
}

impl Default for TransformLimits {
    fn default() -> Self {
        Self {
            max_sets: DEFAULT_MAX_TRANSFORM_SETS,
            max_rules: DEFAULT_MAX_TRANSFORM_RULES,
            max_rules_per_phase: DEFAULT_MAX_RULES_PER_PHASE,
            max_input_chars: DEFAULT_MAX_INPUT_CHARS,
            max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
            max_pattern_chars: DEFAULT_MAX_PATTERN_CHARS,
            max_replacement_chars: DEFAULT_MAX_REPLACEMENT_CHARS,
            max_replacements_per_rule: DEFAULT_MAX_REPLACEMENTS_PER_RULE,
            regex_size_limit_bytes: DEFAULT_REGEX_SIZE_LIMIT_BYTES,
            regex_dfa_size_limit_bytes: DEFAULT_REGEX_DFA_SIZE_LIMIT_BYTES,
            max_diff_chars: DEFAULT_MAX_DIFF_CHARS,
            max_condition_depth: DEFAULT_MAX_TRANSFORM_CONDITION_DEPTH,
            max_condition_nodes: DEFAULT_MAX_TRANSFORM_CONDITION_NODES,
        }
    }
}

impl TransformLimits {
    fn validate(self) -> Result<Self, TransformFailure> {
        if self.max_sets == 0
            || self.max_rules == 0
            || self.max_rules_per_phase == 0
            || self.max_input_chars == 0
            || self.max_output_chars == 0
            || self.max_pattern_chars == 0
            || self.max_replacement_chars == 0
            || self.max_replacements_per_rule == 0
            || self.regex_size_limit_bytes == 0
            || self.regex_dfa_size_limit_bytes == 0
            || self.max_diff_chars == 0
            || self.max_condition_depth == 0
            || self.max_condition_nodes == 0
        {
            return Err(TransformFailure::new(
                TransformFailureCode::InvalidLimits,
                "all transform limits must be non-zero",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TransformCompileOptions {
    /// Exact provenance source IDs approved by the user. Imported rules require
    /// both this approval and `TransformRule::imported_enabled`.
    pub approved_import_source_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformApplyOptions {
    /// Transforming the fully resolved plan is intentionally opt-in because it
    /// can invalidate role and token-budget decisions.
    pub allow_resolved_prompt: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct TransformContext<'a> {
    pub variables: &'a VariableMap,
    pub model_capabilities: &'a [CapabilityKey],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformRuleStatus {
    Applied,
    NoMatch,
    Disabled,
    PendingImportApproval,
    ResolvedPromptDisabled,
    ConditionFalse,
    Failed,
}

impl TransformRuleStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::NoMatch => "no_match",
            Self::Disabled => "disabled",
            Self::PendingImportApproval => "pending_import_approval",
            Self::ResolvedPromptDisabled => "resolved_prompt_disabled",
            Self::ConditionFalse => "condition_false",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformDiff {
    pub unchanged_prefix_chars: u32,
    pub before_fragment: String,
    pub after_fragment: String,
    pub unchanged_suffix_chars: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformRuleReport {
    pub trace: TransformTrace,
    pub status: TransformRuleStatus,
    pub diff: Option<TransformDiff>,
    /// Content-free execution evidence for trusted persistence code.
    ///
    /// This is deliberately excluded from the preview/document wire format:
    /// callers may persist the hashes and stable failure code, but must never
    /// turn the report's input/output fragments or free-form error text into a
    /// generation diagnostic surface.
    #[serde(skip)]
    pub execution_audit: Option<TransformRuleExecutionAudit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformOutputRendering {
    /// Every transform output is inert text. A caller must never interpret it
    /// as HTML, JavaScript, a file path, or an executable command.
    NativePlainText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformResult {
    pub phase: TransformPhase,
    pub original: String,
    pub output: String,
    pub changed: bool,
    pub rendering: TransformOutputRendering,
    pub reports: Vec<TransformRuleReport>,
    pub diff: Option<TransformDiff>,
    pub error: Option<TransformFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformFailureCode {
    InvalidLimits,
    TooManySets,
    TooManyRules,
    TooManyRulesForPhase,
    DuplicateSetId,
    DuplicateRuleId,
    InvalidIdentifier,
    InvalidRuleLimit,
    PatternTooLarge,
    ReplacementTooLarge,
    InvalidRegex,
    InvalidReplacement,
    InputLimitExceeded,
    OutputLimitExceeded,
    ConditionFailed,
    ImportedRuleMissingSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformFailure {
    pub code: TransformFailureCode,
    pub message: String,
}

/// Bounded, content-free evidence needed to persist one exact rule
/// application without retaining either side of the transformed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformRuleExecutionAudit {
    pub set_id: TransformSetId,
    pub before_sha256: String,
    pub after_sha256: Option<String>,
    pub failure_code: Option<TransformFailureCode>,
}

impl TransformFailureCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidLimits => "invalid_limits",
            Self::TooManySets => "too_many_sets",
            Self::TooManyRules => "too_many_rules",
            Self::TooManyRulesForPhase => "too_many_rules_for_phase",
            Self::DuplicateSetId => "duplicate_set_id",
            Self::DuplicateRuleId => "duplicate_rule_id",
            Self::InvalidIdentifier => "invalid_identifier",
            Self::InvalidRuleLimit => "invalid_rule_limit",
            Self::PatternTooLarge => "pattern_too_large",
            Self::ReplacementTooLarge => "replacement_too_large",
            Self::InvalidRegex => "invalid_regex",
            Self::InvalidReplacement => "invalid_replacement",
            Self::InputLimitExceeded => "input_limit_exceeded",
            Self::OutputLimitExceeded => "output_limit_exceeded",
            Self::ConditionFailed => "condition_failed",
            Self::ImportedRuleMissingSource => "imported_rule_missing_source",
        }
    }
}

impl TransformFailure {
    fn new(code: TransformFailureCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for TransformFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TransformFailure {}

#[derive(Debug, Clone)]
enum CompiledPattern {
    Ready {
        regex: Regex,
        replacement: Vec<ReplacementToken>,
    },
    Invalid(TransformFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplacementToken {
    Literal(String),
    CaptureIndex(usize),
    CaptureName(String),
}

#[derive(Debug, Clone)]
struct CompiledRule {
    set_id: TransformSetId,
    set_enabled: bool,
    rule: TransformRule,
    approved_for_execution: bool,
    output_limit_chars: usize,
    compiled: CompiledPattern,
}

#[derive(Debug, Clone)]
pub struct TransformPipeline {
    rules: Vec<CompiledRule>,
    limits: TransformLimits,
}

impl TransformPipeline {
    pub fn compile(
        sets: &[TransformSet],
        limits: TransformLimits,
    ) -> Result<Self, TransformFailure> {
        Self::compile_with_options(sets, limits, &TransformCompileOptions::default())
    }

    #[allow(clippy::too_many_lines)]
    pub fn compile_with_options(
        sets: &[TransformSet],
        limits: TransformLimits,
        options: &TransformCompileOptions,
    ) -> Result<Self, TransformFailure> {
        let limits = limits.validate()?;
        if sets.len() > limits.max_sets {
            return Err(TransformFailure::new(
                TransformFailureCode::TooManySets,
                format!(
                    "transform set count {} exceeds limit {}",
                    sets.len(),
                    limits.max_sets
                ),
            ));
        }
        let total_rules = sets
            .iter()
            .try_fold(0_usize, |count, set| count.checked_add(set.rules.len()))
            .ok_or_else(|| {
                TransformFailure::new(
                    TransformFailureCode::TooManyRules,
                    "transform rule count overflowed",
                )
            })?;
        if total_rules > limits.max_rules {
            return Err(TransformFailure::new(
                TransformFailureCode::TooManyRules,
                format!(
                    "transform rule count {total_rules} exceeds limit {}",
                    limits.max_rules
                ),
            ));
        }

        let mut set_ids = BTreeSet::new();
        let mut rule_ids = BTreeSet::new();
        let mut phase_counts = [0_usize; 5];
        let mut rules = Vec::with_capacity(total_rules);

        for set in sets {
            validate_identifier("transform set id", set.id.as_str())?;
            if !set_ids.insert(set.id.as_str()) {
                return Err(TransformFailure::new(
                    TransformFailureCode::DuplicateSetId,
                    format!("duplicate transform set id `{}`", set.id.as_str()),
                ));
            }
            let set_rule_limit = usize::try_from(set.max_rules_per_phase).unwrap_or(usize::MAX);
            if set_rule_limit == 0 || set_rule_limit > limits.max_rules_per_phase {
                return Err(TransformFailure::new(
                    TransformFailureCode::InvalidRuleLimit,
                    format!(
                        "transform set `{}` phase rule limit must be within 1..={}",
                        set.id.as_str(),
                        limits.max_rules_per_phase
                    ),
                ));
            }
            let set_output_limit = usize::try_from(set.max_output_chars).unwrap_or(usize::MAX);
            if set_output_limit == 0 || set_output_limit > limits.max_output_chars {
                return Err(TransformFailure::new(
                    TransformFailureCode::InvalidRuleLimit,
                    format!(
                        "transform set `{}` output limit must be within 1..={}",
                        set.id.as_str(),
                        limits.max_output_chars
                    ),
                ));
            }

            let mut set_phase_counts = [0_usize; 5];
            for rule in &set.rules {
                validate_identifier("transform rule id", rule.id.as_str())?;
                if !rule_ids.insert(rule.id.as_str()) {
                    return Err(TransformFailure::new(
                        TransformFailureCode::DuplicateRuleId,
                        format!("duplicate transform rule id `{}`", rule.id.as_str()),
                    ));
                }
                let phase_index = phase_index(rule.phase);
                set_phase_counts[phase_index] = set_phase_counts[phase_index].saturating_add(1);
                phase_counts[phase_index] = phase_counts[phase_index].saturating_add(1);
                if set_phase_counts[phase_index] > set_rule_limit
                    || phase_counts[phase_index] > limits.max_rules_per_phase
                {
                    return Err(TransformFailure::new(
                        TransformFailureCode::TooManyRulesForPhase,
                        format!(
                            "transform phase {:?} exceeds its configured rule limit",
                            rule.phase
                        ),
                    ));
                }

                let input_limit = usize::try_from(rule.input_limit).unwrap_or(usize::MAX);
                let output_limit = usize::try_from(rule.output_limit).unwrap_or(usize::MAX);
                if input_limit == 0
                    || input_limit > limits.max_input_chars
                    || output_limit == 0
                    || output_limit > set_output_limit
                    || rule.max_replacements == 0
                    || rule.max_replacements > limits.max_replacements_per_rule
                {
                    return Err(TransformFailure::new(
                        TransformFailureCode::InvalidRuleLimit,
                        format!(
                            "transform rule `{}` has an invalid input, output, or replacement limit",
                            rule.id.as_str()
                        ),
                    ));
                }

                let approved_for_execution =
                    transform_rule_is_approved(&set.provenance, rule, options)?;
                let compiled = compile_rule_pattern(rule, limits);
                rules.push(CompiledRule {
                    set_id: set.id.clone(),
                    set_enabled: set.enabled,
                    rule: rule.clone(),
                    approved_for_execution,
                    output_limit_chars: output_limit.min(set_output_limit),
                    compiled,
                });
            }
        }

        rules.sort_by(|left, right| {
            phase_index(left.rule.phase)
                .cmp(&phase_index(right.rule.phase))
                .then_with(|| left.rule.order.cmp(&right.rule.order))
                .then_with(|| left.rule.id.cmp(&right.rule.id))
                .then_with(|| left.set_id.cmp(&right.set_id))
        });
        Ok(Self { rules, limits })
    }

    #[must_use]
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    #[allow(clippy::too_many_lines)]
    pub fn apply(
        &self,
        phase: TransformPhase,
        input: &str,
        context: TransformContext<'_>,
        options: TransformApplyOptions,
    ) -> TransformResult {
        let input_chars = input.chars().count();
        if input_chars > self.limits.max_input_chars {
            let error = TransformFailure::new(
                TransformFailureCode::InputLimitExceeded,
                format!(
                    "transform input has {input_chars} characters; limit is {}",
                    self.limits.max_input_chars
                ),
            );
            return TransformResult {
                phase,
                original: input.to_owned(),
                output: input.to_owned(),
                changed: false,
                rendering: TransformOutputRendering::NativePlainText,
                reports: Vec::new(),
                diff: None,
                error: Some(error),
            };
        }

        let mut output = input.to_owned();
        let mut reports = Vec::new();
        for compiled in self.rules.iter().filter(|rule| rule.rule.phase == phase) {
            let rule = &compiled.rule;
            if !compiled.set_enabled || !rule.enabled {
                reports.push(skipped_report(
                    &compiled.set_id,
                    rule,
                    TransformRuleStatus::Disabled,
                    &output,
                ));
                continue;
            }
            if !compiled.approved_for_execution {
                reports.push(skipped_report(
                    &compiled.set_id,
                    rule,
                    TransformRuleStatus::PendingImportApproval,
                    &output,
                ));
                continue;
            }
            if phase == TransformPhase::ResolvedPrompt && !options.allow_resolved_prompt {
                reports.push(skipped_report(
                    &compiled.set_id,
                    rule,
                    TransformRuleStatus::ResolvedPromptDisabled,
                    &output,
                ));
                continue;
            }
            if let Some(condition) = &rule.condition {
                let condition_limits = InteractionLimits {
                    max_condition_depth: self.limits.max_condition_depth,
                    max_condition_nodes: self.limits.max_condition_nodes,
                    max_text_chars: self.limits.max_output_chars,
                    ..InteractionLimits::default()
                };
                match evaluate_condition_ast(
                    condition,
                    ConditionContext {
                        variables: context.variables,
                        model_capabilities: context.model_capabilities,
                    },
                    condition_limits,
                ) {
                    Ok(true) => {}
                    Ok(false) => {
                        reports.push(skipped_report(
                            &compiled.set_id,
                            rule,
                            TransformRuleStatus::ConditionFalse,
                            &output,
                        ));
                        continue;
                    }
                    Err(error) => {
                        reports.push(failed_report(
                            &compiled.set_id,
                            rule,
                            &output,
                            TransformFailure::new(
                                TransformFailureCode::ConditionFailed,
                                error.to_string(),
                            ),
                        ));
                        continue;
                    }
                }
            }

            let rule_input_chars = output.chars().count();
            if rule_input_chars > usize::try_from(rule.input_limit).unwrap_or(usize::MAX) {
                reports.push(failed_report(
                    &compiled.set_id,
                    rule,
                    &output,
                    TransformFailure::new(
                        TransformFailureCode::InputLimitExceeded,
                        format!(
                            "rule `{}` input has {rule_input_chars} characters; limit is {}",
                            rule.id.as_str(),
                            rule.input_limit
                        ),
                    ),
                ));
                continue;
            }

            let (regex, replacement) = match &compiled.compiled {
                CompiledPattern::Ready { regex, replacement } => (regex, replacement),
                CompiledPattern::Invalid(error) => {
                    reports.push(failed_report(
                        &compiled.set_id,
                        rule,
                        &output,
                        error.clone(),
                    ));
                    continue;
                }
            };
            match replace_bounded(
                regex,
                replacement,
                &output,
                rule.max_replacements,
                compiled
                    .output_limit_chars
                    .min(self.limits.max_output_chars),
            ) {
                Ok((candidate, 0)) => {
                    reports.push(skipped_report(
                        &compiled.set_id,
                        rule,
                        TransformRuleStatus::NoMatch,
                        &output,
                    ));
                    debug_assert_eq!(candidate, output);
                }
                Ok((candidate, replacements)) => {
                    let before = output;
                    let diff = make_diff(&before, &candidate, self.limits.max_diff_chars);
                    let trace = TransformTrace {
                        rule_id: rule.id.clone(),
                        applied: true,
                        replacements,
                        input_chars: saturating_u32(before.chars().count()),
                        output_chars: saturating_u32(candidate.chars().count()),
                        error: None,
                    };
                    output = candidate;
                    reports.push(TransformRuleReport {
                        trace,
                        status: TransformRuleStatus::Applied,
                        diff: Some(diff),
                        execution_audit: Some(TransformRuleExecutionAudit {
                            set_id: compiled.set_id.clone(),
                            before_sha256: transform_text_sha256(&before),
                            after_sha256: Some(transform_text_sha256(&output)),
                            failure_code: None,
                        }),
                    });
                }
                Err(error) => {
                    // Rule failure is fail-open for content preservation: the
                    // exact pre-rule text remains the input to later rules.
                    reports.push(failed_report(&compiled.set_id, rule, &output, error));
                }
            }
        }

        let changed = output != input;
        let diff = changed.then(|| make_diff(input, &output, self.limits.max_diff_chars));
        TransformResult {
            phase,
            original: input.to_owned(),
            output,
            changed,
            rendering: TransformOutputRendering::NativePlainText,
            reports,
            diff,
            error: None,
        }
    }
}

pub fn preview_transform_rule(
    rule: &TransformRule,
    input: &str,
    context: TransformContext<'_>,
    limits: TransformLimits,
    compile_options: &TransformCompileOptions,
    apply_options: TransformApplyOptions,
) -> Result<TransformResult, TransformFailure> {
    let set = TransformSet {
        id: TransformSetId::from("preview"),
        name: "Transform preview".to_owned(),
        schema_version: 1,
        enabled: true,
        imported_author_enabled: false,
        rules: vec![rule.clone()],
        max_rules_per_phase: 1,
        max_output_chars: rule.output_limit,
        provenance: rule.provenance.clone(),
    };
    let pipeline = TransformPipeline::compile_with_options(&[set], limits, compile_options)?;
    Ok(pipeline.apply(rule.phase, input, context, apply_options))
}

fn compile_rule_pattern(rule: &TransformRule, limits: TransformLimits) -> CompiledPattern {
    if rule.pattern.pattern.chars().count() > limits.max_pattern_chars {
        return CompiledPattern::Invalid(TransformFailure::new(
            TransformFailureCode::PatternTooLarge,
            format!(
                "rule `{}` regex pattern exceeds the configured character limit",
                rule.id.as_str()
            ),
        ));
    }
    if rule.replacement.chars().count() > limits.max_replacement_chars {
        return CompiledPattern::Invalid(TransformFailure::new(
            TransformFailureCode::ReplacementTooLarge,
            format!(
                "rule `{}` replacement exceeds the configured character limit",
                rule.id.as_str()
            ),
        ));
    }

    let regex = match RegexBuilder::new(&rule.pattern.pattern)
        .case_insensitive(rule.pattern.case_insensitive)
        .size_limit(limits.regex_size_limit_bytes)
        .dfa_size_limit(limits.regex_dfa_size_limit_bytes)
        .build()
    {
        Ok(regex) => regex,
        Err(error) => {
            return CompiledPattern::Invalid(TransformFailure::new(
                TransformFailureCode::InvalidRegex,
                format!("rule `{}` regex is invalid: {error}", rule.id.as_str()),
            ));
        }
    };
    let replacement = match parse_replacement(&rule.replacement, &regex) {
        Ok(replacement) => replacement,
        Err(error) => return CompiledPattern::Invalid(error),
    };
    CompiledPattern::Ready { regex, replacement }
}

fn parse_replacement(
    source: &str,
    regex: &Regex,
) -> Result<Vec<ReplacementToken>, TransformFailure> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0_usize;
    let mut literal_start = 0_usize;

    while cursor < bytes.len() {
        if bytes[cursor] != b'$' {
            cursor += source[cursor..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if literal_start < cursor {
            tokens.push(ReplacementToken::Literal(
                source[literal_start..cursor].to_owned(),
            ));
        }
        cursor += 1;
        if cursor >= bytes.len() {
            return Err(TransformFailure::new(
                TransformFailureCode::InvalidReplacement,
                "replacement has a dangling `$`; use `$$` for a literal dollar sign",
            ));
        }
        if bytes[cursor] == b'$' {
            tokens.push(ReplacementToken::Literal("$".to_owned()));
            cursor += 1;
            literal_start = cursor;
            continue;
        }

        let (capture, next_cursor) = if bytes[cursor] == b'{' {
            let capture_start = cursor + 1;
            let Some(relative_end) = bytes[capture_start..].iter().position(|byte| *byte == b'}')
            else {
                return Err(TransformFailure::new(
                    TransformFailureCode::InvalidReplacement,
                    "replacement capture has no closing `}`",
                ));
            };
            let capture_end = capture_start + relative_end;
            if capture_start == capture_end {
                return Err(TransformFailure::new(
                    TransformFailureCode::InvalidReplacement,
                    "replacement capture name cannot be empty",
                ));
            }
            (&source[capture_start..capture_end], capture_end + 1)
        } else {
            let capture_start = cursor;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
            {
                cursor += 1;
            }
            if capture_start == cursor {
                return Err(TransformFailure::new(
                    TransformFailureCode::InvalidReplacement,
                    "replacement `$` must be followed by `$`, a capture number, or a capture name",
                ));
            }
            (&source[capture_start..cursor], cursor)
        };

        if capture.bytes().all(|byte| byte.is_ascii_digit()) {
            let index = capture.parse::<usize>().map_err(|_| {
                TransformFailure::new(
                    TransformFailureCode::InvalidReplacement,
                    "replacement capture index is too large",
                )
            })?;
            if index >= regex.captures_len() {
                return Err(TransformFailure::new(
                    TransformFailureCode::InvalidReplacement,
                    format!("replacement references unknown capture `${capture}`"),
                ));
            }
            tokens.push(ReplacementToken::CaptureIndex(index));
        } else {
            let exists = regex.capture_names().flatten().any(|name| name == capture);
            if !exists {
                return Err(TransformFailure::new(
                    TransformFailureCode::InvalidReplacement,
                    format!("replacement references unknown capture `${capture}`"),
                ));
            }
            tokens.push(ReplacementToken::CaptureName(capture.to_owned()));
        }
        cursor = next_cursor;
        literal_start = cursor;
    }

    if literal_start < source.len() {
        tokens.push(ReplacementToken::Literal(
            source[literal_start..].to_owned(),
        ));
    }
    Ok(tokens)
}

fn replace_bounded(
    regex: &Regex,
    replacement: &[ReplacementToken],
    input: &str,
    max_replacements: u32,
    max_output_chars: usize,
) -> Result<(String, u32), TransformFailure> {
    let mut output = String::with_capacity(input.len().min(max_output_chars));
    let mut output_chars = 0_usize;
    let mut last_end = 0_usize;
    let mut replacements = 0_u32;

    for captures in regex.captures_iter(input) {
        if replacements >= max_replacements {
            break;
        }
        let Some(whole_match) = captures.get(0) else {
            continue;
        };
        push_bounded(
            &mut output,
            &mut output_chars,
            &input[last_end..whole_match.start()],
            max_output_chars,
        )?;
        expand_replacement(
            &captures,
            replacement,
            &mut output,
            &mut output_chars,
            max_output_chars,
        )?;
        last_end = whole_match.end();
        replacements = replacements.saturating_add(1);
    }

    push_bounded(
        &mut output,
        &mut output_chars,
        &input[last_end..],
        max_output_chars,
    )?;
    Ok((output, replacements))
}

fn expand_replacement(
    captures: &Captures<'_>,
    replacement: &[ReplacementToken],
    output: &mut String,
    output_chars: &mut usize,
    max_output_chars: usize,
) -> Result<(), TransformFailure> {
    for token in replacement {
        match token {
            ReplacementToken::Literal(value) => {
                push_bounded(output, output_chars, value, max_output_chars)?;
            }
            ReplacementToken::CaptureIndex(index) => {
                if let Some(capture) = captures.get(*index) {
                    push_bounded(output, output_chars, capture.as_str(), max_output_chars)?;
                }
            }
            ReplacementToken::CaptureName(name) => {
                if let Some(capture) = captures.name(name) {
                    push_bounded(output, output_chars, capture.as_str(), max_output_chars)?;
                }
            }
        }
    }
    Ok(())
}

fn push_bounded(
    output: &mut String,
    output_chars: &mut usize,
    value: &str,
    max_output_chars: usize,
) -> Result<(), TransformFailure> {
    let added_chars = value.chars().count();
    let next_chars = output_chars.checked_add(added_chars).ok_or_else(|| {
        TransformFailure::new(
            TransformFailureCode::OutputLimitExceeded,
            "transform output length overflowed",
        )
    })?;
    if next_chars > max_output_chars {
        return Err(TransformFailure::new(
            TransformFailureCode::OutputLimitExceeded,
            format!("transform output would exceed {max_output_chars} characters"),
        ));
    }
    output.push_str(value);
    *output_chars = next_chars;
    Ok(())
}

fn transform_rule_is_approved(
    set_provenance: &lorepia_domain::Provenance,
    rule: &TransformRule,
    options: &TransformCompileOptions,
) -> Result<bool, TransformFailure> {
    let set_source = imported_source_id(set_provenance)?;
    let rule_source = imported_source_id(&rule.provenance)?;
    if let Some(set_source) = set_source {
        if rule_source != Some(set_source) {
            return Err(TransformFailure::new(
                TransformFailureCode::ImportedRuleMissingSource,
                "an imported transform set may contain only rules from the same imported source",
            ));
        }
        return Ok(rule.imported_enabled && options.approved_import_source_ids.contains(set_source));
    }
    Ok(rule_source.is_none_or(|source| {
        rule.imported_enabled && options.approved_import_source_ids.contains(source)
    }))
}

fn imported_source_id(
    provenance: &lorepia_domain::Provenance,
) -> Result<Option<&str>, TransformFailure> {
    if matches!(
        provenance.source_kind,
        SourceKind::ImportedPackage | SourceKind::ImportedStandard
    ) {
        return provenance.source_id.as_deref().map(Some).ok_or_else(|| {
            TransformFailure::new(
                TransformFailureCode::ImportedRuleMissingSource,
                "imported transform content requires a provenance source ID",
            )
        });
    }
    Ok(None)
}

fn skipped_report(
    set_id: &TransformSetId,
    rule: &TransformRule,
    status: TransformRuleStatus,
    input: &str,
) -> TransformRuleReport {
    let chars = saturating_u32(input.chars().count());
    TransformRuleReport {
        trace: TransformTrace {
            rule_id: rule.id.clone(),
            applied: false,
            replacements: 0,
            input_chars: chars,
            output_chars: chars,
            error: None,
        },
        status,
        diff: None,
        execution_audit: Some(TransformRuleExecutionAudit {
            set_id: set_id.clone(),
            before_sha256: transform_text_sha256(input),
            after_sha256: Some(transform_text_sha256(input)),
            failure_code: None,
        }),
    }
}

fn failed_report(
    set_id: &TransformSetId,
    rule: &TransformRule,
    input: &str,
    error: TransformFailure,
) -> TransformRuleReport {
    let chars = saturating_u32(input.chars().count());
    TransformRuleReport {
        trace: TransformTrace {
            rule_id: rule.id.clone(),
            applied: false,
            replacements: 0,
            input_chars: chars,
            output_chars: chars,
            error: Some(error.to_string()),
        },
        status: TransformRuleStatus::Failed,
        diff: None,
        execution_audit: Some(TransformRuleExecutionAudit {
            set_id: set_id.clone(),
            before_sha256: transform_text_sha256(input),
            after_sha256: None,
            failure_code: Some(error.code),
        }),
    }
}

fn transform_text_sha256(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn make_diff(before: &str, after: &str, max_fragment_chars: usize) -> TransformDiff {
    let before_chars = before.chars().collect::<Vec<_>>();
    let after_chars = after.chars().collect::<Vec<_>>();
    let prefix = before_chars
        .iter()
        .zip(&after_chars)
        .take_while(|(left, right)| left == right)
        .count();
    let maximum_suffix = before_chars
        .len()
        .saturating_sub(prefix)
        .min(after_chars.len().saturating_sub(prefix));
    let suffix = before_chars
        .iter()
        .rev()
        .zip(after_chars.iter().rev())
        .take(maximum_suffix)
        .take_while(|(left, right)| left == right)
        .count();

    let before_middle = &before_chars[prefix..before_chars.len().saturating_sub(suffix)];
    let after_middle = &after_chars[prefix..after_chars.len().saturating_sub(suffix)];
    let before_fragment = before_middle
        .iter()
        .take(max_fragment_chars)
        .collect::<String>();
    let after_fragment = after_middle
        .iter()
        .take(max_fragment_chars)
        .collect::<String>();
    TransformDiff {
        unchanged_prefix_chars: saturating_u32(prefix),
        before_fragment,
        after_fragment,
        unchanged_suffix_chars: saturating_u32(suffix),
        truncated: before_middle.len() > max_fragment_chars
            || after_middle.len() > max_fragment_chars,
    }
}

const fn phase_index(phase: TransformPhase) -> usize {
    match phase {
        TransformPhase::UserInputForRequest => 0,
        TransformPhase::ResolvedPrompt => 1,
        TransformPhase::ProviderOutputCanonical => 2,
        TransformPhase::DisplayOnly => 3,
        TransformPhase::MemoryInput => 4,
    }
}

fn validate_identifier(label: &str, value: &str) -> Result<(), TransformFailure> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(TransformFailure::new(
            TransformFailureCode::InvalidIdentifier,
            format!(
                "{label} must contain only ASCII letters, digits, `.`, `_`, or `-` and fit 128 bytes"
            ),
        ));
    }
    Ok(())
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{
        ConditionExpr, Provenance, SafeRegex, SourceKind, TransformRuleId, VariableId, VariableRef,
        VariableScope,
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

    fn rule(id: &str, phase: TransformPhase, pattern: &str, replacement: &str) -> TransformRule {
        TransformRule {
            id: TransformRuleId::from(id),
            name: id.to_owned(),
            enabled: true,
            imported_enabled: false,
            imported_author_enabled: false,
            phase,
            order: 0,
            pattern: SafeRegex {
                pattern: pattern.to_owned(),
                case_insensitive: false,
            },
            replacement: replacement.to_owned(),
            condition: None,
            max_replacements: 100,
            input_limit: 1_024,
            output_limit: 1_024,
            provenance: provenance(SourceKind::UserCreated, None),
        }
    }

    fn set(rules: Vec<TransformRule>) -> TransformSet {
        TransformSet {
            id: TransformSetId::from("set"),
            name: "Set".to_owned(),
            schema_version: 1,
            enabled: true,
            imported_author_enabled: false,
            rules,
            max_rules_per_phase: 64,
            max_output_chars: 1_024,
            provenance: provenance(SourceKind::UserCreated, None),
        }
    }

    fn empty_variables() -> VariableMap {
        VariableMap::default()
    }

    fn context(variables: &VariableMap) -> TransformContext<'_> {
        TransformContext {
            variables,
            model_capabilities: &[],
        }
    }

    #[test]
    fn applies_rules_once_in_deterministic_order_with_trace_and_diff() {
        let mut later = rule("later", TransformPhase::UserInputForRequest, "cat", "fox");
        later.order = 20;
        let mut earlier = rule("earlier", TransformPhase::UserInputForRequest, "dog", "cat");
        earlier.order = 10;
        let pipeline =
            TransformPipeline::compile(&[set(vec![later, earlier])], TransformLimits::default())
                .expect("compile");
        let variables = empty_variables();
        let result = pipeline.apply(
            TransformPhase::UserInputForRequest,
            "dog",
            context(&variables),
            TransformApplyOptions::default(),
        );

        assert_eq!(result.output, "fox");
        assert_eq!(
            result
                .reports
                .iter()
                .map(|report| report.trace.rule_id.as_str())
                .collect::<Vec<_>>(),
            vec!["earlier", "later"]
        );
        assert!(result.reports.iter().all(|report| {
            report.status == TransformRuleStatus::Applied && report.diff.is_some()
        }));
        let first_audit = result.reports[0]
            .execution_audit
            .as_ref()
            .expect("trusted execution audit");
        assert_eq!(first_audit.set_id.as_str(), "set");
        assert_eq!(first_audit.before_sha256, transform_text_sha256("dog"));
        assert_eq!(
            first_audit.after_sha256.as_deref(),
            Some(transform_text_sha256("cat").as_str())
        );
        assert_eq!(first_audit.failure_code, None);
        let wire = serde_json::to_string(&result.reports[0]).expect("serialize preview report");
        assert!(!wire.contains("execution_audit"));
        assert!(!wire.contains(&first_audit.before_sha256));
        assert_eq!(
            result.diff,
            Some(TransformDiff {
                unchanged_prefix_chars: 0,
                before_fragment: "dog".to_owned(),
                after_fragment: "fox".to_owned(),
                unchanged_suffix_chars: 0,
                truncated: false,
            })
        );
    }

    #[test]
    fn one_rule_is_not_repeated_to_a_fixed_point() {
        let pipeline = TransformPipeline::compile(
            &[set(vec![rule(
                "grow",
                TransformPhase::DisplayOnly,
                "a",
                "aa",
            )])],
            TransformLimits::default(),
        )
        .expect("compile");
        let variables = empty_variables();
        let result = pipeline.apply(
            TransformPhase::DisplayOnly,
            "a",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(result.output, "aa");
        assert_eq!(result.reports[0].trace.replacements, 1);
        assert_eq!(result.rendering, TransformOutputRendering::NativePlainText);
    }

    #[test]
    fn named_and_numbered_captures_expand_with_literal_dollar() {
        let pipeline = TransformPipeline::compile(
            &[set(vec![rule(
                "captures",
                TransformPhase::ProviderOutputCanonical,
                r"(?P<word>[a-z]+)-(\d+)",
                "${word}:$2:$$",
            )])],
            TransformLimits::default(),
        )
        .expect("compile");
        let variables = empty_variables();
        let result = pipeline.apply(
            TransformPhase::ProviderOutputCanonical,
            "item-42",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(result.output, "item:42:$");
    }

    #[test]
    fn invalid_regex_and_replacement_preserve_original_at_runtime() {
        let pipeline = TransformPipeline::compile(
            &[set(vec![
                rule("bad-regex", TransformPhase::MemoryInput, "(", "lost"),
                rule("bad-capture", TransformPhase::MemoryInput, "(ok)", "$9"),
            ])],
            TransformLimits::default(),
        )
        .expect("invalid individual rules remain inspectable");
        let variables = empty_variables();
        let result = pipeline.apply(
            TransformPhase::MemoryInput,
            "original",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(result.output, "original");
        assert!(!result.changed);
        assert_eq!(result.reports.len(), 2);
        assert!(
            result
                .reports
                .iter()
                .all(|report| report.status == TransformRuleStatus::Failed)
        );
        assert!(
            result
                .reports
                .iter()
                .all(|report| report.trace.error.is_some())
        );
    }

    #[test]
    fn output_limit_failure_keeps_pre_rule_text_and_later_rules_continue() {
        let mut explosive = rule("explosive", TransformPhase::DisplayOnly, "(.*)", "$1$1$1$1");
        explosive.output_limit = 5;
        let mut later = rule("later", TransformPhase::DisplayOnly, "hello", "safe");
        later.order = 1;
        let pipeline =
            TransformPipeline::compile(&[set(vec![explosive, later])], TransformLimits::default())
                .expect("compile");
        let variables = empty_variables();
        let result = pipeline.apply(
            TransformPhase::DisplayOnly,
            "hello",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(result.reports[0].status, TransformRuleStatus::Failed);
        let failed_audit = result.reports[0]
            .execution_audit
            .as_ref()
            .expect("failed execution audit");
        assert_eq!(
            failed_audit.failure_code,
            Some(TransformFailureCode::OutputLimitExceeded)
        );
        assert!(failed_audit.after_sha256.is_none());
        assert_eq!(
            result.reports[0].trace.error.as_deref(),
            Some("transform output would exceed 5 characters")
        );
        assert_eq!(result.output, "safe");
    }

    #[test]
    fn replacement_and_input_counts_are_bounded() {
        let mut bounded = rule("bounded", TransformPhase::UserInputForRequest, "a", "b");
        bounded.max_replacements = 2;
        bounded.input_limit = 4;
        let pipeline =
            TransformPipeline::compile(&[set(vec![bounded])], TransformLimits::default())
                .expect("compile");
        let variables = empty_variables();
        let applied = pipeline.apply(
            TransformPhase::UserInputForRequest,
            "aaaa",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(applied.output, "bbaa");
        assert_eq!(applied.reports[0].trace.replacements, 2);

        let rejected = pipeline.apply(
            TransformPhase::UserInputForRequest,
            "aaaaa",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(rejected.output, "aaaaa");
        assert_eq!(rejected.reports[0].status, TransformRuleStatus::Failed);
    }

    #[test]
    fn resolved_prompt_phase_is_disabled_unless_explicitly_allowed() {
        let pipeline = TransformPipeline::compile(
            &[set(vec![rule(
                "prompt",
                TransformPhase::ResolvedPrompt,
                "secret",
                "changed",
            )])],
            TransformLimits::default(),
        )
        .expect("compile");
        let variables = empty_variables();
        let blocked = pipeline.apply(
            TransformPhase::ResolvedPrompt,
            "secret",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(blocked.output, "secret");
        assert_eq!(
            blocked.reports[0].status,
            TransformRuleStatus::ResolvedPromptDisabled
        );

        let allowed = pipeline.apply(
            TransformPhase::ResolvedPrompt,
            "secret",
            context(&variables),
            TransformApplyOptions {
                allow_resolved_prompt: true,
            },
        );
        assert_eq!(allowed.output, "changed");
    }

    #[test]
    fn imported_rules_need_both_activation_and_exact_source_approval() {
        let mut imported = rule("imported", TransformPhase::DisplayOnly, "before", "after");
        imported.imported_enabled = true;
        imported.provenance = provenance(SourceKind::ImportedPackage, Some("module-a"));
        let mut transform_set = set(vec![imported]);
        transform_set.provenance = provenance(SourceKind::ImportedPackage, Some("module-a"));
        let default_pipeline = TransformPipeline::compile(
            std::slice::from_ref(&transform_set),
            TransformLimits::default(),
        )
        .expect("compile pending import");
        let variables = empty_variables();
        let pending = default_pipeline.apply(
            TransformPhase::DisplayOnly,
            "before",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(pending.output, "before");
        assert_eq!(
            pending.reports[0].status,
            TransformRuleStatus::PendingImportApproval
        );

        let approved_pipeline = TransformPipeline::compile_with_options(
            &[transform_set],
            TransformLimits::default(),
            &TransformCompileOptions {
                approved_import_source_ids: BTreeSet::from(["module-a".to_owned()]),
            },
        )
        .expect("approved compile");
        let applied = approved_pipeline.apply(
            TransformPhase::DisplayOnly,
            "before",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(applied.output, "after");
    }

    #[test]
    fn conditions_are_typed_and_phase_rules_are_isolated() {
        let flag = VariableRef {
            scope: VariableScope::Conversation,
            namespace: None,
            id: VariableId::from("enabled"),
        };
        let mut conditional = rule(
            "conditional",
            TransformPhase::MemoryInput,
            "before",
            "after",
        );
        conditional.condition = Some(ConditionExpr::Equals {
            variable: flag.clone(),
            value: lorepia_domain::VariableValue::Bool(true),
        });
        let other_phase = rule("other", TransformPhase::DisplayOnly, "before", "wrong");
        let pipeline = TransformPipeline::compile(
            &[set(vec![conditional, other_phase])],
            TransformLimits::default(),
        )
        .expect("compile");
        let variables = VariableMap::default();
        let skipped = pipeline.apply(
            TransformPhase::MemoryInput,
            "before",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(skipped.output, "before");
        assert_eq!(skipped.reports.len(), 1);
        assert_eq!(
            skipped.reports[0].status,
            TransformRuleStatus::ConditionFalse
        );

        let mut enabled = VariableMap::default();
        enabled.insert(flag, lorepia_domain::VariableValue::Bool(true));
        let applied = pipeline.apply(
            TransformPhase::MemoryInput,
            "before",
            context(&enabled),
            TransformApplyOptions::default(),
        );
        assert_eq!(applied.output, "after");
    }

    #[test]
    fn imported_set_cannot_disguise_a_child_rule_as_user_authored() {
        let mut imported_set = set(vec![rule(
            "disguised",
            TransformPhase::DisplayOnly,
            "before",
            "after",
        )]);
        imported_set.provenance = provenance(SourceKind::ImportedPackage, Some("hostile-module"));
        let error = TransformPipeline::compile(&[imported_set], TransformLimits::default())
            .expect_err("import provenance mismatch must fail");
        assert_eq!(error.code, TransformFailureCode::ImportedRuleMissingSource);
    }

    #[test]
    fn rust_regex_handles_nested_quantifier_without_backtracking_engine() {
        let mut safe = rule("linear", TransformPhase::DisplayOnly, r"(a+)+$", "matched");
        safe.input_limit = 10_000;
        safe.output_limit = 10_000;
        let mut transform_set = set(vec![safe]);
        transform_set.max_output_chars = 10_000;
        let pipeline = TransformPipeline::compile(&[transform_set], TransformLimits::default())
            .expect("Rust regex accepts the expression in its finite automata engine");
        let input = format!("{}!", "a".repeat(9_000));
        let variables = empty_variables();
        let result = pipeline.apply(
            TransformPhase::DisplayOnly,
            &input,
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(result.output, input);
        assert_eq!(result.reports[0].status, TransformRuleStatus::NoMatch);
    }

    #[test]
    fn unicode_diff_counts_characters_not_bytes_and_truncates() {
        let diff = make_diff("가나다라마바사", "가나XYZ바사", 2);
        assert_eq!(diff.unchanged_prefix_chars, 2);
        assert_eq!(diff.before_fragment, "다라");
        assert_eq!(diff.after_fragment, "XY");
        assert_eq!(diff.unchanged_suffix_chars, 2);
        assert!(diff.truncated);
    }

    #[test]
    fn oversized_pipeline_input_preserves_original_without_running_rules() {
        let limits = TransformLimits {
            max_input_chars: 4,
            ..TransformLimits::default()
        };
        let mut bounded = rule("bounded", TransformPhase::DisplayOnly, ".", "x");
        bounded.input_limit = 4;
        bounded.output_limit = 4;
        let mut transform_set = set(vec![bounded]);
        transform_set.max_output_chars = 4;
        let pipeline = TransformPipeline::compile(&[transform_set], limits).expect("compile");
        let variables = empty_variables();
        let result = pipeline.apply(
            TransformPhase::DisplayOnly,
            "12345",
            context(&variables),
            TransformApplyOptions::default(),
        );
        assert_eq!(result.output, "12345");
        assert!(!result.changed);
        assert_eq!(
            result.error.as_ref().map(|error| &error.code),
            Some(&TransformFailureCode::InputLimitExceeded)
        );
        assert!(result.reports.is_empty());
    }
}
