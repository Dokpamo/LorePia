use crate::{GenerationPresetDto, ModelRouteDto, ShellApi, ShellError, ShellResult};

impl ShellApi {
    pub fn provider_generation_catalog(
        &self,
    ) -> ShellResult<(Vec<ModelRouteDto>, Vec<GenerationPresetDto>)> {
        self.core
            .provider_generation_catalog()
            .map(|(routes, presets)| {
                (
                    routes.into_iter().map(Into::into).collect(),
                    presets.into_iter().map(Into::into).collect(),
                )
            })
            .map_err(ShellError::from)
    }
}
