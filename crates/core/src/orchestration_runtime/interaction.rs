mod policy;
mod review;
mod state;

pub(in crate::orchestration_runtime) use policy::{
    ResolvedInteractionPolicy, interaction_policy_snapshot,
};
pub use review::{InteractionEventReview, InteractionReviewRequest, InteractionRuleSetRevision};
pub(in crate::orchestration_runtime) use review::{PreparedInteractionReview, interaction_seed};
#[cfg(test)]
pub(in crate::orchestration_runtime) use state::interaction_evaluation_limits;
pub(crate) use state::interaction_state_key;
pub(in crate::orchestration_runtime) use state::{
    initial_interaction_state, interaction_knowledge_bindings,
    reconcile_interaction_knowledge_state, validate_interaction_evaluation_seal,
};

use lorepia_domain::{CoreErrorCode, CoreResult};

use crate::Core;

impl Core {
    /// Produces a read-only deterministic interaction review. It never
    /// initializes or mutates durable state.
    pub fn preview_interaction_event(
        &self,
        request: &InteractionReviewRequest,
    ) -> CoreResult<InteractionEventReview> {
        self.validate_runtime_branch_head(
            &request.conversation_id,
            &request.branch_id,
            request.expected_head.as_ref(),
        )?;
        let (state, knowledge) = match self
            .storage()
            .get_interaction_state_snapshot(&request.conversation_id, &request.branch_id)
        {
            Ok(snapshot) => (snapshot.state, snapshot.knowledge),
            Err(error) if error.code == CoreErrorCode::NotFound => {
                let policy =
                    self.resolve_interaction_policy(&request.conversation_id, &request.branch_id)?;
                (initial_interaction_state(&policy), Vec::new())
            }
            Err(error) => return Err(error),
        };
        Ok(self
            .prepare_interaction_review_from_state(request, state, &knowledge, None, true)?
            .public)
    }
}
