use lorepia_domain::CoreResult;

use super::{Storage, migration_verification::read_current_schema_version};

pub(crate) const SCHEMA_VERSION: u32 = 40;
pub(crate) const FROZEN_NATIVE_SCHEMA_VERSION: u32 = 11;

impl Storage {
    /// Reads and validates the durable migration registry.
    ///
    /// This intentionally does not return the compile-time schema constant:
    /// corruption or out-of-band registry edits after open must remain
    /// observable instead of being reported as a successful current schema.
    pub fn schema_version(&self) -> CoreResult<u32> {
        let connection = self.connection()?;
        read_current_schema_version(&connection)
    }
}
