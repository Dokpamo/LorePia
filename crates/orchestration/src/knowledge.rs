use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{
    ActivationRule, CapabilityKey, ConditionExpr, KnowledgeActivationReason, KnowledgeBook,
    KnowledgeEntry, KnowledgeEntryId, KnowledgePlacement, KnowledgeSelectionEvidence,
    SemanticKnowledgeScore, ValidateOrchestration, VariableMap, VariableValue,
};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_KNOWLEDGE_ENTRIES: usize = 10_000;
pub const MAX_ACTIVATION_RULE_DEPTH: usize = 32;
pub const MAX_ACTIVATION_RULE_NODES: usize = 1_024;
pub const MAX_REGEX_PATTERNS_PER_RULE: usize = 64;
pub const MAX_REGEX_PATTERN_BYTES: usize = 4 * 1_024;
pub const MAX_KNOWLEDGE_SCAN_CHARS: usize = 512 * 1_024;
pub const MAX_KNOWLEDGE_RECURSION_DEPTH: u32 = 32;
pub const MAX_ACTIVATION_PROBABILITY_BASIS_POINTS: u16 = 10_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KnowledgeSelectionError {
    #[error("knowledge book is invalid: {message}")]
    InvalidBook { message: String },
    #[error("knowledge book contains more than {MAX_KNOWLEDGE_ENTRIES} entries")]
    TooManyEntries,
    #[error("knowledge entry id is duplicated: {entry_id}")]
    DuplicateEntryId { entry_id: String },
    #[error("knowledge entry belongs to another book: {entry_id}")]
    ForeignBookEntry { entry_id: String },
    #[error("knowledge activation rule exceeds the structural limits for entry: {entry_id}")]
    RuleLimitExceeded { entry_id: String },
    #[error("knowledge regex is invalid for entry {entry_id}: {message}")]
    InvalidRegex { entry_id: String, message: String },
    #[error("semantic score is duplicated for entry: {entry_id}")]
    DuplicateSemanticScore { entry_id: String },
    #[error("semantic score is not finite or is outside 0..=1 for entry: {entry_id}")]
    InvalidSemanticScore { entry_id: String },
    #[error("semantic threshold is not finite or is outside 0..=1 for entry: {entry_id}")]
    InvalidSemanticThreshold { entry_id: String },
    #[error("activation probability is outside 0..=10000 for entry: {entry_id}")]
    InvalidActivationProbability { entry_id: String },
    #[error("knowledge recursion depth exceeds {MAX_KNOWLEDGE_RECURSION_DEPTH}")]
    RecursionDepthLimitExceeded,
    #[error("knowledge activation scan exceeds {MAX_KNOWLEDGE_SCAN_CHARS} characters")]
    ScanLimitExceeded,
}

