//! Versioned native-only documents for complete module authority compositions.
//! Individual content documents keep their old limits. The aggregate plan has
//! bounded expansion and immutable 256KiB pieces, in the caller's transaction.

use super::{sha256_hex, storage_corrupted, storage_db_error, validate_json_bounds};
use lorepia_domain::{CoreError, CoreResult, Sha256Digest};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::borrow::Cow;

const PART_BYTES: usize = 256 * 1024;
const MAX_PARTS: usize = 128;
const MAX_BYTES: usize = PART_BYTES * MAX_PARTS;
const MAX_NODES: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Review,
    Approval,
    Runtime,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Approval => "approval",
            Self::Runtime => "runtime",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    lorepia_module_document: u32,
    kind: Kind,
    sha256: String,
    byte_length: usize,
    part_count: usize,
}

pub(crate) fn validate(json: &str) -> CoreResult<()> {
    parse_validated(json).map(|_| ())
}

fn parse_validated(json: &str) -> CoreResult<serde_json::Value> {
    super::document_history::parse_json_limits(
        "module authority composition",
        json,
        MAX_BYTES,
        MAX_BYTES,
        MAX_NODES,
    )
}

pub(crate) fn encode<T: Serialize>(value: &T) -> CoreResult<String> {
    let json =
        serde_json::to_string(value).map_err(|error| CoreError::internal(error.to_string()))?;
    validate(&json)?;
    Ok(json)
}

pub(crate) fn encode_hashed<T: Serialize>(value: &T) -> CoreResult<(String, Sha256Digest)> {
    let json = encode(value)?;
    let digest = Sha256Digest::parse(sha256_hex(json.as_bytes())).map_err(CoreError::invalid)?;
    Ok((json, digest))
}

pub(crate) fn store(transaction: &Transaction<'_>, kind: Kind, json: &str) -> CoreResult<String> {
    validate(json)?;
    if validate_json_bounds("module authority document", json).is_ok() {
        return Ok(json.to_owned());
    }
    let sha256 = sha256_hex(json.as_bytes());
    let manifest = Manifest {
        lorepia_module_document: 1,
        kind,
        sha256: sha256.clone(),
        byte_length: json.len(),
        part_count: json.len().div_ceil(PART_BYTES),
    };
    let encoded =
        serde_json::to_string(&manifest).map_err(|error| CoreError::internal(error.to_string()))?;
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO module_plan_documents (kind, sha256, byte_length, part_count) VALUES (?1, ?2, ?3, ?4)",
        params![kind.name(), sha256, i64::try_from(manifest.byte_length).map_err(|_| storage_corrupted("module document integer overflow"))?, i64::try_from(manifest.part_count).map_err(|_| storage_corrupted("module document integer overflow"))?],
    ).map_err(storage_db_error)?;
    if inserted != 0 {
        for (ordinal, bytes) in json.as_bytes().chunks(PART_BYTES).enumerate() {
            transaction.execute(
                "INSERT INTO module_plan_document_parts (kind, document_sha256, ordinal, sha256, bytes) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![kind.name(), sha256, i64::try_from(ordinal).map_err(|_| storage_corrupted("module document integer overflow"))?, sha256_hex(bytes), bytes],
            ).map_err(storage_db_error)?;
        }
    }
    if expand(transaction, kind, &encoded)?.as_ref() != json {
        return Err(storage_corrupted(
            "module authority document identity collision",
        ));
    }
    Ok(encoded)
}

pub(crate) fn expand<'a>(
    connection: &Connection,
    kind: Kind,
    stored: &'a str,
) -> CoreResult<Cow<'a, str>> {
    // Legacy generation snapshots permitted larger inline authority than
    // ordinary documents. Read both forms within the aggregate bound.
    let value = parse_validated(stored)
        .map_err(|_| storage_corrupted("stored module document exceeds limits"))?;
    if value.get("lorepia_module_document").is_none() {
        return Ok(Cow::Borrowed(stored));
    }
    validate_json_bounds("module document manifest", stored)
        .map_err(|_| storage_corrupted("module document manifest exceeds limits"))?;
    let manifest: Manifest = serde_json::from_value(value)
        .map_err(|_| storage_corrupted("invalid module document manifest"))?;
    if manifest.lorepia_module_document != 1
        || manifest.kind != kind
        || manifest.sha256.len() != 64
        || !manifest
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || manifest.byte_length == 0
        || manifest.byte_length > MAX_BYTES
        || manifest.part_count == 0
        || manifest.part_count > MAX_PARTS
        || manifest.part_count != manifest.byte_length.div_ceil(PART_BYTES)
    {
        return Err(storage_corrupted(
            "module document manifest exceeds its contract",
        ));
    }
    let metadata = connection.query_row(
        "SELECT byte_length, part_count FROM module_plan_documents WHERE kind = ?1 AND sha256 = ?2",
        params![kind.name(), manifest.sha256], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    ).optional().map_err(storage_db_error)?;
    if metadata
        != Some((
            i64::try_from(manifest.byte_length)
                .map_err(|_| storage_corrupted("module document integer overflow"))?,
            i64::try_from(manifest.part_count)
                .map_err(|_| storage_corrupted("module document integer overflow"))?,
        ))
    {
        return Err(storage_corrupted(
            "module document manifest has no exact immutable metadata",
        ));
    }
    let mut statement = connection.prepare(
        "SELECT ordinal, sha256, CASE WHEN length(bytes) <= 262144 THEN bytes ELSE NULL END
         FROM module_plan_document_parts WHERE kind = ?1 AND document_sha256 = ?2 ORDER BY ordinal LIMIT 129",
    ).map_err(storage_db_error)?;
    let mut rows = statement
        .query(params![kind.name(), manifest.sha256])
        .map_err(storage_db_error)?;
    let mut bytes = Vec::with_capacity(manifest.byte_length);
    let mut count = 0;
    while let Some(row) = rows.next().map_err(storage_db_error)? {
        let ordinal: i64 = row.get(0).map_err(storage_db_error)?;
        let hash: String = row.get(1).map_err(storage_db_error)?;
        let part: Option<Vec<u8>> = row.get(2).map_err(storage_db_error)?;
        let part =
            part.ok_or_else(|| storage_corrupted("module document part exceeds byte limit"))?;
        let expected_length =
            (manifest.byte_length.saturating_sub(count * PART_BYTES)).min(PART_BYTES);
        if ordinal
            != i64::try_from(count)
                .map_err(|_| storage_corrupted("module document integer overflow"))?
            || count >= manifest.part_count
            || part.len() != expected_length
            || sha256_hex(&part) != hash
        {
            return Err(storage_corrupted(
                "module document part order, length or identity changed",
            ));
        }
        bytes.extend_from_slice(&part);
        count += 1;
    }
    if count != manifest.part_count
        || bytes.len() != manifest.byte_length
        || sha256_hex(&bytes) != manifest.sha256
    {
        return Err(storage_corrupted(
            "module document parts are incomplete or corrupted",
        ));
    }
    let json =
        String::from_utf8(bytes).map_err(|_| storage_corrupted("module document is not UTF-8"))?;
    validate(&json).map_err(|_| storage_corrupted("expanded module document violates bounds"))?;
    Ok(Cow::Owned(json))
}

pub(crate) fn decode<T: DeserializeOwned>(
    connection: &Connection,
    kind: Kind,
    stored: &str,
) -> CoreResult<T> {
    serde_json::from_str(&expand(connection, kind, stored)?)
        .map_err(|error| storage_corrupted(format!("invalid module authority document: {error}")))
}

#[cfg(test)]
mod tests;
