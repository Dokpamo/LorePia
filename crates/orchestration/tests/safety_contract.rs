use std::collections::BTreeSet;

use lorepia_domain::{
    ConditionExpr, ContentModuleId, InteractionAction, InteractionEvent, InteractionRule,
    InteractionRuleId, InteractionRuleSet, InteractionRuleSetId, InteractionState, SafeTemplate,
    SourceKind, TemplatePart, ValueExpr, VariableBinding, VariableId, VariableMap, VariableRef,
    VariableScope, VariableValue,
};
use lorepia_orchestration::{
    InteractionCompileOptions, InteractionContext, InteractionEngine, InteractionLimits,
    InteractionRuleStatus, TemplateEnvironment, TemplateError, render_safe_template,
};

fn variable(namespace: Option<&str>, id: &str) -> VariableRef {
    VariableRef {
        scope: namespace.map_or(VariableScope::Conversation, |_| VariableScope::Module),
        namespace: namespace.map(ContentModuleId::from),
        id: VariableId::from(id),
    }
}

fn imported_rule_set(target: VariableRef) -> InteractionRuleSet {
    InteractionRuleSet {
        id: InteractionRuleSetId::from("synthetic.rules"),
        name: "Synthetic imported rules".to_owned(),
        schema_version: 1,
        rules: vec![InteractionRule {
            id: InteractionRuleId::from("synthetic.set-state"),
            name: "Set state".to_owned(),
            enabled: true,
            imported_author_enabled: true,
            event: InteractionEvent::ConversationStarted,
            condition: None,
            actions: vec![InteractionAction::SetVariable {
                target,
                value: ValueExpr::Literal {
                    value: VariableValue::Integer(7),
                },
            }],
            priority: 0,
            stop_after_match: false,
            provenance: lorepia_domain::Provenance {
                source_kind: SourceKind::ImportedPackage,
                source_id: Some("synthetic.module".to_owned()),
                source_hash: Some("synthetic-hash".to_owned()),
                author: Some("Synthetic Author".to_owned()),
                license: Some("MIT".to_owned()),
                imported_at: None,
            },
        }],
        max_actions_per_event: 16,
        provenance: lorepia_domain::Provenance {
            source_kind: SourceKind::ImportedPackage,
            source_id: Some("synthetic.module".to_owned()),
            source_hash: Some("synthetic-hash".to_owned()),
            author: Some("Synthetic Author".to_owned()),
            license: Some("MIT".to_owned()),
            imported_at: None,
        },
    }
}

#[test]
fn templates_are_read_only_bounded_value_insertion() {
    let mood = variable(None, "mood");
    let tags = variable(None, "tags");
    let payload = "<script>globalThis.pwned=true</script> $(touch /tmp/never)";
    let variables = VariableMap {
        values: vec![
            VariableBinding {
                variable: mood.clone(),
                value: VariableValue::Text(payload.to_owned()),
            },
            VariableBinding {
                variable: tags.clone(),
                value: VariableValue::StringList(vec!["calm".to_owned(), "safe".to_owned()]),
            },
        ],
    };
    let before = variables.clone();
    let template = SafeTemplate {
        parts: vec![
            TemplatePart::Text {
                value: "mood=".to_owned(),
            },
            TemplatePart::Variable {
                variable: mood.clone(),
            },
            TemplatePart::Text {
                value: "; tags=".to_owned(),
            },
            TemplatePart::Join {
                variable: tags,
                separator: "|".to_owned(),
            },
            TemplatePart::Conditional {
                condition: ConditionExpr::Contains {
                    variable: mood,
                    value: "script".to_owned(),
                },
                then_template: Box::new(SafeTemplate {
                    parts: vec![TemplatePart::Text {
                        value: "; treated-as-text".to_owned(),
                    }],
                    max_output_chars: 64,
                }),
                else_template: None,
            },
        ],
        max_output_chars: 256,
    };
    let environment = TemplateEnvironment {
        variables: &variables,
        capabilities: &[],
        character_name: "Synthetic Character",
        user_name: "Synthetic User",
        persona_name: None,
        persona_description: None,
        current_date: "2026-08-03",
        current_time: "12:00",
        slots: &[],
    };

    let rendered = render_safe_template(&template, &environment).expect("safe render");

    assert_eq!(
        rendered,
        format!("mood={payload}; tags=calm|safe; treated-as-text")
    );
    assert_eq!(
        variables, before,
        "template evaluation must not mutate state"
    );

    let too_small = SafeTemplate {
        max_output_chars: 4,
        ..template
    };
    assert_eq!(
        render_safe_template(&too_small, &environment),
        Err(TemplateError::OutputLimit { limit: 4 })
    );
}

#[test]
fn imported_interactions_are_inert_until_exact_source_approval() {
    let score = variable(Some("synthetic.module"), "score");
    let set = imported_rule_set(score.clone());
    let initial = InteractionState {
        variables: VariableMap::default(),
        manually_active_knowledge: Vec::new(),
        proposals: Vec::new(),
        revision: 0,
    };
    let event = InteractionEvent::ConversationStarted;

    let pending =
        InteractionEngine::compile(std::slice::from_ref(&set), InteractionLimits::default())
            .expect("unapproved imported rules compile as inert")
            .handle_event(&initial, &event, &InteractionContext::default())
            .expect("pending rules are safe to inspect");

    assert_eq!(pending.state, initial);
    assert!(pending.effects.is_empty());
    assert_eq!(
        pending.trace[0].status,
        InteractionRuleStatus::PendingImportApproval
    );

    let wrong_source = InteractionEngine::compile_with_options(
        std::slice::from_ref(&set),
        InteractionLimits::default(),
        &InteractionCompileOptions {
            approved_import_source_ids: BTreeSet::from(["another.module".to_owned()]),
        },
    )
    .expect("wrong approval stays inert")
    .handle_event(&initial, &event, &InteractionContext::default())
    .expect("wrong approval is safe");
    assert_eq!(
        wrong_source.trace[0].status,
        InteractionRuleStatus::PendingImportApproval
    );

    let approved = InteractionEngine::compile_with_options(
        &[set],
        InteractionLimits::default(),
        &InteractionCompileOptions {
            approved_import_source_ids: BTreeSet::from(["synthetic.module".to_owned()]),
        },
    )
    .expect("exact approval compiles")
    .handle_event(&initial, &event, &InteractionContext::default())
    .expect("approved declarative action executes");
    assert_eq!(
        approved.state.variables.get(&score),
        Some(&VariableValue::Integer(7))
    );
    assert_eq!(approved.trace[0].status, InteractionRuleStatus::Applied);
}

#[test]
fn executable_or_html_interaction_variants_do_not_deserialize() {
    for forbidden in [
        r#"{"kind":"execute_javascript","source":"fetch('https://example.invalid')"}"#,
        r#"{"kind":"read_file","path":"/etc/passwd"}"#,
        r#"{"kind":"append_html","html":"<button onclick='pwn()'>go</button>"}"#,
        r#"{"kind":"shell","command":"echo unsafe"}"#,
    ] {
        assert!(
            serde_json::from_str::<InteractionAction>(forbidden).is_err(),
            "forbidden action unexpectedly deserialized: {forbidden}"
        );
    }
}
