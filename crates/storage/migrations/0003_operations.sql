-- 0003_operations.sql
-- Durable capture operation journal: enables idempotent replay after lost reply
-- and ownership-scoped isolation across principals.
-- `content_sha256` records the canonical digest; `semantic_fingerprint` is a
-- domain-separated fingerprint of the accepted request (canonical digest, asset role,
-- capture mode, capture context, provenances) used to match a replay independently of
-- the fresh random record/asset ids carried by a retry.
-- The saved row also carries the rest of the replayable receipt metadata: the closed
-- capture `mode` (16383 is the closed WebP lossless side limit), positive canonical
-- pixel `width`/`height`, and a bounded validated JSON `warnings` array. All of it is
-- written in the same transaction as the record, asset and file. The `warnings` array
-- shape is enforced here; the application revalidates the bounded count and per-entry
-- length on both write and read, so a tampered row is refused rather than repaired.
-- See ScreenMemory S02 §9.1, §8.7, §9.3.
CREATE TABLE ingest_operations (
    operation_id      TEXT PRIMARY KEY CHECK (length(operation_id) = 36),
    principal_id      TEXT NOT NULL CHECK (length(principal_id) = 36),
    kind              TEXT NOT NULL CHECK (kind IN ('capture')),
    planned_record_id TEXT NOT NULL CHECK (length(planned_record_id) = 36),
    planned_asset_id  TEXT NOT NULL CHECK (length(planned_asset_id) = 36),
    content_sha256    TEXT NOT NULL CHECK (length(content_sha256) = 64),
    semantic_fingerprint TEXT NOT NULL CHECK (length(semantic_fingerprint) = 64),
    mode              TEXT NOT NULL CHECK (mode IN ('region', 'active_window', 'monitor', 'desktop')),
    width             INTEGER NOT NULL CHECK (width > 0 AND width <= 16383),
    height            INTEGER NOT NULL CHECK (height > 0 AND height <= 16383),
    warnings          TEXT NOT NULL CHECK (json_valid(warnings) AND json_type(warnings) = 'array'),
    record_id         TEXT REFERENCES records(id),
    state             TEXT NOT NULL CHECK (state IN ('saved')),
    created_at_utc    TEXT NOT NULL CHECK (length(created_at_utc) = 24),
    updated_at_utc    TEXT NOT NULL CHECK (length(updated_at_utc) = 24),
    CHECK (state != 'saved' OR (record_id IS NOT NULL))
);
