-- S03 additive provenance/journal and asset-container expansion. Capture journals stay intact.
ALTER TABLE records ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'capture' CHECK (source_kind IN ('capture','imported_image'));
CREATE TABLE assets_v5 (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    record_id TEXT NOT NULL REFERENCES records(id) ON DELETE RESTRICT,
    asset_type TEXT NOT NULL CHECK (asset_type IN ('original','preview','edited')),
    format TEXT NOT NULL CHECK (format IN ('webp_lossless','avif_sdr90')),
    path TEXT NOT NULL UNIQUE CHECK (substr(path,1,7) = 'assets/' AND instr(path,'..') = 0 AND instr(path,char(92)) = 0 AND instr(path,':') = 0 AND length(path) BETWEEN 32 AND 200 AND (substr(path,-5) = '.webp' OR substr(path,-5) = '.avif')),
    size_bytes INTEGER NOT NULL CHECK (size_bytes > 0 AND size_bytes <= 134217728),
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL CHECK (length(created_at) = 24),
    UNIQUE(record_id,asset_type)
);
INSERT INTO assets_v5 SELECT * FROM assets;
DROP TABLE assets;
ALTER TABLE assets_v5 RENAME TO assets;
CREATE TABLE import_batches (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    principal_id TEXT NOT NULL CHECK (length(principal_id) = 36),
    request_json TEXT NOT NULL CHECK (json_valid(request_json)),
    control TEXT NOT NULL CHECK (control IN ('running','paused','cancelled')),
    discovery_complete INTEGER NOT NULL DEFAULT 0 CHECK (discovery_complete IN (0,1)),
    discovery_limit INTEGER NOT NULL DEFAULT 100000 CHECK (discovery_limit BETWEEN 100000 AND 10000000),
    discovery_limit_hit INTEGER NOT NULL DEFAULT 0 CHECK (discovery_limit_hit IN (0,1)),
    discovery_abandoned INTEGER NOT NULL DEFAULT 0 CHECK (discovery_abandoned IN (0,1)),
    directory_issues INTEGER NOT NULL DEFAULT 0 CHECK (directory_issues >= 0),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at TEXT NOT NULL CHECK (length(created_at) = 24)
);
CREATE TABLE import_items (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    batch_id TEXT NOT NULL REFERENCES import_batches(id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    source_path TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('discovered','processing','held','recoverable','saved_new','linked_existing','skipped','cancelled_unaccepted')),
    snapshot_id TEXT NOT NULL UNIQUE CHECK (length(snapshot_id) = 36),
    operation_id TEXT NOT NULL UNIQUE CHECK (length(operation_id) = 36),
    planned_record_id TEXT NOT NULL CHECK (length(planned_record_id) = 36),
    planned_asset_id TEXT NOT NULL CHECK (length(planned_asset_id) = 36),
    durable_input INTEGER NOT NULL DEFAULT 0 CHECK (durable_input IN (0,1)),
    resolved_source_path TEXT,
    source_sha256 TEXT CHECK (source_sha256 IS NULL OR (length(source_sha256) = 64 AND source_sha256 NOT GLOB '*[^0-9a-f]*')),
    source_length INTEGER CHECK (source_length > 0 AND source_length <= 536870912),
    outcome_json TEXT CHECK (outcome_json IS NULL OR json_valid(outcome_json)),
    error_code TEXT,
    UNIQUE(batch_id,source_path COLLATE NOCASE),
    UNIQUE(batch_id,ordinal)
);
CREATE TABLE import_item_roots (
    item_id TEXT NOT NULL REFERENCES import_items(id),
    root_ordinal INTEGER NOT NULL CHECK (root_ordinal >= 0 AND root_ordinal < 128),
    PRIMARY KEY(item_id,root_ordinal)
);
CREATE TABLE origin_occurrences (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    item_id TEXT NOT NULL UNIQUE REFERENCES import_items(id),
    record_id TEXT NOT NULL REFERENCES records(id),
    original_asset_id TEXT NOT NULL REFERENCES assets(id),
    source_sha256 TEXT NOT NULL CHECK (length(source_sha256) = 64),
    source_length INTEGER NOT NULL CHECK (source_length > 0),
    source_path TEXT NOT NULL,
    resolved_source_path TEXT NOT NULL,
    metadata_json TEXT NOT NULL CHECK (json_valid(metadata_json)),
    created_at TEXT NOT NULL CHECK (length(created_at) = 24)
);
CREATE INDEX origins_source ON origin_occurrences(source_sha256,source_length);
CREATE TABLE asset_processing (
    asset_id TEXT PRIMARY KEY REFERENCES assets(id),
    report_json TEXT NOT NULL CHECK (json_valid(report_json))
);
CREATE TABLE export_operations (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    principal_id TEXT NOT NULL CHECK (length(principal_id) = 36),
    request_json TEXT NOT NULL CHECK (json_valid(request_json)),
    source_asset_id TEXT NOT NULL REFERENCES assets(id),
    state TEXT NOT NULL CHECK (state IN ('pending','saved','recoverable')),
    receipt_json TEXT CHECK (receipt_json IS NULL OR json_valid(receipt_json)),
    source_metadata_json TEXT NOT NULL CHECK (json_valid(source_metadata_json))
);