#[derive(Debug)]
pub struct KnowledgeSelectionContext<'a> {
    /// Canonical message representations ordered from oldest to newest.
    pub scan_texts: &'a [String],
    pub manual_entry_ids: &'a BTreeSet<KnowledgeEntryId>,
    /// Scores supplied by the embedding/search subsystem. This engine never
    /// performs provider networking.
    pub semantic_scores: &'a [SemanticKnowledgeScore],
    pub variables: &'a VariableMap,
    pub supported_capabilities: &'a [CapabilityKey],
    /// Exact tokenizer results may be supplied by the caller. Missing entries
    /// use the deterministic conservative estimator.
    pub token_estimates: &'a BTreeMap<KnowledgeEntryId, u32>,
    /// Persist this seed with the generation so probability gates are
    /// reproducible.
    pub activation_seed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedKnowledgeEntry {
    pub entry_id: KnowledgeEntryId,
    pub content: String,
    pub placement: KnowledgePlacement,
    pub estimated_tokens: u32,
    pub recursion_depth: u32,
    pub reasons: Vec<KnowledgeActivationReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeSelection {
    pub selected: Vec<SelectedKnowledgeEntry>,
    pub evidence: Vec<KnowledgeSelectionEvidence>,
    pub used_tokens: u32,
    pub token_budget: u32,
}

#[derive(Debug, Clone)]
struct ActivatedCandidate {
    reasons: Vec<KnowledgeActivationReason>,
    recursion_depth: u32,
    estimated_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RegexKey {
    pattern: String,
    case_insensitive: bool,
}

struct EvaluationContext<'a> {
    manual_entry_ids: &'a BTreeSet<KnowledgeEntryId>,
    semantic_scores: &'a BTreeMap<KnowledgeEntryId, u32>,
    semantic_rank: &'a BTreeMap<KnowledgeEntryId, usize>,
    variables: &'a VariableMap,
    supported_capabilities: &'a [CapabilityKey],
    regexes: &'a BTreeMap<RegexKey, Regex>,
}

type NormalizedSemanticScores = (
    BTreeMap<KnowledgeEntryId, u32>,
    BTreeMap<KnowledgeEntryId, usize>,
);

pub struct KnowledgeEngine;

impl KnowledgeEngine {
    /// Selects entries without performing I/O. All unordered inputs are
    /// normalized and every tie is resolved by the stable entry id.
    #[allow(clippy::too_many_lines)]
    pub fn select(
        book: &KnowledgeBook,
        context: &KnowledgeSelectionContext<'_>,
    ) -> Result<KnowledgeSelection, KnowledgeSelectionError> {
        validate_book(book)?;
        let entries = normalized_entries(book)?;
        let regexes = compile_regexes(&entries)?;
        let (semantic_scores, semantic_rank) =
            normalize_semantic_scores(&entries, context.semantic_scores)?;
        let evaluation = EvaluationContext {
            manual_entry_ids: context.manual_entry_ids,
            semantic_scores: &semantic_scores,
            semantic_rank: &semantic_rank,
            variables: context.variables,
            supported_capabilities: context.supported_capabilities,
            regexes: &regexes,
        };
        let mut evidence = initial_evidence(&entries, context.token_estimates);
        let mut scan = initial_scan(book, context.scan_texts)?;
        let mut activated = BTreeMap::<KnowledgeEntryId, ActivatedCandidate>::new();

        for entry in &entries {
            if !entry.enabled {
                continue;
            }
            if let Some(reasons) = evaluate_activation(entry, &scan, &evaluation)
                && probability_allows(book, entry, context.activation_seed)
            {
                activated.insert(
                    entry.id.clone(),
                    ActivatedCandidate {
                        reasons,
                        recursion_depth: 0,
                        estimated_tokens: estimated_tokens(entry, context.token_estimates),
                    },
                );
            }
        }

        if book.recursive && book.max_recursion_depth > 0 {
            let mut frontier = activated.keys().cloned().collect::<Vec<_>>();
            for depth in 1..=book.max_recursion_depth {
                if frontier.is_empty() {
                    break;
                }
                frontier.sort();
                for parent_id in &frontier {
                    let Some(parent) = entries.iter().find(|entry| &entry.id == parent_id) else {
                        continue;
                    };
                    append_scan_text(&mut scan, &parent.content)?;
                }

                let frontier_ids = frontier.iter().cloned().collect::<BTreeSet<_>>();
                let Some(fallback_parent) = frontier.first().cloned() else {
                    break;
                };
                let mut next_frontier = Vec::new();
                for entry in &entries {
                    if !entry.enabled || activated.contains_key(&entry.id) {
                        continue;
                    }
                    let Some(mut reasons) = evaluate_activation(entry, &scan, &evaluation) else {
                        continue;
                    };
                    if !probability_allows(book, entry, context.activation_seed) {
                        continue;
                    }
                    let trigger = entry
                        .parent_id
                        .as_ref()
                        .filter(|parent_id| frontier_ids.contains(*parent_id))
                        .cloned()
                        .unwrap_or_else(|| fallback_parent.clone());
                    push_reason(
                        &mut reasons,
                        KnowledgeActivationReason::Recursive { parent_id: trigger },
                    );
                    activated.insert(
                        entry.id.clone(),
                        ActivatedCandidate {
                            reasons,
                            recursion_depth: depth,
                            estimated_tokens: estimated_tokens(entry, context.token_estimates),
                        },
                    );
                    next_frontier.push(entry.id.clone());
                }
                frontier = next_frontier;
            }
        }

        apply_activation_exclusions(
            book,
            &entries,
            &activated,
            context.activation_seed,
            &scan,
            &evaluation,
            &mut evidence,
        );
        let (selected, used_tokens) = apply_budget(book, &entries, &activated, &mut evidence);
        let mut evidence = evidence.into_values().collect::<Vec<_>>();
        evidence.sort_by(|left, right| left.entry_id.cmp(&right.entry_id));

        Ok(KnowledgeSelection {
            selected,
            evidence,
            used_tokens,
            token_budget: book.token_budget.max_tokens,
        })
    }
}

fn validate_book(book: &KnowledgeBook) -> Result<(), KnowledgeSelectionError> {
    if book.entries.len() > MAX_KNOWLEDGE_ENTRIES {
        return Err(KnowledgeSelectionError::TooManyEntries);
    }
    let mut entry_ids = book
        .entries
        .iter()
        .map(|entry| &entry.id)
        .collect::<Vec<_>>();
    entry_ids.sort();
    if let Some(duplicate) = entry_ids
        .windows(2)
        .find(|pair| pair[0] == pair[1])
        .map(|pair| pair[0])
    {
        return Err(KnowledgeSelectionError::DuplicateEntryId {
            entry_id: duplicate.as_str().to_owned(),
        });
    }
    book.validate()
        .map_err(|error| KnowledgeSelectionError::InvalidBook {
            message: error.to_string(),
        })?;
    if book.max_recursion_depth > MAX_KNOWLEDGE_RECURSION_DEPTH {
        return Err(KnowledgeSelectionError::RecursionDepthLimitExceeded);
    }
    Ok(())
}

fn normalized_entries(
    book: &KnowledgeBook,
) -> Result<Vec<&KnowledgeEntry>, KnowledgeSelectionError> {
    let mut entries = book.entries.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    for window in entries.windows(2) {
        if window[0].id == window[1].id {
            return Err(KnowledgeSelectionError::DuplicateEntryId {
                entry_id: window[0].id.as_str().to_owned(),
            });
        }
    }
    for entry in &entries {
        if entry.book_id != book.id {
            return Err(KnowledgeSelectionError::ForeignBookEntry {
                entry_id: entry.id.as_str().to_owned(),
            });
        }
        if entry.activation_probability_basis_points > MAX_ACTIVATION_PROBABILITY_BASIS_POINTS {
            return Err(KnowledgeSelectionError::InvalidActivationProbability {
                entry_id: entry.id.as_str().to_owned(),
            });
        }
        validate_rule(entry)?;
    }
    Ok(entries)
}

fn validate_rule(entry: &KnowledgeEntry) -> Result<(), KnowledgeSelectionError> {
    fn visit(rule: &ActivationRule, depth: usize, nodes: &mut usize) -> Result<(), ()> {
        *nodes = nodes.checked_add(1).ok_or(())?;
        if depth > MAX_ACTIVATION_RULE_DEPTH || *nodes > MAX_ACTIVATION_RULE_NODES {
            return Err(());
        }
        match rule {
            ActivationRule::Regex { patterns } => {
                if patterns.len() > MAX_REGEX_PATTERNS_PER_RULE
                    || patterns
                        .iter()
                        .any(|pattern| pattern.pattern.len() > MAX_REGEX_PATTERN_BYTES)
                {
                    return Err(());
                }
            }
            ActivationRule::Semantic { threshold, .. } => {
                if !threshold.is_finite() || !(0.0..=1.0).contains(threshold) {
                    return Err(());
                }
            }
            ActivationRule::Any { rules } | ActivationRule::All { rules } => {
                if rules.is_empty() {
                    return Err(());
                }
                for child in rules {
                    visit(child, depth + 1, nodes)?;
                }
            }
            ActivationRule::Always
            | ActivationRule::Manual
            | ActivationRule::Keyword { .. }
            | ActivationRule::Condition { .. } => {}
        }
        Ok(())
    }

    let mut nodes = 0;
    visit(&entry.activation, 1, &mut nodes).map_err(|()| {
        if contains_invalid_semantic_threshold(&entry.activation) {
            KnowledgeSelectionError::InvalidSemanticThreshold {
                entry_id: entry.id.as_str().to_owned(),
            }
        } else {
            KnowledgeSelectionError::RuleLimitExceeded {
                entry_id: entry.id.as_str().to_owned(),
            }
        }
    })
}

fn contains_invalid_semantic_threshold(rule: &ActivationRule) -> bool {
    match rule {
        ActivationRule::Semantic { threshold, .. } => {
            !threshold.is_finite() || !(0.0..=1.0).contains(threshold)
        }
        ActivationRule::Any { rules } | ActivationRule::All { rules } => {
            rules.iter().any(contains_invalid_semantic_threshold)
        }
        ActivationRule::Always
        | ActivationRule::Manual
        | ActivationRule::Keyword { .. }
        | ActivationRule::Regex { .. }
        | ActivationRule::Condition { .. } => false,
    }
}

fn compile_regexes(
    entries: &[&KnowledgeEntry],
) -> Result<BTreeMap<RegexKey, Regex>, KnowledgeSelectionError> {
    fn collect<'a>(rule: &'a ActivationRule, output: &mut Vec<&'a lorepia_domain::SafeRegex>) {
        match rule {
            ActivationRule::Regex { patterns } => output.extend(patterns),
            ActivationRule::Any { rules } | ActivationRule::All { rules } => {
                for rule in rules {
                    collect(rule, output);
                }
            }
            ActivationRule::Always
            | ActivationRule::Manual
            | ActivationRule::Keyword { .. }
            | ActivationRule::Semantic { .. }
            | ActivationRule::Condition { .. } => {}
        }
    }

    let mut compiled = BTreeMap::new();
    for entry in entries {
        let mut patterns = Vec::new();
        collect(&entry.activation, &mut patterns);
        for pattern in patterns {
            let key = RegexKey {
                pattern: pattern.pattern.clone(),
                case_insensitive: pattern.case_insensitive,
            };
            if compiled.contains_key(&key) {
                continue;
            }
            let regex = RegexBuilder::new(&pattern.pattern)
                .case_insensitive(pattern.case_insensitive)
                .size_limit(2 * 1_024 * 1_024)
                .dfa_size_limit(4 * 1_024 * 1_024)
                .build()
                .map_err(|error| KnowledgeSelectionError::InvalidRegex {
                    entry_id: entry.id.as_str().to_owned(),
                    message: error.to_string(),
                })?;
            compiled.insert(key, regex);
        }
    }
    Ok(compiled)
}

