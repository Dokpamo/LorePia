//! Durable UTF-8 delta checkpoints; message replacement remains the canonical close.
use super::{
    CoreError, CoreErrorCode, CoreResult, GenerationId, MessageId, OptionalExtension, Storage,
    TransactionBehavior, params, storage_corrupted, storage_db_error,
};
use rusqlite::Transaction;

mod proof_cache;
pub(super) use proof_cache::CheckpointProofCache;

const CHUNK_BYTES: usize = 64 * 1024;

#[derive(PartialEq, Eq)]
struct State {
    base_bytes: u64,
    total_bytes: u64,
    next_sequence: u64,
}

impl Storage {
    /// Appends a durable pending-assistant suffix at its exact byte watermark.
    /// An identical replay ending at the current watermark succeeds; older,
    /// conflicting or out-of-order writes fail without changing the journal.
    pub fn append_pending_assistant_checkpoint(
        &self,
        message_id: &MessageId,
        generation_id: &GenerationId,
        expected_bytes: u64,
        delta: &str,
    ) -> CoreResult<u64> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let state = load_state(&transaction, message_id, generation_id)?;
        let version = transaction
            .query_row("PRAGMA data_version", [], |row| row.get::<_, u64>(0))
            .map_err(storage_db_error)?;
        let mut proofs = self
            .checkpoint_proofs
            .lock()
            .map_err(|_| storage_corrupted("pending checkpoint proof lock poisoned"))?;
        if !proofs.contains(
            (transaction.total_changes(), version),
            &message_id.0,
            &generation_id.0,
            &state,
        ) {
            validate_chunks(&transaction, message_id, &state)?;
            #[cfg(test)]
            {
                proofs.validations += 1;
            }
        }
        let target = expected_bytes
            .checked_add(delta.len() as u64)
            .ok_or_else(|| CoreError::invalid("pending checkpoint byte count overflowed"))?;
        if expected_bytes != state.total_bytes {
            if target != state.total_bytes || expected_bytes > state.total_bytes {
                return Err(offset_conflict());
            }
            let content: String = transaction
                .query_row(
                    "SELECT content FROM messages_with_checkpoints WHERE id=?1",
                    [&message_id.0],
                    |row| row.get(0),
                )
                .map_err(storage_db_error)?;
            let offset = usize::try_from(expected_bytes).map_err(|_| offset_conflict())?;
            if content.get(offset..) != Some(delta) {
                return Err(offset_conflict());
            }
        } else if !delta.is_empty() {
            append_chunks(&transaction, message_id, &state, delta, target)?;
        }
        let next_sequence = transaction
            .query_row(
                "SELECT next_sequence FROM pending_assistant_checkpoints WHERE message_id=?1",
                [&message_id.0],
                |row| row.get::<_, u64>(0),
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
        proofs.record(
            (connection.total_changes(), version),
            &message_id.0,
            &generation_id.0,
            State {
                base_bytes: state.base_bytes,
                total_bytes: target,
                next_sequence,
            },
        );
        Ok(target)
    }
}

fn append_chunks(
    transaction: &Transaction<'_>,
    message_id: &MessageId,
    state: &State,
    delta: &str,
    target: u64,
) -> CoreResult<()> {
    let mut sequence = state.next_sequence;
    let mut offset = state.total_bytes;
    let mut remaining = delta;
    while !remaining.is_empty() {
        let mut end = remaining.len().min(CHUNK_BYTES);
        while !remaining.is_char_boundary(end) {
            end -= 1;
        }
        let (chunk, rest) = remaining.split_at(end);
        transaction
            .execute(
                "INSERT INTO pending_assistant_checkpoint_chunks
                     (message_id,sequence,offset_bytes,byte_length,content)
                     VALUES (?1,?2,?3,?4,?5)",
                params![message_id.0, as_i64(sequence)?, as_i64(offset)?, end, chunk],
            )
            .map_err(storage_db_error)?;
        sequence = sequence
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("pending checkpoint sequence overflowed"))?;
        offset = offset
            .checked_add(end as u64)
            .ok_or_else(|| CoreError::invalid("pending checkpoint byte count overflowed"))?;
        remaining = rest;
    }
    if offset != target {
        return Err(storage_corrupted("pending checkpoint watermark mismatch"));
    }
    Ok(())
}

