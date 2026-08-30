//! Transaction-preserving package and imported-character commit bridge.

use super::{
    AssetDescriptor, AssetRole, CharacterContentV1, ContentModule, CoreError, CoreResult,
    DocumentTable, InteractionRuleSet, KnowledgeBook, MemoryProfile, OptionalExtension,
    PackageCommitDocument, PackageDocumentWrite, PromptPreset, Provenance, RevisionEventKind,
    Serialize, SourceKind, Transaction, TransformSet, Utc, ValidateOrchestration,
    append_content_revision, decode_document, encode_document, params, storage_db_error,
    validate_identifier, validate_optional_sha256, write_content_module_projection,
    write_interaction_rule_set_projection, write_knowledge_book_projection,
    write_memory_profile_projection, write_prompt_preset_projection,
    write_transform_set_projection,
};
use serde::de::DeserializeOwned;

pub(super) fn character_content_object_id(character_id: &str) -> String {
    format!("character-content:{character_id}")
}

pub(super) fn character_content_metadata_json(
    content: &CharacterContentV1,
    inspection_plan_sha256: Option<&str>,
) -> CoreResult<String> {
    // Asset descriptors are already projected into `asset_descriptors` and
    // `asset_links`, while the immutable canonical document retains the full
    // list. Keep this bounded metadata projection compact instead of storing a
    // third copy that grows linearly with archive size.
    let knowledge_book = content.knowledge_book.as_ref().map(|book| {
        serde_json::json!({
            "id": book.id,
            "name": book.name,
            "source_sha256": book.source_sha256,
        })
    });
    serde_json::to_string(&serde_json::json!({
        "schema_version": content.schema_version,
        "knowledge_book": knowledge_book,
        "asset_count": content.assets.len(),
        "inspection_plan_sha256": inspection_plan_sha256,
    }))
    .map_err(|error| CoreError::invalid(format!("cannot encode character metadata: {error}")))
}