fn normalize_semantic_scores(
    entries: &[&KnowledgeEntry],
    supplied: &[SemanticKnowledgeScore],
) -> Result<NormalizedSemanticScores, KnowledgeSelectionError> {
    let semantic_entry_ids = entries
        .iter()
        .filter(|entry| entry.enabled && contains_semantic_rule(&entry.activation))
        .map(|entry| entry.id.clone())
        .collect::<BTreeSet<_>>();
    let mut scores = BTreeMap::new();
    for score in supplied {
        if !semantic_entry_ids.contains(&score.entry_id) {
            continue;
        }
        if !score.score.is_finite() || !(0.0..=1.0).contains(&score.score) {
            return Err(KnowledgeSelectionError::InvalidSemanticScore {
                entry_id: score.entry_id.as_str().to_owned(),
            });
        }
        let millionths = score_to_millionths(score.score);
        if scores.insert(score.entry_id.clone(), millionths).is_some() {
            return Err(KnowledgeSelectionError::DuplicateSemanticScore {
                entry_id: score.entry_id.as_str().to_owned(),
            });
        }
    }
    let mut ordered = scores
        .iter()
        .map(|(entry_id, score)| (entry_id.clone(), *score))
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let rank = ordered
        .into_iter()
        .enumerate()
        .map(|(index, (entry_id, _))| (entry_id, index))
        .collect();
    Ok((scores, rank))
}

fn contains_semantic_rule(rule: &ActivationRule) -> bool {
    match rule {
        ActivationRule::Semantic { .. } => true,
        ActivationRule::Any { rules } | ActivationRule::All { rules } => {
            rules.iter().any(contains_semantic_rule)
        }
        ActivationRule::Always
        | ActivationRule::Manual
        | ActivationRule::Keyword { .. }
        | ActivationRule::Regex { .. }
        | ActivationRule::Condition { .. } => false,
    }
}