fn load_state(
    transaction: &Transaction<'_>,
    message: &MessageId,
    generation: &GenerationId,
) -> CoreResult<State> {
    let row = transaction
        .query_row(
            "SELECT length(CAST(message.content AS BLOB)), checkpoint.generation_id,
                checkpoint.base_bytes, checkpoint.total_bytes, checkpoint.next_sequence
         FROM messages message LEFT JOIN pending_assistant_checkpoints checkpoint
           ON checkpoint.message_id=message.id
         WHERE message.id=?1 AND message.generation_id=?2
           AND message.role='assistant' AND message.status='pending'",
            params![message.0, generation.0],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<u64>>(2)?,
                    row.get::<_, Option<u64>>(3)?,
                    row.get::<_, Option<u64>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "pending assistant checkpoint target was not found",
                false,
            )
        })?;
    if let Some(owner) = row.1 {
        let (Some(base_bytes), Some(total_bytes), Some(next_sequence)) = (row.2, row.3, row.4)
        else {
            return Err(storage_corrupted("pending checkpoint state is incomplete"));
        };
        if owner != generation.0 || base_bytes != row.0 || total_bytes < base_bytes {
            return Err(storage_corrupted(
                "pending checkpoint owner or base changed",
            ));
        }
        Ok(State {
            base_bytes,
            total_bytes,
            next_sequence,
        })
    } else {
        // Validate the existing canonical base once before establishing a journal.
        transaction
            .query_row(
                "SELECT content FROM messages WHERE id=?1",
                [&message.0],
                |row| row.get_ref(0)?.as_str().map(|_| ()).map_err(Into::into),
            )
            .map_err(storage_db_error)?;
        transaction.execute(
            "INSERT INTO pending_assistant_checkpoints
             (message_id,generation_id,base_bytes,total_bytes,next_sequence) VALUES (?1,?2,?3,?3,0)",
            params![message.0,generation.0,as_i64(row.0)?],
        ).map_err(storage_db_error)?;
        Ok(State {
            base_bytes: row.0,
            total_bytes: row.0,
            next_sequence: 0,
        })
    }
}

fn validate_chunks(
    transaction: &Transaction<'_>,
    message: &MessageId,
    state: &State,
) -> CoreResult<()> {
    let valid: bool = transaction.query_row(
        "SELECT COUNT(*)=?2 AND COALESCE(MIN(sequence),0)=0
                AND COALESCE(MAX(sequence),-1)=?2-1
                AND COALESCE(SUM(byte_length),0)=?3-?4
                AND NOT EXISTS (
                    SELECT 1 FROM pending_assistant_checkpoint_chunks current
                    LEFT JOIN pending_assistant_checkpoint_chunks previous
                      ON previous.message_id=current.message_id AND previous.sequence=current.sequence-1
                    WHERE current.message_id=?1
                      AND (current.offset_bytes != CASE WHEN current.sequence=0 THEN ?4
                           ELSE previous.offset_bytes+previous.byte_length END
                           OR (current.sequence>0 AND previous.sequence IS NULL))
                )
         FROM pending_assistant_checkpoint_chunks WHERE message_id=?1",
        params![message.0,as_i64(state.next_sequence)?,as_i64(state.total_bytes)?,as_i64(state.base_bytes)?],
        |row| row.get(0),
    ).map_err(storage_db_error)?;
    if !valid {
        return Err(storage_corrupted(
            "pending checkpoint chunks are incomplete or out of order",
        ));
    }
    Ok(())
}

fn as_i64(value: u64) -> CoreResult<i64> {
    i64::try_from(value)
        .map_err(|_| CoreError::invalid("pending checkpoint exceeds SQLite integer range"))
}

fn offset_conflict() -> CoreError {
    CoreError::invalid("pending checkpoint offset or replay content conflicts with durable data")
}

#[cfg(test)]
mod tests;
