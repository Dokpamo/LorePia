-- Test-only inverse of schema 41. Production never migrates backwards.
--
-- These previous-release fixtures intentionally open SQLite without LorePia's
-- registered integrity functions. Rebuilding or renaming a table would make
-- SQLite reparse unrelated integrity triggers that call those functions, so
-- the fixture narrows only this CHECK constraint in sqlite_schema and forces a
-- schema reload. The fixture contains no portable_runtime rows in either
-- capability table.

PRAGMA writable_schema = ON;

UPDATE sqlite_schema
SET sql = replace(
    sql,
    char(39) || 'portable_runtime' || char(39) || ',',
    ''
)
WHERE type = 'table'
  AND name IN (
      'content_module_required_capabilities',
      'package_capability_requests'
  )
  AND instr(sql, char(39) || 'portable_runtime' || char(39)) > 0;

PRAGMA writable_schema = RESET;