fn write_character_content_header(
    transaction: &Transaction<'_>,
    object_id: &str,
    character_id: &str,
    revision_id: &str,
    content: &CharacterContentV1,
    document_json: &str,
    inspection_plan_sha256: Option<&str>,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT OR IGNORE INTO character_content (object_id, character_id)
             VALUES (?1, ?2)",
            params![object_id, character_id],
        )
        .map_err(storage_db_error)?;
    let unknown_extensions_json =
        serde_json::to_string(&content.unknown_extensions).map_err(|error| {
            CoreError::invalid(format!("cannot encode character extensions: {error}"))
        })?;
    let metadata_json = character_content_metadata_json(content, inspection_plan_sha256)?;
    transaction
        .execute(
            "INSERT INTO character_content_revisions
             (revision_id, object_id, personality, scenario, first_message,
              system_instruction, post_history_instruction, creator_notes,
              unknown_extensions_json, metadata_json, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '', ?8, ?9, ?10)",
            params![
                revision_id,
                object_id,
                content.personality,
                content.scenario,
                content.first_message,
                content.system_instruction,
                content.post_history_instruction,
                unknown_extensions_json,
                metadata_json,
                document_json,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn write_character_greetings(
    transaction: &Transaction<'_>,
    revision_id: &str,
    content: &CharacterContentV1,
) -> CoreResult<()> {
    let mut greeting_ordinal = 0_i64;
    if !content.first_message.is_empty() {
        transaction
            .execute(
                "INSERT INTO character_greetings
                 (character_content_revision_id, ordinal, greeting_id, kind,
                  content, enabled, payload_json)
                 VALUES (?1, ?2, 'default', 'default', ?3, 1, ?4)",
                params![
                    revision_id,
                    greeting_ordinal,
                    content.first_message,
                    serde_json::json!({"content": content.first_message}).to_string(),
                ],
            )
            .map_err(storage_db_error)?;
        greeting_ordinal += 1;
    }
    for (index, greeting) in content.alternate_greetings.iter().enumerate() {
        if greeting.is_empty() {
            continue;
        }
        transaction
            .execute(
                "INSERT INTO character_greetings
                 (character_content_revision_id, ordinal, greeting_id, kind,
                  content, enabled, payload_json)
                 VALUES (?1, ?2, ?3, 'alternate', ?4, 1, ?5)",
                params![
                    revision_id,
                    greeting_ordinal,
                    format!("alternate-{index}"),
                    greeting,
                    serde_json::json!({"content": greeting}).to_string(),
                ],
            )
            .map_err(storage_db_error)?;
        greeting_ordinal += 1;
    }
    Ok(())
}

fn write_character_dialogue_examples(
    transaction: &Transaction<'_>,
    revision_id: &str,
    content: &CharacterContentV1,
) -> CoreResult<()> {
    for (index, example) in content.example_dialogs.iter().enumerate() {
        if example.is_empty() {
            continue;
        }
        let example_id = format!("example-{index}");
        transaction
            .execute(
                "INSERT INTO character_dialogue_examples
                 (character_content_revision_id, example_id, ordinal, name, payload_json)
                 VALUES (?1, ?2, ?3, '', ?4)",
                params![
                    revision_id,
                    example_id,
                    i64::try_from(index)
                        .map_err(|_| CoreError::invalid("too many dialogue examples"))?,
                    serde_json::json!({"content": example}).to_string(),
                ],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT INTO character_dialogue_example_messages
                 (character_content_revision_id, example_id, ordinal, role, content)
                 VALUES (?1, ?2, 0, 'assistant', ?3)",
                params![revision_id, example_id, example],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_character_assets(
    transaction: &Transaction<'_>,
    revision_id: &str,
    content: &CharacterContentV1,
) -> CoreResult<()> {
    for (ordinal, descriptor) in content.assets.iter().enumerate() {
        write_asset_descriptor(transaction, descriptor, Some(revision_id))?;
        transaction
            .execute(
                "INSERT INTO asset_links
                 (owner_revision_id, asset_descriptor_id, role, ordinal, payload_json)
                 VALUES (?1, ?2, ?3, ?4, '{}')",
                params![
                    revision_id,
                    descriptor.id.as_str(),
                    asset_role_str(descriptor.role),
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many character assets"))?,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

pub(super) fn write_character_content_projection(
    transaction: &Transaction<'_>,
    object_id: &str,
    character_id: &str,
    revision_id: &str,
    content: &CharacterContentV1,
    document_json: &str,
    inspection_plan_sha256: Option<&str>,
) -> CoreResult<()> {
    write_character_content_header(
        transaction,
        object_id,
        character_id,
        revision_id,
        content,
        document_json,
        inspection_plan_sha256,
    )?;
    write_character_greetings(transaction, revision_id, content)?;
    write_character_dialogue_examples(transaction, revision_id, content)?;
    write_character_assets(transaction, revision_id, content)
}

pub(crate) fn write_imported_character_content(
    transaction: &Transaction<'_>,
    character_id: &str,
    source_hash: &str,
    content: &CharacterContentV1,
    inspection_plan_sha256: &str,
) -> CoreResult<()> {
    validate_optional_sha256("character source hash", Some(source_hash))?;
    validate_optional_sha256(
        "character inspection plan hash",
        Some(inspection_plan_sha256),
    )?;
    if content
        .unknown_extensions
        .raw_source_sha256
        .as_ref()
        .is_some_and(|digest| digest.as_str() != source_hash)
    {
        return Err(CoreError::invalid(
            "character extension index is not bound to the imported source",
        ));
    }
    let object_id = character_content_object_id(character_id);
    let provenance = Provenance {
        source_kind: SourceKind::ImportedStandard,
        source_id: Some(character_id.to_owned()),
        source_hash: Some(source_hash.to_owned()),
        author: None,
        license: None,
        imported_at: Some(Utc::now()),
    };
    if let Some(embedded) = content
        .knowledge_book
        .as_ref()
        .and_then(|reference| reference.embedded.as_ref())
    {
        let book = embedded.materialize(provenance.clone());
        // An empty inline reference is only a future link. Creating an empty
        // native object here would claim that ID and prevent the creator from
        // supplying its actual contents later.
        if !book.entries.is_empty() {
            ensure_imported_character_knowledge_book(transaction, &book)?;
        }
    }
    if let Some(transform_set) = content.runtime.materialize_transform_set(Provenance {
        // The immutable imported profile remains attached to the character.
        // This executable set is a native projection generated from that
        // profile, so it is not an unreviewed imported transform document.
        source_kind: SourceKind::Generated,
        source_id: Some(format!("character-import:{character_id}")),
        ..provenance.clone()
    }) {
        ensure_imported_character_transform_set(transaction, &transform_set)?;
    }
    let written = append_content_revision(
        transaction,
        DocumentTable::CharacterContent,
        &object_id,
        content.schema_version,
        content,
        &provenance,
        None,
        RevisionEventKind::Create,
    )?;
    let (document_json, _) = encode_document("character content", content)?;
    write_character_content_projection(
        transaction,
        &object_id,
        character_id,
        &written.revision_id,
        content,
        &document_json,
        Some(inspection_plan_sha256),
    )
}

fn ensure_imported_character_knowledge_book(
    transaction: &Transaction<'_>,
    book: &KnowledgeBook,
) -> CoreResult<()> {
    book.validate()
        .map_err(|error| CoreError::invalid(format!("knowledge book is invalid: {error}")))?;
    let existing = transaction
        .query_row(
            "SELECT revision.document_json
             FROM content_objects AS object
             JOIN content_object_state AS state ON state.object_id = object.id
             JOIN content_revisions AS revision
               ON revision.id = state.active_revision_id
              AND revision.object_id = object.id
             WHERE object.id = ?1
               AND object.object_kind = 'knowledge_book'
               AND object.deleted_at IS NULL",
            [book.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if let Some(document_json) = existing {
        let stored: KnowledgeBook = decode_document("knowledge book", &document_json)?;
        if stored == *book {
            return Ok(());
        }
        return Err(CoreError::invalid(format!(
            "embedded knowledge book {} conflicts with existing content",
            book.id.as_str()
        )));
    }
    let written = append_content_revision(
        transaction,
        DocumentTable::KnowledgeBooks,
        book.id.as_str(),
        book.schema_version,
        book,
        &book.provenance,
        None,
        RevisionEventKind::Import,
    )?;
    let (document_json, _) = encode_document("knowledge book", book)?;
    write_knowledge_book_projection(
        transaction,
        &written.revision_id,
        book,
        &document_json,
        None,
    )
}

fn ensure_imported_character_transform_set(
    transaction: &Transaction<'_>,
    transform_set: &TransformSet,
) -> CoreResult<()> {
    transform_set.validate().map_err(|error| {
        CoreError::invalid(format!("character transform set is invalid: {error}"))
    })?;
    let existing = transaction
        .query_row(
            "SELECT revision.document_json
             FROM content_objects AS object
             JOIN content_object_state AS state ON state.object_id = object.id
             JOIN content_revisions AS revision
               ON revision.id = state.active_revision_id
              AND revision.object_id = object.id
             WHERE object.id = ?1
               AND object.object_kind = 'transform_set'
               AND object.deleted_at IS NULL",
            [transform_set.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if let Some(document_json) = existing {
        let stored: TransformSet = decode_document("transform set", &document_json)?;
        if stored == *transform_set {
            return Ok(());
        }
        return Err(CoreError::invalid(format!(
            "character transform set {} conflicts with existing content",
            transform_set.id.as_str()
        )));
    }
    let written = append_content_revision(
        transaction,
        DocumentTable::TransformSets,
        transform_set.id.as_str(),
        transform_set.schema_version,
        transform_set,
        &transform_set.provenance,
        None,
        RevisionEventKind::Import,
    )?;
    let (document_json, _) = encode_document("transform set", transform_set)?;
    write_transform_set_projection(
        transaction,
        &written.revision_id,
        transform_set,
        &document_json,
        None,
    )
}

fn write_asset_descriptor(
    transaction: &Transaction<'_>,
    descriptor: &AssetDescriptor,
    source_revision_id: Option<&str>,
) -> CoreResult<()> {
    let (payload_json, _) = encode_document("asset descriptor", descriptor)?;
    let existing = transaction
        .query_row(
            "SELECT asset_hash, payload_json
             FROM asset_descriptors WHERE id = ?1",
            [descriptor.id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?;
    if let Some((asset_hash, stored_payload)) = existing {
        if asset_hash != descriptor.sha256.as_str() || stored_payload != payload_json {
            return Err(CoreError::invalid(format!(
                "asset descriptor {} conflicts with an existing immutable descriptor",
                descriptor.id.as_str()
            )));
        }
        return Ok(());
    }
    transaction
        .execute(
            "INSERT INTO asset_descriptors
             (id, asset_hash, name, role, media_type, size_bytes, width, height,
              duration_ms, risk_class, source_revision_id, source_kind,
              source_hash, logical_path, payload_json, created_at)
             VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'normal', ?10, ?11,
                 ?12, ?13, ?14, ?15
             )",
            params![
                descriptor.id.as_str(),
                descriptor.sha256.as_str(),
                descriptor.name,
                asset_role_str(descriptor.role),
                descriptor.media_type,
                descriptor.size_bytes,
                descriptor.width,
                descriptor.height,
                descriptor.duration_ms,
                source_revision_id,
                asset_source_kind_str(descriptor.source.kind),
                descriptor
                    .source
                    .source_sha256
                    .as_ref()
                    .map(lorepia_domain::Sha256Digest::as_str),
                descriptor.source.logical_path,
                payload_json,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn validate_package_provenance(
    label: &str,
    provenance: &Provenance,
    source_sha256: &str,
) -> CoreResult<()> {
    validate_optional_sha256("package source hash", Some(source_sha256))?;
    if provenance.source_kind != SourceKind::ImportedPackage
        || provenance.source_hash.as_deref() != Some(source_sha256)
    {
        return Err(CoreError::invalid(format!(
            "{label} provenance must identify the exact imported package source"
        )));
    }
    Ok(())
}

struct ImportedPackageDocument<'a, T> {
    label: &'static str,
    table: DocumentTable,
    object_id: &'a str,
    schema_version: u32,
    document: &'a T,
    provenance: &'a Provenance,
}

fn append_imported_package_document<T, F>(
    transaction: &Transaction<'_>,
    input: ImportedPackageDocument<'_, T>,
    expected_revision: Option<u64>,
    source_sha256: &str,
    write_projection: F,
) -> CoreResult<PackageDocumentWrite>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce(&Transaction<'_>, &str, &T, &str, Option<u64>) -> CoreResult<()>,
{
    validate_package_provenance(input.label, input.provenance, source_sha256)?;
    let written = append_content_revision(
        transaction,
        input.table,
        input.object_id,
        input.schema_version,
        input.document,
        input.provenance,
        expected_revision,
        RevisionEventKind::Import,
    )?;
    let (document_json, _) = encode_document(input.label, input.document)?;
    write_projection(
        transaction,
        &written.revision_id,
        input.document,
        &document_json,
        expected_revision,
    )?;
    Ok(PackageDocumentWrite {
        object_id: input.object_id.to_owned(),
        revision_id: written.revision_id,
        state_revision: written.state_version,
    })
}

fn append_imported_prompt_preset(
    transaction: &Transaction<'_>,
    preset: &PromptPreset,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "prompt preset",
            table: DocumentTable::PromptPresets,
            object_id: preset.id.as_str(),
            schema_version: preset.schema_version,
            document: preset,
            provenance: &preset.metadata.provenance,
        },
        expected_revision,
        source_sha256,
        write_prompt_preset_projection,
    )
}

fn append_imported_knowledge_book(
    transaction: &Transaction<'_>,
    book: &KnowledgeBook,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "knowledge book",
            table: DocumentTable::KnowledgeBooks,
            object_id: book.id.as_str(),
            schema_version: book.schema_version,
            document: book,
            provenance: &book.provenance,
        },
        expected_revision,
        source_sha256,
        write_knowledge_book_projection,
    )
}

fn append_imported_memory_profile(
    transaction: &Transaction<'_>,
    profile: &MemoryProfile,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "memory profile",
            table: DocumentTable::MemoryProfiles,
            object_id: profile.id.as_str(),
            schema_version: profile.schema_version,
            document: profile,
            provenance: &profile.provenance,
        },
        expected_revision,
        source_sha256,
        write_memory_profile_projection,
    )
}

fn append_imported_transform_set(
    transaction: &Transaction<'_>,
    transform_set: &TransformSet,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "transform set",
            table: DocumentTable::TransformSets,
            object_id: transform_set.id.as_str(),
            schema_version: transform_set.schema_version,
            document: transform_set,
            provenance: &transform_set.provenance,
        },
        expected_revision,
        source_sha256,
        write_transform_set_projection,
    )
}

fn append_imported_interaction_rule_set(
    transaction: &Transaction<'_>,
    rule_set: &InteractionRuleSet,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "interaction rule set",
            table: DocumentTable::InteractionRuleSets,
            object_id: rule_set.id.as_str(),
            schema_version: rule_set.schema_version,
            document: rule_set,
            provenance: &rule_set.provenance,
        },
        expected_revision,
        source_sha256,
        write_interaction_rule_set_projection,
    )
}

fn append_imported_content_module(
    transaction: &Transaction<'_>,
    module: &ContentModule,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "content module",
            table: DocumentTable::ContentModules,
            object_id: module.id.as_str(),
            schema_version: module.schema_version,
            document: module,
            provenance: &module.metadata.provenance,
        },
        expected_revision,
        source_sha256,
        write_content_module_projection,
    )
}

fn append_imported_character_package_content(
    transaction: &Transaction<'_>,
    character_id: &str,
    content: &CharacterContentV1,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    validate_identifier("character", character_id)?;
    if content
        .unknown_extensions
        .raw_source_sha256
        .as_ref()
        .is_some_and(|digest| digest.as_str() != source_sha256)
    {
        return Err(CoreError::invalid(
            "character extension index is not bound to the imported package source",
        ));
    }
    for asset in &content.assets {
        validate_package_asset_source(asset, source_sha256)?;
    }
    let object_id = character_content_object_id(character_id);
    let provenance = Provenance {
        source_kind: SourceKind::ImportedPackage,
        source_id: Some(character_id.to_owned()),
        source_hash: Some(source_sha256.to_owned()),
        author: None,
        license: None,
        imported_at: Some(Utc::now()),
    };
    let written = append_content_revision(
        transaction,
        DocumentTable::CharacterContent,
        &object_id,
        content.schema_version,
        content,
        &provenance,
        expected_revision,
        RevisionEventKind::Import,
    )?;
    let (document_json, _) = encode_document("character content", content)?;
    write_character_content_projection(
        transaction,
        &object_id,
        character_id,
        &written.revision_id,
        content,
        &document_json,
        None,
    )?;
    Ok(PackageDocumentWrite {
        object_id,
        revision_id: written.revision_id,
        state_revision: written.state_version,
    })
}

/// Appends one approved package document and all of its normalized
/// projections inside the caller-owned transaction.
///
/// Keeping this helper in the orchestration repository prevents the package
/// state machine from bypassing typed validation or creating a content
/// revision without its corresponding query projection.
pub(crate) fn append_package_commit_document(
    transaction: &Transaction<'_>,
    document: &PackageCommitDocument,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    match document {
        PackageCommitDocument::PromptPreset(preset) => {
            append_imported_prompt_preset(transaction, preset, expected_revision, source_sha256)
        }
        PackageCommitDocument::KnowledgeBook(book) => {
            append_imported_knowledge_book(transaction, book, expected_revision, source_sha256)
        }
        PackageCommitDocument::MemoryProfile(profile) => {
            append_imported_memory_profile(transaction, profile, expected_revision, source_sha256)
        }
        PackageCommitDocument::TransformSet(transform_set) => append_imported_transform_set(
            transaction,
            transform_set,
            expected_revision,
            source_sha256,
        ),
        PackageCommitDocument::InteractionRuleSet(rule_set) => {
            append_imported_interaction_rule_set(
                transaction,
                rule_set,
                expected_revision,
                source_sha256,
            )
        }
        PackageCommitDocument::ContentModule(module) => {
            append_imported_content_module(transaction, module, expected_revision, source_sha256)
        }
        PackageCommitDocument::CharacterContent {
            character_id,
            content,
        } => append_imported_character_package_content(
            transaction,
            character_id,
            content,
            expected_revision,
            source_sha256,
        ),
    }
}

fn validate_package_asset_source(
    descriptor: &AssetDescriptor,
    source_sha256: &str,
) -> CoreResult<()> {
    validate_optional_sha256("package source hash", Some(source_sha256))?;
    if descriptor.source.kind != lorepia_domain::AssetSourceKind::LorepiaPackage
        || descriptor
            .source
            .source_sha256
            .as_ref()
            .map(lorepia_domain::Sha256Digest::as_str)
            != Some(source_sha256)
    {
        return Err(CoreError::invalid(format!(
            "asset descriptor {} is not bound to the exact imported package source",
            descriptor.id.as_str()
        )));
    }
    Ok(())
}

/// Appends a package asset descriptor without allowing the package state
/// machine to bypass immutable-descriptor conflict checks.
pub(crate) fn append_package_asset_descriptor(
    transaction: &Transaction<'_>,
    descriptor: &AssetDescriptor,
    source_sha256: &str,
) -> CoreResult<()> {
    validate_package_asset_source(descriptor, source_sha256)?;
    write_asset_descriptor(transaction, descriptor, None)
}

const fn asset_role_str(role: AssetRole) -> &'static str {
    match role {
        AssetRole::Avatar => "avatar",
        AssetRole::Icon => "icon",
        AssetRole::Background => "background",
        AssetRole::UserIcon => "user_icon",
        AssetRole::Emotion => "emotion",
        AssetRole::Expression => "expression",
        AssetRole::Illustration => "illustration",
        AssetRole::Audio => "audio",
        AssetRole::Voice => "voice",
        AssetRole::Video => "video",
        AssetRole::StatusPanel => "status_panel",
        AssetRole::Attachment => "attachment",
        AssetRole::Other => "other",
    }
}

const fn asset_source_kind_str(kind: lorepia_domain::AssetSourceKind) -> &'static str {
    match kind {
        lorepia_domain::AssetSourceKind::CharacterCard => "character_card",
        lorepia_domain::AssetSourceKind::CharxPackage => "charx_package",
        lorepia_domain::AssetSourceKind::LorepiaPackage => "lorepia_package",
        lorepia_domain::AssetSourceKind::ContentModule => "content_module",
        lorepia_domain::AssetSourceKind::UserSelected => "user_selected",
        lorepia_domain::AssetSourceKind::Generated => "generated",
        lorepia_domain::AssetSourceKind::Unknown => "unknown",
    }
}
