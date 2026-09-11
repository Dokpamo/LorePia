-- Immutable chunks for complete module authority documents. Manifests remain
-- in the existing plan rows; every chunk is published in their transaction.
CREATE TABLE module_plan_documents (
    kind TEXT NOT NULL CHECK (kind IN ('review', 'approval', 'runtime')),
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    byte_length INTEGER NOT NULL CHECK (byte_length > 0 AND byte_length <= 33554432),
    part_count INTEGER NOT NULL CHECK (part_count > 0 AND part_count <= 128),
    PRIMARY KEY (kind, sha256)
);
CREATE TABLE module_plan_document_parts (
    kind TEXT NOT NULL,
    document_sha256 TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0 AND ordinal < 128),
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    bytes BLOB NOT NULL CHECK (length(bytes) > 0 AND length(bytes) <= 262144),
    PRIMARY KEY (kind, document_sha256, ordinal),
    FOREIGN KEY (kind, document_sha256) REFERENCES module_plan_documents(kind, sha256)
        ON DELETE RESTRICT ON UPDATE RESTRICT
);
CREATE TRIGGER module_plan_documents_no_update BEFORE UPDATE ON module_plan_documents
BEGIN SELECT RAISE(ABORT, 'module authority document is immutable'); END;
CREATE TRIGGER module_plan_documents_no_delete BEFORE DELETE ON module_plan_documents
BEGIN SELECT RAISE(ABORT, 'module authority document is immutable'); END;
CREATE TRIGGER module_plan_parts_no_update BEFORE UPDATE ON module_plan_document_parts
BEGIN SELECT RAISE(ABORT, 'module authority document part is immutable'); END;
CREATE TRIGGER module_plan_parts_no_delete BEFORE DELETE ON module_plan_document_parts
BEGIN SELECT RAISE(ABORT, 'module authority document part is immutable'); END;
