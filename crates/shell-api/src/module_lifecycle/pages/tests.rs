use super::*;
use crate::module_lifecycle::{
    ReviewContentModuleActivationInput,
    tests::{activation, module, shell_and_target},
};
use lorepia_core::{ModuleActivationApproval, ModuleMergeResolutionSet, PromptBlockId};

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one exact paginated review must survive resolution, forged approval rejection and response-loss replay"
)]
fn large_module_pages_apply_complete_authority_and_replay() {
    let (_root, shell, target) = shell_and_target();
    let mut document = module("paged-module", "1.0", "a", Some("Synthetic instruction"));
    let prototype = document.prompt_fragments.remove(0);
    document.prompt_fragments = (0..513)
        .map(|i| {
            let mut block = prototype.clone();
            block.id = PromptBlockId::from(format!("block-{i:04}"));
            block
        })
        .collect();
    shell.core.upsert_content_module(&document, None).unwrap();
    let request = activation("paged-module", "paged-binding", &target);
    assert!(
        shell
            .review_content_module_activation(ReviewContentModuleActivationInput {
                activation: request.clone()
            })
            .is_err()
    );
    let page = shell
        .review_content_module_activation_page(ReviewContentModuleActivationPageInput {
            activation: request.clone(),
            offset: 0,
            expected_review_sha256: None,
        })
        .unwrap();
    assert_eq!(page.component_count, 513);
    assert_eq!(page.components.len(), 64);
    assert_eq!(page.next_offset, Some(64));
    assert!(serde_json::to_vec(&page).unwrap().len() < super::super::MAX_LIFECYCLE_DOCUMENT_BYTES);
    let mut components = page.components.clone();
    let mut next = page.next_offset;
    while let Some(offset) = next {
        let part = shell
            .review_content_module_activation_page(ReviewContentModuleActivationPageInput {
                activation: request.clone(),
                offset,
                expected_review_sha256: Some(page.review_sha256.clone()),
            })
            .unwrap();
        assert_eq!(part.review_sha256, page.review_sha256);
        assert_eq!(part.component_count, 513);
        components.extend(part.components);
        next = part.next_offset;
    }
    let complete = shell
        .core
        .review_content_module_activation(&request)
        .unwrap();
    assert_eq!(components, complete.components);
    for (offset, expected_review_sha256) in [
        (64, None),
        (513, Some(page.review_sha256.clone())),
        (64, Some(Sha256Digest::parse("b".repeat(64)).unwrap())),
    ] {
        assert!(
            shell
                .review_content_module_activation_page(ReviewContentModuleActivationPageInput {
                    activation: request.clone(),
                    offset,
                    expected_review_sha256,
                })
                .is_err()
        );
    }
    let resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: page.review_sha256.clone(),
        resolutions: vec![],
    };
    let plan = shell
        .resolve_content_module_activation_summary(ResolveContentModuleActivationInput {
            activation: request.clone(),
            resolutions: resolutions.clone(),
        })
        .unwrap();
    assert_eq!(plan.component_count, 513);
    let input = ActivateContentModuleInput {
        activation: request.clone(),
        resolutions,
        approval: ModuleActivationApproval {
            approval_id: "page-approval".into(),
            expected_review_sha256: page.review_sha256.clone(),
            expected_plan_sha256: plan.plan_sha256.clone(),
        },
    };
    let mut forged = input.clone();
    forged.approval.expected_plan_sha256 = Sha256Digest::parse("c".repeat(64)).unwrap();
    assert!(shell.activate_content_module_summary(forged).is_err());
    assert!(
        shell
            .core
            .list_content_module_bindings(&document.id)
            .unwrap()
            .is_empty()
    );
    let receipt = shell
        .activate_content_module_summary(input.clone())
        .unwrap();
    assert!(receipt.verified);
    assert_eq!(receipt.plan.component_count, 513);
    assert_eq!(receipt.plan.plan_sha256, plan.plan_sha256);
    assert_eq!(
        shell
            .activate_content_module_summary(input.clone())
            .unwrap(),
        receipt
    );
    let core = shell.core.clone();
    assert_eq!(
        core.list_content_module_bindings(&document.id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn paged_review_keeps_conflicts_explicit_and_rejects_stale_pages() {
    let (_root, shell, target) = shell_and_target();
    let first = module("page-first", "1.0", "a", Some("First"));
    let second = module("page-second", "1.0", "b", Some("Second"));
    shell.core.upsert_content_module(&first, None).unwrap();
    shell.core.upsert_content_module(&second, None).unwrap();
    let first_request = activation("page-first", "first-binding", &target);
    let first_page = shell
        .review_content_module_activation_page(ReviewContentModuleActivationPageInput {
            activation: first_request.clone(),
            offset: 0,
            expected_review_sha256: None,
        })
        .unwrap();
    let resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: first_page.review_sha256.clone(),
        resolutions: vec![],
    };
    let plan = shell
        .resolve_content_module_activation_summary(ResolveContentModuleActivationInput {
            activation: first_request.clone(),
            resolutions: resolutions.clone(),
        })
        .unwrap();
    shell
        .activate_content_module_summary(ActivateContentModuleInput {
            activation: first_request,
            resolutions,
            approval: ModuleActivationApproval {
                approval_id: "first-approval".into(),
                expected_review_sha256: plan.review_sha256,
                expected_plan_sha256: plan.plan_sha256,
            },
        })
        .unwrap();
    let second_request = activation("page-second", "second-binding", &target);
    let page = shell
        .review_content_module_activation_page(ReviewContentModuleActivationPageInput {
            activation: second_request.clone(),
            offset: 0,
            expected_review_sha256: None,
        })
        .unwrap();
    assert_eq!(page.conflicts.len(), 1);
    assert!(
        shell
            .resolve_content_module_activation_summary(ResolveContentModuleActivationInput {
                activation: second_request.clone(),
                resolutions: ModuleMergeResolutionSet {
                    expected_review_sha256: page.review_sha256.clone(),
                    resolutions: vec![]
                },
            })
            .is_err()
    );
    let mut changed = second_request;
    changed.binding.priority = 4;
    assert!(
        shell
            .review_content_module_activation_page(ReviewContentModuleActivationPageInput {
                activation: changed,
                offset: 0,
                expected_review_sha256: Some(page.review_sha256),
            })
            .is_err()
    );
}
