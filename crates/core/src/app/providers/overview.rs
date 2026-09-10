use lorepia_domain::{CoreResult, GenerationPreset, ModelRoute};

use crate::app::Core;

impl Core {
    pub fn provider_generation_catalog(
        &self,
    ) -> CoreResult<(Vec<ModelRoute>, Vec<GenerationPreset>)> {
        self.inner.storage.provider_generation_catalog()
    }
}
