-- Schema-only pending capture intent. No row in this table is a Saved receipt.
-- Future writer: delete the pending row before inserting its saved journal row in
-- the SAME BEGIN IMMEDIATE transaction. Neither table admits a shared operation_id.
CREATE TABLE capture_intents (
    operation_id TEXT PRIMARY KEY NOT NULL CHECK (length(operation_id) = 36 AND operation_id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]' AND operation_id != '00000000-0000-0000-0000-000000000000'),
    library_id TEXT NOT NULL REFERENCES library_meta(library_id),
    principal_id TEXT NOT NULL CHECK (length(principal_id) = 36 AND principal_id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]' AND principal_id != '00000000-0000-0000-0000-000000000000'),
    kind TEXT NOT NULL CHECK (kind = 'capture'),
    planned_record_id TEXT NOT NULL UNIQUE CHECK (length(planned_record_id) = 36 AND planned_record_id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-4[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    planned_asset_id TEXT NOT NULL UNIQUE CHECK (length(planned_asset_id) = 36 AND planned_asset_id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-4[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    frame_id TEXT NOT NULL UNIQUE CHECK (length(frame_id) = 36 AND frame_id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-4[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    mode TEXT NOT NULL CHECK (mode IN ('region', 'active_window', 'monitor', 'desktop')),
    width INTEGER NOT NULL CHECK (typeof(width) = 'integer' AND width BETWEEN 1 AND 16383),
    height INTEGER NOT NULL CHECK (typeof(height) = 'integer' AND height BETWEEN 1 AND 16383 AND width * height <= 96000000),
    pixel_sha256 TEXT NOT NULL CHECK (length(pixel_sha256) = 64 AND pixel_sha256 NOT GLOB '*[^0-9a-f]*'),
    context_sha256 TEXT NOT NULL CHECK (length(context_sha256) = 64 AND context_sha256 NOT GLOB '*[^0-9a-f]*'),
    request_fingerprint TEXT NOT NULL CHECK (length(request_fingerprint) = 64 AND request_fingerprint NOT GLOB '*[^0-9a-f]*'),
    context_json TEXT NOT NULL CHECK (typeof(context_json) = 'text' AND length(CAST(context_json AS BLOB)) BETWEEN 1 AND 262144 AND json_valid(context_json) AND json_type(context_json) = 'object'),
    warnings TEXT NOT NULL CHECK (typeof(warnings) = 'text' AND length(CAST(warnings AS BLOB)) BETWEEN 2 AND 8192 AND json_valid(warnings) AND json_type(warnings) = 'array' AND json_array_length(warnings) <= 16),
    encoder_fingerprint TEXT NOT NULL CHECK (typeof(encoder_fingerprint) = 'text' AND length(CAST(encoder_fingerprint AS BLOB)) BETWEEN 1 AND 512),
    captured_at_utc TEXT NOT NULL CHECK (length(captured_at_utc) = 24 AND captured_at_utc GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    planned_created_at_utc TEXT NOT NULL CHECK (length(planned_created_at_utc) = 24 AND planned_created_at_utc GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    CHECK (operation_id != planned_record_id AND operation_id != planned_asset_id AND operation_id != frame_id AND planned_record_id != planned_asset_id AND planned_record_id != frame_id AND planned_asset_id != frame_id)
);
CREATE TRIGGER capture_intents_no_saved_insert BEFORE INSERT ON capture_intents
BEGIN SELECT RAISE(ABORT, 'operation already saved') WHERE EXISTS (SELECT 1 FROM ingest_operations WHERE operation_id = NEW.operation_id); END;
CREATE TRIGGER capture_intents_no_saved_update BEFORE UPDATE OF operation_id ON capture_intents
BEGIN SELECT RAISE(ABORT, 'operation already saved') WHERE EXISTS (SELECT 1 FROM ingest_operations WHERE operation_id = NEW.operation_id); END;
CREATE TRIGGER ingest_operations_no_pending_insert BEFORE INSERT ON ingest_operations
BEGIN SELECT RAISE(ABORT, 'operation still pending') WHERE EXISTS (SELECT 1 FROM capture_intents WHERE operation_id = NEW.operation_id); END;
CREATE TRIGGER ingest_operations_no_pending_update BEFORE UPDATE OF operation_id ON ingest_operations
BEGIN SELECT RAISE(ABORT, 'operation still pending') WHERE EXISTS (SELECT 1 FROM capture_intents WHERE operation_id = NEW.operation_id); END;
CREATE TRIGGER capture_intents_unique_resources_insert BEFORE INSERT ON capture_intents
BEGIN SELECT RAISE(ABORT, 'pending resource identity collision') WHERE EXISTS (SELECT 1 FROM capture_intents WHERE NEW.planned_record_id IN (planned_record_id, planned_asset_id, frame_id) OR NEW.planned_asset_id IN (planned_record_id, planned_asset_id, frame_id) OR NEW.frame_id IN (planned_record_id, planned_asset_id, frame_id)); END;
CREATE TRIGGER capture_intents_unique_resources_update BEFORE UPDATE OF planned_record_id, planned_asset_id, frame_id ON capture_intents
BEGIN SELECT RAISE(ABORT, 'pending resource identity collision') WHERE EXISTS (SELECT 1 FROM capture_intents WHERE operation_id != OLD.operation_id AND (NEW.planned_record_id IN (planned_record_id, planned_asset_id, frame_id) OR NEW.planned_asset_id IN (planned_record_id, planned_asset_id, frame_id) OR NEW.frame_id IN (planned_record_id, planned_asset_id, frame_id))); END;
CREATE TRIGGER capture_intents_warnings_insert BEFORE INSERT ON capture_intents
BEGIN SELECT RAISE(ABORT, 'invalid pending warnings') WHERE EXISTS (SELECT 1 FROM json_each(NEW.warnings) WHERE type != 'text' OR length(CAST(value AS BLOB)) NOT BETWEEN 1 AND 64 OR value GLOB '*[^a-z_]*'); END;
CREATE TRIGGER capture_intents_warnings_update BEFORE UPDATE OF warnings ON capture_intents
BEGIN SELECT RAISE(ABORT, 'invalid pending warnings') WHERE EXISTS (SELECT 1 FROM json_each(NEW.warnings) WHERE type != 'text' OR length(CAST(value AS BLOB)) NOT BETWEEN 1 AND 64 OR value GLOB '*[^a-z_]*'); END;
