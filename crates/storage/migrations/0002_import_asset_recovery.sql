ALTER TABLE import_jobs
ADD COLUMN asset_hashes_json TEXT NOT NULL DEFAULT '[]';
