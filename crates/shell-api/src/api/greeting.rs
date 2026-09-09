use lorepia_core::{CoreError, CoreErrorCode};
use serde::{Deserialize, Serialize};

use super::{ShellApi, validate_identifier};
use crate::{ShellError, ShellResult};

/// On-demand, revision-bound reading text. Never accepted as chat-start input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterGreetingDetailDto {
    pub character_id: String,
    pub character_content_revision_id: String,
    pub greeting_id: String,
    pub text: String,
}

impl ShellApi {
    pub fn get_character_greeting_detail(
        &self,
        character_id: &str,
        character_content_revision_id: &str,
        greeting_id: &str,
    ) -> ShellResult<CharacterGreetingDetailDto> {
        validate_identifier("character_id", character_id)?;
        validate_identifier(
            "character_content_revision_id",
            character_content_revision_id,
        )?;
        validate_identifier("greeting_id", greeting_id)?;
        let stored = self
            .core
            .get_character_content(character_id)
            .map_err(ShellError::from)?;
        if stored.revision_id.as_deref() != Some(character_content_revision_id) {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "Stale greeting revision",
                true,
            )
            .into());
        }
        let text = if greeting_id == "default" {
            Some(&stored.value.first_message)
        } else {
            greeting_id
                .strip_prefix("alternate-")
                .and_then(|index| index.parse::<usize>().ok())
                .filter(|index| *index < 128 && format!("alternate-{index}") == greeting_id)
                .and_then(|index| stored.value.alternate_greetings.get(index))
        }
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            ShellError::from(CoreError::new(
                CoreErrorCode::NotFound,
                "Opening not found",
                true,
            ))
        })?;
        if text.len() > 256 * 1024 {
            return Err(CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "Opening exceeds reader limit",
                true,
            )
            .into());
        }
        Ok(CharacterGreetingDetailDto {
            character_id: character_id.to_owned(),
            character_content_revision_id: character_content_revision_id.to_owned(),
            greeting_id: greeting_id.to_owned(),
            text: text.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests;