fn initial_evidence(
    entries: &[&KnowledgeEntry],
    estimates: &BTreeMap<KnowledgeEntryId, u32>,
) -> BTreeMap<KnowledgeEntryId, KnowledgeSelectionEvidence> {
    entries
        .iter()
        .map(|entry| {
            (
                entry.id.clone(),
                KnowledgeSelectionEvidence {
                    entry_id: entry.id.clone(),
                    selected: false,
                    reasons: Vec::new(),
                    estimated_tokens: estimated_tokens(entry, estimates),
                    exclusion_reason: (!entry.enabled).then(|| "entry is disabled".to_owned()),
                },
            )
        })
        .collect()
}

fn initial_scan(
    book: &KnowledgeBook,
    scan_texts: &[String],
) -> Result<String, KnowledgeSelectionError> {
    let depth = usize::try_from(book.scan_depth).unwrap_or(usize::MAX);
    let start = scan_texts.len().saturating_sub(depth);
    let mut scan = String::new();
    for text in &scan_texts[start..] {
        append_scan_text(&mut scan, text)?;
    }
    Ok(scan)
}

fn append_scan_text(scan: &mut String, text: &str) -> Result<(), KnowledgeSelectionError> {
    let separator_chars = usize::from(!scan.is_empty());
    let next_chars = scan
        .chars()
        .count()
        .checked_add(separator_chars)
        .and_then(|count| count.checked_add(text.chars().count()))
        .ok_or(KnowledgeSelectionError::ScanLimitExceeded)?;
    if next_chars > MAX_KNOWLEDGE_SCAN_CHARS {
        return Err(KnowledgeSelectionError::ScanLimitExceeded);
    }
    if !scan.is_empty() {
        scan.push('\n');
    }
    scan.push_str(text);
    Ok(())
}

fn evaluate_activation(
    entry: &KnowledgeEntry,
    scan: &str,
    context: &EvaluationContext<'_>,
) -> Option<Vec<KnowledgeActivationReason>> {
    evaluate_rule(&entry.activation, &entry.id, scan, context)
}

fn evaluate_rule(
    rule: &ActivationRule,
    entry_id: &KnowledgeEntryId,
    scan: &str,
    context: &EvaluationContext<'_>,
) -> Option<Vec<KnowledgeActivationReason>> {
    match rule {
        ActivationRule::Always => Some(vec![KnowledgeActivationReason::Always]),
        ActivationRule::Manual => context
            .manual_entry_ids
            .contains(entry_id)
            .then(|| vec![KnowledgeActivationReason::Manual]),
        ActivationRule::Keyword {
            primary,
            secondary,
            selective,
            case_sensitive,
            whole_word,
        } => {
            let primary_match = first_keyword_match(scan, primary, *case_sensitive, *whole_word)?;
            let mut reasons = vec![KnowledgeActivationReason::Keyword {
                matched: primary_match,
            }];
            if *selective {
                let secondary_match =
                    first_keyword_match(scan, secondary, *case_sensitive, *whole_word)?;
                push_reason(
                    &mut reasons,
                    KnowledgeActivationReason::Keyword {
                        matched: secondary_match,
                    },
                );
            }
            Some(reasons)
        }
        ActivationRule::Regex { patterns } => patterns.iter().find_map(|pattern| {
            let key = RegexKey {
                pattern: pattern.pattern.clone(),
                case_insensitive: pattern.case_insensitive,
            };
            context.regexes.get(&key).and_then(|regex| {
                regex.is_match(scan).then(|| {
                    vec![KnowledgeActivationReason::Regex {
                        pattern: pattern.pattern.clone(),
                    }]
                })
            })
        }),
        ActivationRule::Semantic { threshold, top_k } => {
            let score = *context.semantic_scores.get(entry_id)?;
            let threshold = score_to_millionths(*threshold);
            let rank = *context.semantic_rank.get(entry_id)?;
            (score >= threshold && rank < usize::try_from(*top_k).unwrap_or(usize::MAX)).then(
                || {
                    vec![KnowledgeActivationReason::Semantic {
                        score_millionths: score,
                    }]
                },
            )
        }
        ActivationRule::Condition { expression } => evaluate_condition(
            expression,
            context.variables,
            context.supported_capabilities,
        )
        .then(|| vec![KnowledgeActivationReason::Condition]),
        ActivationRule::Any { rules } => {
            let mut reasons = Vec::new();
            for rule in rules {
                if let Some(child_reasons) = evaluate_rule(rule, entry_id, scan, context) {
                    for reason in child_reasons {
                        push_reason(&mut reasons, reason);
                    }
                }
            }
            (!reasons.is_empty()).then_some(reasons)
        }
        ActivationRule::All { rules } => {
            let mut reasons = Vec::new();
            for rule in rules {
                let child_reasons = evaluate_rule(rule, entry_id, scan, context)?;
                for reason in child_reasons {
                    push_reason(&mut reasons, reason);
                }
            }
            Some(reasons)
        }
    }
}

fn first_keyword_match(
    scan: &str,
    keywords: &[String],
    case_sensitive: bool,
    whole_word: bool,
) -> Option<String> {
    keywords.iter().find_map(|keyword| {
        (!keyword.is_empty() && contains_keyword(scan, keyword, case_sensitive, whole_word))
            .then(|| keyword.clone())
    })
}

