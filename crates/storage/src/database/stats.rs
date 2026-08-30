use super::{CoreResult, Storage, count};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseStats {
    pub characters: u64,
    pub conversations: u64,
    pub messages: u64,
    pub pending_imports: u64,
}

impl Storage {
    pub fn stats(&self) -> CoreResult<DatabaseStats> {
        let connection = self.connection()?;
        Ok(DatabaseStats {
            characters: count(&connection, "characters")?,
            conversations: count(&connection, "conversations")?,
            messages: count(&connection, "messages")?,
            pending_imports: count(&connection, "import_jobs")?,
        })
    }
}
