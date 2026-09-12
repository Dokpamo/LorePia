//! Exact module evidence codec for durable generation attempts.

use super::{
    AppliedModuleRuntimePlan, CoreError, CoreResult, EncodedGenerationAttemptAuthorities,
    GenerationAttemptInput, ModuleMergeReview, corrupted, decode_hashed, documents, encode_hashed,
};

pub(super) fn encode_generation_attempt_authorities(
    input: &GenerationAttemptInput,
) -> CoreResult<EncodedGenerationAttemptAuthorities> {
    let prompt_selection = input.prompt_selection_authority.as_ref().ok_or_else(|| {
        CoreError::invalid("generation attempt prompt selection authority is missing")
    })?;
    let (prompt_selection_json, prompt_selection_sha256) =
        encode_hashed("generation prompt selection authority", prompt_selection)?;
    let module_runtime_review =
        input
            .module_runtime_review_authority
            .as_ref()
            .ok_or_else(|| {
                CoreError::invalid("generation attempt module runtime authority is missing")
            })?;
    let (module_runtime_review_json, module_runtime_review_sha256) =
        documents::encode_hashed(module_runtime_review)?;
    let (applied_runtime_plan_json, applied_runtime_plan_sha256) = input
        .applied_runtime_plan_authority
        .as_ref()
        .map(documents::encode_hashed)
        .transpose()?
        .map_or((None, None), |(json, sha256)| (Some(json), Some(sha256)));
    Ok(EncodedGenerationAttemptAuthorities {
        prompt_selection_json,
        prompt_selection_sha256,
        module_runtime_review_json,
        module_runtime_review_sha256,
        applied_runtime_plan_json,
        applied_runtime_plan_sha256,
    })
}

pub(super) fn decode_module_runtime_authority(
    review_json: Option<&str>,
    review_sha256: Option<&str>,
    plan_json: Option<&str>,
    plan_sha256: Option<&str>,
    authority_version: i64,
) -> CoreResult<(Option<ModuleMergeReview>, Option<AppliedModuleRuntimePlan>)> {
    match (
        review_json,
        review_sha256,
        plan_json,
        plan_sha256,
        authority_version,
    ) {
        (None, None, None, None, 0) => Ok((None, None)),
        (Some(review_json), Some(review_sha256), plan_json, plan_sha256, 1) => {
            let review = decode_hashed::<ModuleMergeReview>(
                "generation module runtime review authority",
                Some(review_json),
                Some(review_sha256),
            )?
            .ok_or_else(|| corrupted("generation module runtime review authority is missing"))?
            .0;
            review.verify().map_err(|error| {
                corrupted(format!(
                    "generation module runtime review authority is invalid: {error}"
                ))
            })?;
            if serde_json::to_string(&review).map_err(|error| {
                corrupted(format!(
                    "generation module runtime review authority cannot be canonicalized: {error}"
                ))
            })? != review_json
            {
                return Err(corrupted(
                    "generation module runtime review authority JSON is not canonical",
                ));
            }
            let plan = match (plan_json, plan_sha256) {
                (None, None) => None,
                (Some(plan_json), Some(plan_sha256)) => {
                    let plan = decode_hashed::<AppliedModuleRuntimePlan>(
                        "generation applied runtime plan authority",
                        Some(plan_json),
                        Some(plan_sha256),
                    )?
                    .ok_or_else(|| {
                        corrupted("generation applied runtime plan authority is missing")
                    })?
                    .0;
                    plan.verify().map_err(|error| {
                        corrupted(format!(
                            "generation applied runtime plan authority is invalid: {error}"
                        ))
                    })?;
                    if serde_json::to_string(&plan).map_err(|error| {
                        corrupted(format!(
                            "generation applied runtime plan authority cannot be canonicalized: {error}"
                        ))
                    })? != plan_json
                    {
                        return Err(corrupted(
                            "generation applied runtime plan authority JSON is not canonical",
                        ));
                    }
                    Some(plan)
                }
                _ => {
                    return Err(corrupted(
                        "generation applied runtime plan authority columns are incomplete",
                    ));
                }
            };
            if plan.as_ref().is_some_and(|plan| plan.review != review) {
                return Err(corrupted(
                    "generation applied runtime plan authority differs from its review",
                ));
            }
            Ok((Some(review), plan))
        }
        _ => Err(corrupted(
            "generation module runtime authority columns are incomplete",
        )),
    }
}