fn contains_keyword(scan: &str, keyword: &str, case_sensitive: bool, whole_word: bool) -> bool {
    if !whole_word {
        return if case_sensitive {
            scan.contains(keyword)
        } else {
            scan.to_lowercase().contains(&keyword.to_lowercase())
        };
    }
    let (haystack, needle) = if case_sensitive {
        (scan.to_owned(), keyword.to_owned())
    } else {
        (scan.to_lowercase(), keyword.to_lowercase())
    };
    haystack.match_indices(&needle).any(|(start, matched)| {
        let before = haystack[..start].chars().next_back();
        let end = start + matched.len();
        let after = haystack[end..].chars().next();
        before.is_none_or(|character| !is_word_character(character))
            && after.is_none_or(|character| !is_word_character(character))
    })
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn evaluate_condition(
    condition: &ConditionExpr,
    variables: &VariableMap,
    supported_capabilities: &[CapabilityKey],
) -> bool {
    match condition {
        ConditionExpr::True => true,
        ConditionExpr::False => false,
        ConditionExpr::Equals { variable, value } => variables.get(variable) == Some(value),
        ConditionExpr::NotEquals { variable, value } => variables.get(variable) != Some(value),
        ConditionExpr::GreaterThan { variable, value } => {
            if !value.is_finite() {
                return false;
            }
            match variables.get(variable) {
                Some(VariableValue::Integer(actual)) => integer_greater_than(*actual, *value),
                Some(VariableValue::Decimal(actual)) => actual.is_finite() && *actual > *value,
                _ => false,
            }
        }
        ConditionExpr::Contains { variable, value } => match variables.get(variable) {
            Some(VariableValue::Text(actual) | VariableValue::Enum(actual)) => {
                actual.contains(value)
            }
            Some(VariableValue::StringList(actual)) => actual.iter().any(|item| item == value),
            _ => false,
        },
        ConditionExpr::Exists { variable } => variables.get(variable).is_some(),
        ConditionExpr::ModelSupports { capability } => supported_capabilities.contains(capability),
        ConditionExpr::All { expressions } => expressions
            .iter()
            .all(|condition| evaluate_condition(condition, variables, supported_capabilities)),
        ConditionExpr::Any { expressions } => expressions
            .iter()
            .any(|condition| evaluate_condition(condition, variables, supported_capabilities)),
        ConditionExpr::Not { expression } => {
            !evaluate_condition(expression, variables, supported_capabilities)
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn integer_greater_than(actual: i64, threshold: f64) -> bool {
    (actual as f64) > threshold
}

fn probability_allows(book: &KnowledgeBook, entry: &KnowledgeEntry, seed: u64) -> bool {
    let probability = entry.activation_probability_basis_points;
    if probability == 0 {
        return false;
    }
    if probability == MAX_ACTIVATION_PROBABILITY_BASIS_POINTS {
        return true;
    }
    let mut hasher = Sha256::new();
    update_hash_field(&mut hasher, b"lorepia-knowledge-probability-v1");
    update_hash_field(&mut hasher, book.id.as_str().as_bytes());
    update_hash_field(&mut hasher, entry.id.as_str().as_bytes());
    update_hash_field(&mut hasher, &seed.to_be_bytes());
    let digest = hasher.finalize();
    let sample = u16::from_be_bytes([digest[0], digest[1]]) % 10_000;
    sample < probability
}

fn update_hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

fn apply_activation_exclusions(
    book: &KnowledgeBook,
    entries: &[&KnowledgeEntry],
    activated: &BTreeMap<KnowledgeEntryId, ActivatedCandidate>,
    seed: u64,
    scan: &str,
    context: &EvaluationContext<'_>,
    evidence: &mut BTreeMap<KnowledgeEntryId, KnowledgeSelectionEvidence>,
) {
    for entry in entries {
        let item = evidence
            .get_mut(&entry.id)
            .expect("evidence is initialized for every entry");
        if !entry.enabled {
            continue;
        }
        if let Some(candidate) = activated.get(&entry.id) {
            item.reasons.clone_from(&candidate.reasons);
            continue;
        }
        if evaluate_activation(entry, scan, context).is_some()
            && !probability_allows(book, entry, seed)
        {
            item.exclusion_reason = Some("deterministic activation probability gate".to_owned());
        } else {
            item.exclusion_reason = Some("activation rule did not match".to_owned());
        }
    }
}

fn apply_budget(
    book: &KnowledgeBook,
    entries: &[&KnowledgeEntry],
    activated: &BTreeMap<KnowledgeEntryId, ActivatedCandidate>,
    evidence: &mut BTreeMap<KnowledgeEntryId, KnowledgeSelectionEvidence>,
) -> (Vec<SelectedKnowledgeEntry>, u32) {
    let entries_by_id = entries
        .iter()
        .map(|entry| (entry.id.clone(), *entry))
        .collect::<BTreeMap<_, _>>();
    let mut candidates = activated
        .iter()
        .map(|(entry_id, candidate)| {
            let entry = entries_by_id
                .get(entry_id)
                .expect("activated entry must be in the book");
            (
                activation_tier(&candidate.reasons),
                Reverse(entry.priority),
                Reverse(entry.token_policy.priority),
                Reverse(entry.importance),
                candidate.estimated_tokens,
                entry_id.clone(),
            )
        })
        .collect::<Vec<_>>();
    candidates.sort();

    let mut selected = Vec::new();
    let mut used_tokens = 0_u32;
    for (_, _, _, _, _, entry_id) in candidates {
        let entry = entries_by_id
            .get(&entry_id)
            .expect("candidate entry must be in the book");
        let candidate = activated
            .get(&entry_id)
            .expect("candidate activation must exist");
        let evidence_item = evidence
            .get_mut(&entry_id)
            .expect("candidate evidence must exist");

        if entry
            .token_policy
            .max_tokens
            .is_some_and(|limit| candidate.estimated_tokens > limit)
        {
            evidence_item.exclusion_reason =
                Some("entry exceeds its per-entry token limit".to_owned());
            continue;
        }
        let Some(next_tokens) = used_tokens.checked_add(candidate.estimated_tokens) else {
            evidence_item.exclusion_reason = Some("knowledge token budget overflow".to_owned());
            continue;
        };
        if next_tokens > book.token_budget.max_tokens {
            evidence_item.exclusion_reason =
                Some("entry does not fit the remaining knowledge token budget".to_owned());
            continue;
        }
        used_tokens = next_tokens;
        evidence_item.selected = true;
        evidence_item.exclusion_reason = None;
        selected.push(SelectedKnowledgeEntry {
            entry_id,
            content: entry.content.clone(),
            placement: entry.placement,
            estimated_tokens: candidate.estimated_tokens,
            recursion_depth: candidate.recursion_depth,
            reasons: candidate.reasons.clone(),
        });
    }
    (selected, used_tokens)
}

fn activation_tier(reasons: &[KnowledgeActivationReason]) -> u8 {
    if reasons.contains(&KnowledgeActivationReason::Manual) {
        0
    } else if reasons.contains(&KnowledgeActivationReason::Always) {
        1
    } else {
        2
    }
}

fn estimated_tokens(entry: &KnowledgeEntry, estimates: &BTreeMap<KnowledgeEntryId, u32>) -> u32 {
    estimates
        .get(&entry.id)
        .copied()
        .unwrap_or_else(|| estimate_text_tokens(&entry.content))
}

/// A deterministic fallback for previews before a provider tokenizer result is
/// available. Exact request paths should supply token estimates in the
/// selection context.
#[must_use]
pub fn estimate_text_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let chars = text.chars().count();
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX).max(1)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn score_to_millionths(score: f32) -> u32 {
    // Callers validate that scores are finite and in 0..=1.
    (f64::from(score) * 1_000_000.0).round() as u32
}

fn push_reason(reasons: &mut Vec<KnowledgeActivationReason>, reason: KnowledgeActivationReason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use lorepia_domain::{
        ActivationRule, CapabilityKey, ConditionExpr, ContentModuleId, KnowledgeBook,
        KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId, KnowledgePlacement, Provenance,
        SafeRegex, SemanticKnowledgeScore, SourceKind, TokenBudget, TokenPolicy, VariableId,
        VariableRef, VariableScope, VariableValue,
    };

    use super::{
        KnowledgeEngine, KnowledgeSelectionContext, KnowledgeSelectionError, estimate_text_tokens,
    };

    fn provenance() -> Provenance {
        Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: None,
            source_hash: None,
            author: None,
            license: None,
            imported_at: None,
        }
    }

    fn entry(id: &str, rule: ActivationRule, content: &str) -> KnowledgeEntry {
        KnowledgeEntry {
            id: KnowledgeEntryId::from(id),
            book_id: KnowledgeBookId::from("book"),
            name: id.to_owned(),
            content: content.to_owned(),
            enabled: true,
            activation: rule,
            priority: 0,
            importance: 0,
            placement: KnowledgePlacement::RetrievedContext,
            token_policy: TokenPolicy {
                priority: 0,
                min_tokens: None,
                max_tokens: None,
                reserve_tokens: None,
            },
            parent_id: None,
            activation_probability_basis_points: 10_000,
            provenance: provenance(),
        }
    }

    fn book(entries: Vec<KnowledgeEntry>) -> KnowledgeBook {
        KnowledgeBook {
            id: KnowledgeBookId::from("book"),
            name: "Book".to_owned(),
            schema_version: 1,
            entries,
            scan_depth: 8,
            token_budget: TokenBudget { max_tokens: 10_000 },
            recursive: false,
            max_recursion_depth: 0,
            provenance: provenance(),
        }
    }

    fn select(
        book: &KnowledgeBook,
        scan_texts: &[String],
        manual: &BTreeSet<KnowledgeEntryId>,
        semantic: &[SemanticKnowledgeScore],
        variables: &lorepia_domain::VariableMap,
        estimates: &BTreeMap<KnowledgeEntryId, u32>,
    ) -> Result<super::KnowledgeSelection, KnowledgeSelectionError> {
        KnowledgeEngine::select(
            book,
            &KnowledgeSelectionContext {
                scan_texts,
                manual_entry_ids: manual,
                semantic_scores: semantic,
                variables,
                supported_capabilities: &[CapabilityKey::StructuredOutput],
                token_estimates: estimates,
                activation_seed: 7,
            },
        )
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn all_activation_sources_are_distinct_and_composable() {
        let flag = VariableRef {
            scope: VariableScope::Module,
            namespace: Some(ContentModuleId::from("module")),
            id: VariableId::from("flag"),
        };
        let entries = vec![
            entry("always", ActivationRule::Always, "always"),
            entry("manual", ActivationRule::Manual, "manual"),
            entry(
                "keyword",
                ActivationRule::Keyword {
                    primary: vec!["Silver Fox".to_owned()],
                    secondary: vec!["moon".to_owned()],
                    selective: true,
                    case_sensitive: false,
                    whole_word: true,
                },
                "keyword",
            ),
            entry(
                "regex",
                ActivationRule::Regex {
                    patterns: vec![SafeRegex {
                        pattern: r"\bharbor-\d+\b".to_owned(),
                        case_insensitive: true,
                    }],
                },
                "regex",
            ),
            entry(
                "semantic",
                ActivationRule::Semantic {
                    threshold: 0.75,
                    top_k: 2,
                },
                "semantic",
            ),
            entry(
                "condition",
                ActivationRule::Condition {
                    expression: ConditionExpr::All {
                        expressions: vec![
                            ConditionExpr::Equals {
                                variable: flag.clone(),
                                value: VariableValue::Bool(true),
                            },
                            ConditionExpr::ModelSupports {
                                capability: CapabilityKey::StructuredOutput,
                            },
                        ],
                    },
                },
                "condition",
            ),
            entry(
                "any",
                ActivationRule::Any {
                    rules: vec![
                        ActivationRule::Manual,
                        ActivationRule::Keyword {
                            primary: vec!["moon".to_owned()],
                            secondary: vec![],
                            selective: false,
                            case_sensitive: true,
                            whole_word: true,
                        },
                    ],
                },
                "any",
            ),
            entry(
                "all",
                ActivationRule::All {
                    rules: vec![
                        ActivationRule::Keyword {
                            primary: vec!["moon".to_owned()],
                            secondary: vec![],
                            selective: false,
                            case_sensitive: true,
                            whole_word: true,
                        },
                        ActivationRule::Semantic {
                            threshold: 0.5,
                            top_k: 2,
                        },
                    ],
                },
                "all",
            ),
        ];
        let book = book(entries);
        let scan = vec!["The SILVER FOX reached harbor-42 under the moon.".to_owned()];
        let manual = BTreeSet::from([KnowledgeEntryId::from("manual")]);
        let semantic = vec![
            SemanticKnowledgeScore {
                entry_id: KnowledgeEntryId::from("semantic"),
                score: 0.9,
            },
            SemanticKnowledgeScore {
                entry_id: KnowledgeEntryId::from("all"),
                score: 0.8,
            },
        ];
        let mut variables = lorepia_domain::VariableMap::default();
        variables.insert(flag, VariableValue::Bool(true));
        let selection = select(
            &book,
            &scan,
            &manual,
            &semantic,
            &variables,
            &BTreeMap::new(),
        )
        .expect("select");

        assert_eq!(selection.selected.len(), 8);
        let evidence = selection
            .evidence
            .iter()
            .map(|item| (item.entry_id.as_str(), &item.reasons))
            .collect::<BTreeMap<_, _>>();
        assert!(
            evidence["always"]
                .iter()
                .any(|reason| matches!(reason, lorepia_domain::KnowledgeActivationReason::Always))
        );
        assert!(
            evidence["manual"]
                .iter()
                .any(|reason| matches!(reason, lorepia_domain::KnowledgeActivationReason::Manual))
        );
        assert_eq!(
            evidence["semantic"],
            &[lorepia_domain::KnowledgeActivationReason::Semantic {
                score_millionths: 900_000
            }]
        );
    }

    #[test]
    fn keyword_whole_word_and_selective_rules_do_not_overmatch() {
        let book = book(vec![
            entry(
                "whole",
                ActivationRule::Keyword {
                    primary: vec!["cat".to_owned()],
                    secondary: vec![],
                    selective: false,
                    case_sensitive: false,
                    whole_word: true,
                },
                "whole",
            ),
            entry(
                "selective",
                ActivationRule::Keyword {
                    primary: vec!["cat".to_owned()],
                    secondary: vec!["moon".to_owned()],
                    selective: true,
                    case_sensitive: false,
                    whole_word: true,
                },
                "selective",
            ),
        ]);
        let scan = vec!["A concatenate function, not a CAT, under stars.".to_owned()];
        let selection = select(
            &book,
            &scan,
            &BTreeSet::new(),
            &[],
            &lorepia_domain::VariableMap::default(),
            &BTreeMap::new(),
        )
        .expect("select");

        assert_eq!(
            selection
                .selected
                .iter()
                .map(|item| item.entry_id.as_str())
                .collect::<Vec<_>>(),
            vec!["whole"]
        );
    }

    #[test]
    fn semantic_top_k_uses_score_then_id_for_ties() {
        let mut disabled = entry(
            "disabled",
            ActivationRule::Semantic {
                threshold: 0.5,
                top_k: 1,
            },
            "disabled",
        );
        disabled.enabled = false;
        let book = book(vec![
            entry(
                "a",
                ActivationRule::Semantic {
                    threshold: 0.5,
                    top_k: 1,
                },
                "a",
            ),
            entry(
                "b",
                ActivationRule::Semantic {
                    threshold: 0.5,
                    top_k: 1,
                },
                "b",
            ),
            disabled,
        ]);
        let semantic = vec![
            SemanticKnowledgeScore {
                entry_id: KnowledgeEntryId::from("b"),
                score: 0.8,
            },
            SemanticKnowledgeScore {
                entry_id: KnowledgeEntryId::from("a"),
                score: 0.8,
            },
            SemanticKnowledgeScore {
                entry_id: KnowledgeEntryId::from("disabled"),
                score: 1.0,
            },
        ];
        let selection = select(
            &book,
            &[],
            &BTreeSet::new(),
            &semantic,
            &lorepia_domain::VariableMap::default(),
            &BTreeMap::new(),
        )
        .expect("select");

        assert_eq!(selection.selected.len(), 1);
        assert_eq!(selection.selected[0].entry_id.as_str(), "a");
    }

    #[test]
    fn manual_and_always_entries_are_budgeted_before_optional_priority() {
        let mut manual = entry("manual", ActivationRule::Manual, "manual");
        manual.priority = -10;
        let mut always = entry("always", ActivationRule::Always, "always");
        always.priority = -20;
        let mut optional = entry(
            "optional",
            ActivationRule::Keyword {
                primary: vec!["key".to_owned()],
                secondary: vec![],
                selective: false,
                case_sensitive: true,
                whole_word: true,
            },
            "optional",
        );
        optional.priority = 1_000;
        let mut book = book(vec![optional, always, manual]);
        book.token_budget.max_tokens = 2;
        let estimates = BTreeMap::from([
            (KnowledgeEntryId::from("manual"), 1),
            (KnowledgeEntryId::from("always"), 1),
            (KnowledgeEntryId::from("optional"), 1),
        ]);
        let selection = select(
            &book,
            &["key".to_owned()],
            &BTreeSet::from([KnowledgeEntryId::from("manual")]),
            &[],
            &lorepia_domain::VariableMap::default(),
            &estimates,
        )
        .expect("select");

        assert_eq!(
            selection
                .selected
                .iter()
                .map(|item| item.entry_id.as_str())
                .collect::<Vec<_>>(),
            vec!["manual", "always"]
        );
        assert_eq!(selection.used_tokens, 2);
        let optional = selection
            .evidence
            .iter()
            .find(|item| item.entry_id.as_str() == "optional")
            .expect("optional evidence");
        assert!(!optional.selected);
        assert!(
            optional
                .exclusion_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("remaining"))
        );
    }

    #[test]
    fn recursive_activation_is_bounded_and_visited_once() {
        let first = entry("first", ActivationRule::Always, "second-key");
        let mut second = entry(
            "second",
            ActivationRule::Keyword {
                primary: vec!["second-key".to_owned()],
                secondary: vec![],
                selective: false,
                case_sensitive: true,
                whole_word: true,
            },
            "third-key first-key",
        );
        second.parent_id = Some(first.id.clone());
        let mut third = entry(
            "third",
            ActivationRule::Keyword {
                primary: vec!["third-key".to_owned()],
                secondary: vec![],
                selective: false,
                case_sensitive: true,
                whole_word: true,
            },
            "first-key",
        );
        third.parent_id = Some(second.id.clone());
        let mut book = book(vec![third, second, first]);
        book.recursive = true;
        book.max_recursion_depth = 8;
        let selection = select(
            &book,
            &[],
            &BTreeSet::new(),
            &[],
            &lorepia_domain::VariableMap::default(),
            &BTreeMap::new(),
        )
        .expect("select");

        assert_eq!(selection.selected.len(), 3);
        let depths = selection
            .selected
            .iter()
            .map(|item| (item.entry_id.as_str(), item.recursion_depth))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(depths["first"], 0);
        assert_eq!(depths["second"], 1);
        assert_eq!(depths["third"], 2);
    }

    #[test]
    fn scan_depth_only_uses_the_latest_canonical_inputs() {
        let entry = entry(
            "old",
            ActivationRule::Keyword {
                primary: vec!["forgotten".to_owned()],
                secondary: vec![],
                selective: false,
                case_sensitive: true,
                whole_word: true,
            },
            "old",
        );
        let mut book = book(vec![entry]);
        book.scan_depth = 1;
        let selection = select(
            &book,
            &["forgotten".to_owned(), "current".to_owned()],
            &BTreeSet::new(),
            &[],
            &lorepia_domain::VariableMap::default(),
            &BTreeMap::new(),
        )
        .expect("select");
        assert!(selection.selected.is_empty());
    }

    #[test]
    fn invalid_regex_and_duplicate_ids_fail_before_selection() {
        let invalid = book(vec![entry(
            "invalid",
            ActivationRule::Regex {
                patterns: vec![SafeRegex {
                    pattern: "(".to_owned(),
                    case_insensitive: false,
                }],
            },
            "content",
        )]);
        assert!(matches!(
            select(
                &invalid,
                &[],
                &BTreeSet::new(),
                &[],
                &lorepia_domain::VariableMap::default(),
                &BTreeMap::new()
            ),
            Err(KnowledgeSelectionError::InvalidRegex { .. })
        ));

        let duplicate = book(vec![
            entry("same", ActivationRule::Always, "a"),
            entry("same", ActivationRule::Always, "b"),
        ]);
        assert!(matches!(
            select(
                &duplicate,
                &[],
                &BTreeSet::new(),
                &[],
                &lorepia_domain::VariableMap::default(),
                &BTreeMap::new()
            ),
            Err(KnowledgeSelectionError::DuplicateEntryId { .. })
        ));
    }

    #[test]
    fn probability_gate_is_reproducible_for_the_recorded_seed() {
        let mut probabilistic = entry("coin", ActivationRule::Always, "content");
        probabilistic.activation_probability_basis_points = 5_000;
        let book = book(vec![probabilistic]);
        let run = || {
            select(
                &book,
                &[],
                &BTreeSet::new(),
                &[],
                &lorepia_domain::VariableMap::default(),
                &BTreeMap::new(),
            )
            .expect("select")
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn fallback_token_estimate_is_unicode_scalar_based_and_nonzero() {
        assert_eq!(estimate_text_tokens(""), 0);
        assert_eq!(estimate_text_tokens("가나다라"), 1);
        assert_eq!(estimate_text_tokens("abcdef"), 2);
    }
}
