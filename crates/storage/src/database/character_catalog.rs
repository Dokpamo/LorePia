use super::{
    Character, CharacterGreetingCatalog, CharacterGreetingKind, CharacterGreetingOption,
    Connection, CoreError, CoreErrorCode, CoreResult, OptionalExtension, Storage, invalid_enum,
    params, parse_datetime_sql, storage_corrupted, storage_db_error,
};

impl Storage {
    pub fn list_characters(&self) -> CoreResult<Vec<Character>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, description, source_hash, avatar_asset_hash, created_at
                 FROM characters ORDER BY name COLLATE NOCASE, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], map_character)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn get_character(&self, id: &str) -> CoreResult<Character> {
        self.connection()?
            .query_row(
                "SELECT id, name, description, source_hash, avatar_asset_hash, created_at
                 FROM characters WHERE id = ?1",
                [id],
                map_character,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "character was not found", false)
            })
    }

    /// Returns only selector metadata for the exact active character-content
    /// revision. Greeting source text never crosses this read boundary.
    pub fn character_greeting_catalog(
        &self,
        character_id: &str,
    ) -> CoreResult<CharacterGreetingCatalog> {
        let connection = self.connection()?;
        let revision_id = active_character_content_revision(&connection, character_id)?;
        let greetings = if let Some(revision_id) = revision_id.as_deref() {
            let mut statement = connection
                .prepare(
                    "SELECT greeting_id, kind, enabled
                     FROM character_greetings
                     WHERE character_content_revision_id = ?1
                     ORDER BY ordinal, greeting_id",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map([revision_id], |row| {
                    let kind = row.get::<_, String>(1)?;
                    let enabled = row.get::<_, i64>(2)?;
                    Ok(CharacterGreetingOption {
                        id: row.get(0)?,
                        kind: str_to_character_greeting_kind(&kind, 1)?,
                        enabled: match enabled {
                            0 => false,
                            1 => true,
                            other => {
                                return Err(invalid_enum(
                                    2,
                                    "character greeting enabled flag",
                                    &other.to_string(),
                                ));
                            }
                        },
                    })
                })
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        } else {
            Vec::new()
        };
        Ok(CharacterGreetingCatalog {
            character_id: character_id.to_owned(),
            character_content_revision_id: revision_id,
            greetings,
        })
    }
}
fn map_character(row: &rusqlite::Row<'_>) -> rusqlite::Result<Character> {
    Ok(Character {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        source_hash: row.get(3)?,
        avatar_asset_hash: row.get(4)?,
        created_at: parse_datetime_sql(row.get::<_, String>(5)?, 5)?,
    })
}

pub(super) fn active_character_content_revision(
    connection: &Connection,
    character_id: &str,
) -> CoreResult<Option<String>> {
    let revision_id = connection
        .query_row(
            "SELECT state.active_revision_id
             FROM character_content AS content
             JOIN content_objects AS object
               ON object.id = content.object_id
              AND object.object_kind = 'character_content'
              AND object.deleted_at IS NULL
             JOIN content_object_state AS state
               ON state.object_id = content.object_id
             JOIN character_content_revisions AS revision
               ON revision.object_id = content.object_id
              AND revision.revision_id = state.active_revision_id
             WHERE content.character_id = ?1",
            [character_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if revision_id.is_some() {
        return Ok(revision_id);
    }
    let character_exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM characters WHERE id = ?1)",
            [character_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if character_exists {
        Ok(None)
    } else {
        Err(CoreError::new(
            CoreErrorCode::NotFound,
            "character was not found",
            false,
        ))
    }
}

pub(super) fn resolve_character_greeting(
    transaction: &rusqlite::Transaction<'_>,
    character_content_revision_id: Option<&str>,
    greeting_id: Option<&str>,
) -> CoreResult<Option<(String, String)>> {
    let Some(revision_id) = character_content_revision_id else {
        return if greeting_id.is_some() {
            Err(unavailable_character_greeting_error())
        } else {
            Ok(None)
        };
    };
    let selected = if let Some(greeting_id) = greeting_id {
        transaction
            .query_row(
                "SELECT greeting_id, content, enabled
                 FROM character_greetings
                 WHERE character_content_revision_id = ?1
                   AND greeting_id = ?2",
                params![revision_id, greeting_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
    } else {
        transaction
            .query_row(
                "SELECT greeting_id, content, enabled
                 FROM character_greetings
                 WHERE character_content_revision_id = ?1
                   AND kind = 'default'
                   AND enabled = 1
                 ORDER BY ordinal, greeting_id
                 LIMIT 1",
                [revision_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
    };
    match selected {
        Some((id, content, 1)) => Ok(Some((id, content))),
        Some(_) | None if greeting_id.is_some() => Err(unavailable_character_greeting_error()),
        Some(_) => Err(storage_corrupted(
            "enabled default character greeting projection is invalid",
        )),
        None => Ok(None),
    }
}

pub(super) fn validate_character_greeting_id(greeting_id: &str) -> CoreResult<()> {
    if greeting_id.is_empty()
        || greeting_id.len() > 256
        || greeting_id.trim() != greeting_id
        || greeting_id.chars().any(char::is_control)
    {
        Err(CoreError::invalid("character greeting id is invalid"))
    } else {
        Ok(())
    }
}

pub(super) fn stale_character_greeting_catalog_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        "character greeting catalog is stale; refresh before creating the conversation",
        true,
    )
}

fn unavailable_character_greeting_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        "character greeting is unavailable for the exact content revision",
        true,
    )
}

fn str_to_character_greeting_kind(
    value: &str,
    column: usize,
) -> rusqlite::Result<CharacterGreetingKind> {
    match value {
        "default" => Ok(CharacterGreetingKind::Default),
        "alternate" => Ok(CharacterGreetingKind::Alternate),
        other => Err(invalid_enum(column, "character greeting kind", other)),
    }
}
