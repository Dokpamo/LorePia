use std::collections::BTreeSet;

use lorepia_domain::{
    Provenance, SafeRegex, SourceKind, TransformPhase, TransformRule, TransformRuleId,
    TransformSet, TransformSetId, VariableMap,
};
use lorepia_orchestration::{
    TransformApplyOptions, TransformCompileOptions, TransformContext, TransformLimits,
    TransformOutputRendering, TransformPipeline, TransformRuleStatus,
};

fn provenance(source_kind: SourceKind, source_id: Option<&str>) -> Provenance {
    Provenance {
        source_kind,
        source_id: source_id.map(str::to_owned),
        source_hash: Some("synthetic-source-hash".to_owned()),
        author: Some("Synthetic Author".to_owned()),
        license: Some("MIT".to_owned()),
        imported_at: None,
    }
}

fn rule(id: &str, phase: TransformPhase, pattern: &str, replacement: &str) -> TransformRule {
    TransformRule {
        id: TransformRuleId::from(id),
        name: format!("Synthetic {id}"),
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
        max_replacements: 32,
        input_limit: 1_024,
        output_limit: 1_024,
        provenance: provenance(SourceKind::UserCreated, None),
    }
}

fn set(rule: TransformRule) -> TransformSet {
    TransformSet {
        id: TransformSetId::from("synthetic.transform-set"),
        name: "Synthetic transform set".to_owned(),
        schema_version: 1,
        enabled: true,
        imported_author_enabled: false,
        rules: vec![rule],
        max_rules_per_phase: 8,
        max_output_chars: 1_024,
        provenance: provenance(SourceKind::UserCreated, None),
    }
}

fn apply(
    pipeline: &TransformPipeline,
    phase: TransformPhase,
    input: &str,
) -> lorepia_orchestration::TransformResult {
    let variables = VariableMap::default();
    pipeline.apply(
        phase,
        input,
        TransformContext {
            variables: &variables,
            model_capabilities: &[],
        },
        TransformApplyOptions::default(),
    )
}

#[test]
fn invalid_regex_preserves_the_exact_original_and_reports_failure() {
    let pipeline = TransformPipeline::compile(
        &[set(rule(
            "synthetic.invalid-regex",
            TransformPhase::ProviderOutputCanonical,
            "(",
            "lost",
        ))],
        TransformLimits::default(),
    )
    .expect("invalid individual rules remain reviewable");
    let original = "Exact synthetic text that must survive.";

    let result = apply(&pipeline, TransformPhase::ProviderOutputCanonical, original);

    assert_eq!(result.original, original);
    assert_eq!(result.output, original);
    assert!(!result.changed);
    assert_eq!(result.rendering, TransformOutputRendering::NativePlainText);
    assert_eq!(result.reports.len(), 1);
    assert_eq!(result.reports[0].status, TransformRuleStatus::Failed);
    assert!(result.reports[0].trace.error.is_some());
}

#[test]
fn imported_transform_needs_both_enablement_and_exact_approval_and_stays_plain_text() {
    let mut imported = rule(
        "synthetic.imported",
        TransformPhase::DisplayOnly,
        "before",
        "<script>globalThis.pwned=true</script>",
    );
    imported.provenance = provenance(SourceKind::ImportedPackage, Some("synthetic.module"));
    let disabled_set = set(imported.clone());

    let approved_but_disabled = TransformPipeline::compile_with_options(
        &[disabled_set],
        TransformLimits::default(),
        &TransformCompileOptions {
            approved_import_source_ids: BTreeSet::from(["synthetic.module".to_owned()]),
        },
    )
    .expect("disabled imported transform compiles");
    let pending = apply(
        &approved_but_disabled,
        TransformPhase::DisplayOnly,
        "before",
    );
    assert_eq!(pending.output, "before");
    assert_eq!(
        pending.reports[0].status,
        TransformRuleStatus::PendingImportApproval
    );

    imported.imported_enabled = true;
    let enabled_set = set(imported);
    let wrong_approval = TransformPipeline::compile_with_options(
        std::slice::from_ref(&enabled_set),
        TransformLimits::default(),
        &TransformCompileOptions {
            approved_import_source_ids: BTreeSet::from(["another.module".to_owned()]),
        },
    )
    .expect("wrong-source approval compiles as pending");
    assert_eq!(
        apply(&wrong_approval, TransformPhase::DisplayOnly, "before").output,
        "before"
    );

    let approved = TransformPipeline::compile_with_options(
        &[enabled_set],
        TransformLimits::default(),
        &TransformCompileOptions {
            approved_import_source_ids: BTreeSet::from(["synthetic.module".to_owned()]),
        },
    )
    .expect("exact approval");
    let result = apply(&approved, TransformPhase::DisplayOnly, "before");
    assert_eq!(
        result.output, "<script>globalThis.pwned=true</script>",
        "transforms may emit text but never executable markup"
    );
    assert_eq!(result.rendering, TransformOutputRendering::NativePlainText);
    assert_eq!(result.reports[0].status, TransformRuleStatus::Applied);
}

#[test]
fn resolved_prompt_transforms_are_opt_in() {
    let pipeline = TransformPipeline::compile(
        &[set(rule(
            "synthetic.resolved-prompt",
            TransformPhase::ResolvedPrompt,
            "canonical",
            "changed",
        ))],
        TransformLimits::default(),
    )
    .expect("compile");
    let result = apply(&pipeline, TransformPhase::ResolvedPrompt, "canonical");

    assert_eq!(result.output, "canonical");
    assert_eq!(
        result.reports[0].status,
        TransformRuleStatus::ResolvedPromptDisabled
    );
}
