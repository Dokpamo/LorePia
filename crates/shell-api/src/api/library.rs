use lorepia_core::InspectionId;

use crate::{
    CharacterDto, CharacterGreetingCatalogDto, CharacterRenderProfileDto, ImportInspectionDto,
    ShellError, ShellResult, StagedImportFile,
};

use super::{ShellApi, validate_identifier};

impl ShellApi {
    pub fn list_characters(&self) -> ShellResult<Vec<CharacterDto>> {
        self.core
            .list_characters()
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn get_character(&self, character_id: &str) -> ShellResult<CharacterDto> {
        validate_identifier("character_id", character_id)?;
        self.core
            .get_character(character_id)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn get_character_render_profile(
        &self,
        character_id: &str,
    ) -> ShellResult<CharacterRenderProfileDto> {
        validate_identifier("character_id", character_id)?;
        self.core
            .get_character_content(character_id)
            .map(|stored| {
                CharacterRenderProfileDto::from_content(
                    character_id.to_owned(),
                    stored.revision_id,
                    stored.value,
                )
            })
            .map_err(ShellError::from)
    }

    pub fn get_character_greeting_catalog(
        &self,
        character_id: &str,
    ) -> ShellResult<CharacterGreetingCatalogDto> {
        validate_identifier("character_id", character_id)?;
        self.core
            .get_character_greeting_catalog(character_id)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn inspect_import(
        &self,
        staged_file: &StagedImportFile,
    ) -> ShellResult<ImportInspectionDto> {
        self.core
            .inspect_import(staged_file.as_path())
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn commit_import(&self, inspection_id: &str) -> ShellResult<CharacterDto> {
        validate_identifier("inspection_id", inspection_id)?;
        self.core
            .commit_import(&InspectionId(inspection_id.to_owned()))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn discard_import(&self, inspection_id: &str) -> ShellResult<()> {
        validate_identifier("inspection_id", inspection_id)?;
        self.core
            .discard_import(&InspectionId(inspection_id.to_owned()))
            .map_err(ShellError::from)
    }
}
