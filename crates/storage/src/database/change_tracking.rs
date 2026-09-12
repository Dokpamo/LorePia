//! Connection-local epochs; callbacks never reenter `SQLite` or acquire a mutex.
use super::{Connection, CoreResult, storage_db_error};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

pub(super) struct ChangeTracking {
    lineage: Arc<AtomicU64>,
    instance: uuid::Uuid,
}

impl ChangeTracking {
    pub(super) fn install(connection: &Connection) -> CoreResult<Self> {
        let lineage = Arc::new(AtomicU64::new(0));
        let updates = Arc::clone(&lineage);
        connection
            .update_hook(Some(move |_, database: &str, table: &str, _| {
                if database == "main" && table == "messages" {
                    advance(&updates);
                }
            }))
            .map_err(storage_db_error)?;
        let preparation = Arc::clone(&lineage);
        let mut dropping_messages = false;
        connection
            .authorizer(Some(move |context: AuthContext<'_>| {
                // SQLite authorizes DROP with a Delete immediately after the
                // Drop action. IGNORE there would cancel DROP, not just truncate.
                let drop_delete = dropping_messages;
                dropping_messages = context.database_name == Some("main")
                    && matches!(
                        context.action,
                        AuthAction::DropTable {
                            table_name: "messages"
                        } | AuthAction::DropView {
                            view_name: "messages"
                        } | AuthAction::DropVtable {
                            table_name: "messages",
                            ..
                        }
                    );
                if let AuthAction::Delete {
                    table_name: "messages",
                } = context.action
                    && context.database_name == Some("main")
                    && !drop_delete
                {
                    // SQLITE_IGNORE on DELETE preserves deletion but disables the
                    // truncate optimization, which would otherwise skip row hooks.
                    return Authorization::Ignore;
                }
                if invalidates_schema(context.action) {
                    advance(&preparation);
                }
                Authorization::Allow
            }))
            .map_err(storage_db_error)?;
        Ok(Self {
            lineage,
            instance: uuid::Uuid::new_v4(),
        })
    }

    pub(super) fn lineage_epoch(&self) -> u64 {
        self.lineage.load(Ordering::Relaxed)
    }

    // Must run under the same established transaction as the returned contents.
    pub(super) fn history_snapshot_token(
        &self,
        connection: &Connection,
        branch: &str,
    ) -> CoreResult<String> {
        let version = connection
            .query_row("PRAGMA data_version", [], |row| row.get::<_, u64>(0))
            .map_err(storage_db_error)?;
        let schema = connection
            .query_row("PRAGMA schema_version", [], |row| row.get::<_, u64>(0))
            .map_err(storage_db_error)?;
        let mut hash = Sha256::new();
        hash.update(b"lorepia-history-snapshot-v1\0");
        hash.update(self.instance.as_bytes());
        hash.update(connection.total_changes().to_le_bytes());
        hash.update(version.to_le_bytes());
        hash.update(schema.to_le_bytes());
        hash.update(branch.as_bytes());
        Ok(hex::encode(hash.finalize()))
    }
}

fn advance(epoch: &AtomicU64) {
    let _ = epoch.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

fn invalidates_schema(action: AuthAction<'_>) -> bool {
    matches!(
        action,
        AuthAction::CreateIndex { .. }
            | AuthAction::CreateTable { .. }
            | AuthAction::CreateTempIndex { .. }
            | AuthAction::CreateTempTable { .. }
            | AuthAction::CreateTempTrigger { .. }
            | AuthAction::CreateTempView { .. }
            | AuthAction::CreateTrigger { .. }
            | AuthAction::CreateView { .. }
            | AuthAction::DropIndex { .. }
            | AuthAction::DropTable { .. }
            | AuthAction::DropTempIndex { .. }
            | AuthAction::DropTempTable { .. }
            | AuthAction::DropTempTrigger { .. }
            | AuthAction::DropTempView { .. }
            | AuthAction::DropTrigger { .. }
            | AuthAction::DropView { .. }
            | AuthAction::AlterTable { .. }
            | AuthAction::CreateVtable { .. }
            | AuthAction::DropVtable { .. }
            | AuthAction::Attach { .. }
            | AuthAction::Detach { .. }
            | AuthAction::Pragma {
                pragma_value: Some(_),
                ..
            }
    )
}

#[cfg(test)]
mod tests;
