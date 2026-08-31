#[cfg(test)]
mod interaction_knowledge_binding_revision_tests {
    use std::collections::{BTreeMap, BTreeSet};

    use lorepia_domain::{InteractionState, KnowledgeEntryId, VariableMap, VersionedJson};
    use lorepia_storage::InteractionKnowledgeBinding;

    use super::{
        ResolvedInteractionPolicy, interaction_knowledge_bindings,
        reconcile_interaction_knowledge_state,
    };

    #[test]
    fn stale_manual_knowledge_binding_becomes_inert_when_entry_is_removed() {
        let entry_id = KnowledgeEntryId::from("shared-entry");
        let state = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: vec![entry_id.clone()],
            proposals: Vec::new(),
            revision: 7,
        };
        let policy = ResolvedInteractionPolicy {
            module_plan_sha256: None,
            rule_sets: Vec::new(),
            rule_set_revisions: Vec::new(),
            knowledge_revisions: BTreeMap::new(),
            asset_action_diagnostics: BTreeMap::<(String, u32), VersionedJson>::new(),
            approved_import_source_ids: BTreeSet::new(),
            variables: VariableMap::default(),
            supported_capabilities: Vec::new(),
            character_name: "Character".to_owned(),
        };
        let existing = [InteractionKnowledgeBinding {
            book_revision_id: "book-old".to_owned(),
            entry_id,
        }];

        let (state, existing) = reconcile_interaction_knowledge_state(state, &policy, &existing)
            .expect("removed knowledge authority must be reconciled");
        let bindings = interaction_knowledge_bindings(&state, &policy, &existing)
            .expect("removed knowledge authority must become inert");
        assert!(state.manually_active_knowledge.is_empty());
        assert!(bindings.is_empty());
    }

    #[test]
    fn stale_manual_knowledge_binding_does_not_rebind_to_a_new_book_revision() {
        let entry_id = KnowledgeEntryId::from("shared-entry");
        let state = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: vec![entry_id.clone()],
            proposals: Vec::new(),
            revision: 7,
        };
        let policy = ResolvedInteractionPolicy {
            module_plan_sha256: None,
            rule_sets: Vec::new(),
            rule_set_revisions: Vec::new(),
            knowledge_revisions: BTreeMap::from([(entry_id.clone(), "book-new".to_owned())]),
            asset_action_diagnostics: BTreeMap::<(String, u32), VersionedJson>::new(),
            approved_import_source_ids: BTreeSet::new(),
            variables: VariableMap::default(),
            supported_capabilities: Vec::new(),
            character_name: "Character".to_owned(),
        };
        let existing = [InteractionKnowledgeBinding {
            book_revision_id: "book-old".to_owned(),
            entry_id,
        }];

        let (state, existing) = reconcile_interaction_knowledge_state(state, &policy, &existing)
            .expect("revision-drifted knowledge authority must be reconciled");
        let bindings = interaction_knowledge_bindings(&state, &policy, &existing)
            .expect("revision-drifted knowledge authority must become inert");
        assert!(state.manually_active_knowledge.is_empty());
        assert!(bindings.is_empty());
    }

    #[test]
    fn exact_manual_knowledge_binding_keeps_its_existing_authority() {
        let entry_id = KnowledgeEntryId::from("shared-entry");
        let state = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: vec![entry_id.clone()],
            proposals: Vec::new(),
            revision: 7,
        };
        let policy = ResolvedInteractionPolicy {
            module_plan_sha256: None,
            rule_sets: Vec::new(),
            rule_set_revisions: Vec::new(),
            knowledge_revisions: BTreeMap::from([(entry_id.clone(), "book-exact".to_owned())]),
            asset_action_diagnostics: BTreeMap::<(String, u32), VersionedJson>::new(),
            approved_import_source_ids: BTreeSet::new(),
            variables: VariableMap::default(),
            supported_capabilities: Vec::new(),
            character_name: "Character".to_owned(),
        };
        let existing = InteractionKnowledgeBinding {
            book_revision_id: "book-exact".to_owned(),
            entry_id,
        };

        let (state, bindings) =
            reconcile_interaction_knowledge_state(state, &policy, std::slice::from_ref(&existing))
                .expect("exact knowledge authority must remain reconciled");
        let bindings = interaction_knowledge_bindings(&state, &policy, &bindings)
            .expect("exact knowledge authority remains valid");
        assert_eq!(bindings, vec![existing]);
    }
}
