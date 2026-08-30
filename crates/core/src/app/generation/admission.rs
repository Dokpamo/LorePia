use std::sync::Arc;

use lorepia_chat::ChatEvent;
use lorepia_domain::{
    CoreResult, GenerationId, GenerationRecord, GenerationRequest, Message, ModelRouteId,
};
use lorepia_providers::Provider;
use lorepia_storage::Storage;
use tokio::sync::{broadcast, watch};

use super::types::GenerationActionTargetIdentity;
use crate::app::{
    Core, GenerationCredential, GenerationProviderAdmissionKey, GenerationRegistry, GenerationTask,
    GenerationTransformContext,
};

// Admission belongs to Core rather than renderer stream registrations so a
// detached or failed consumer cannot recycle a slot while provider work keeps
// running. The per-conversation allowance preserves bounded background branch
// generation while preventing one conversation from consuming the process.
pub(in crate::app) const MAX_ACTIVE_GENERATIONS_PER_PROCESS: usize = 32;
pub(in crate::app) const MAX_ACTIVE_GENERATIONS_PER_PROVIDER: usize = 8;
pub(in crate::app) const MAX_ACTIVE_GENERATIONS_PER_CONVERSATION: usize = 4;

pub(in crate::app) struct GenerationLaunchPermit {
    generation_id: GenerationId,
    active_generations: Arc<GenerationRegistry>,
    cancel_receiver: Option<watch::Receiver<bool>>,
    preserve_partial: bool,
}

impl GenerationLaunchPermit {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn into_task(
        mut self,
        storage: Arc<Storage>,
        event_bus: broadcast::Sender<ChatEvent>,
        branch_id: lorepia_domain::ConversationBranchId,
        request: GenerationRequest,
        assistant: Message,
        provider: Arc<dyn Provider>,
        credential: GenerationCredential,
        transforms: GenerationTransformContext,
    ) -> CoreResult<GenerationTask> {
        self.active_generations.activate(&self.generation_id)?;
        let cancel_receiver = self
            .cancel_receiver
            .take()
            .expect("generation launch permit can be consumed only once");
        Ok(GenerationTask {
            storage,
            active_generations: Arc::clone(&self.active_generations),
            event_bus,
            branch_id,
            request,
            assistant,
            provider,
            credential,
            cancel_receiver,
            preserve_partial: self.preserve_partial,
            transforms,
        })
    }
}

impl Drop for GenerationLaunchPermit {
    fn drop(&mut self) {
        if self.cancel_receiver.is_some() {
            self.active_generations.remove(&self.generation_id);
        }
    }
}

impl Core {
    pub(in crate::app) fn prepare_generation_launch(
        &self,
        generation: &GenerationRecord,
        provider_admission_key: GenerationProviderAdmissionKey,
    ) -> CoreResult<GenerationLaunchPermit> {
        let preserve_partial = self
            .inner
            .storage
            .load_settings()?
            .preserve_partial_generations;
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        self.inner.active_generations.register(
            generation,
            provider_admission_key,
            cancel_sender,
        )?;
        Ok(GenerationLaunchPermit {
            generation_id: generation.id.clone(),
            active_generations: Arc::clone(&self.inner.active_generations),
            cancel_receiver: Some(cancel_receiver),
            preserve_partial,
        })
    }

    pub(in crate::app) fn generation_provider_admission_key(
        &self,
        target: &GenerationActionTargetIdentity,
    ) -> CoreResult<GenerationProviderAdmissionKey> {
        match target {
            GenerationActionTargetIdentity::GenerationTarget { model_route_id, .. } => {
                self.generation_provider_admission_key_for_model_route(model_route_id)
            }
            GenerationActionTargetIdentity::ProviderProfile {
                provider_profile_id,
            } => Ok(GenerationProviderAdmissionKey::ProviderProfile(
                provider_profile_id.clone(),
            )),
            #[cfg(test)]
            GenerationActionTargetIdentity::DirectModel { model_sha256 } => Ok(
                GenerationProviderAdmissionKey::DirectModel(model_sha256.clone()),
            ),
        }
    }

    pub(in crate::app) fn prepare_generation_launch_for_target(
        &self,
        generation: &GenerationRecord,
        target: &GenerationActionTargetIdentity,
    ) -> CoreResult<GenerationLaunchPermit> {
        let provider_admission_key = self.generation_provider_admission_key(target)?;
        self.prepare_generation_launch(generation, provider_admission_key)
    }

    pub(in crate::app) fn generation_provider_admission_key_for_model_route(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<GenerationProviderAdmissionKey> {
        Ok(GenerationProviderAdmissionKey::Connection(
            self.inner
                .storage
                .get_model_route(model_route_id)?
                .connection_id,
        ))
    }

    pub(in crate::app) fn active_generation_count(&self) -> usize {
        self.inner.active_generations.len()
    }
}
