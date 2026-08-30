use super::{CoreResult, Storage, storage_db_error};

impl Storage {
    pub fn recovery_pending(&self) -> CoreResult<bool> {
        self.connection()?
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM import_jobs
                    UNION ALL
                    SELECT 1 FROM package_cas_promotion_journal
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)
    }
}
