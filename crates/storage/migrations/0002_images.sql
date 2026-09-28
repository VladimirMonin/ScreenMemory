CREATE TABLE records (
    record_pk       INTEGER PRIMARY KEY AUTOINCREMENT,
    id              TEXT NOT NULL UNIQUE CHECK (length(id) = 36),
    library_id      TEXT NOT NULL REFERENCES library_meta(library_id),
    created_at      TEXT NOT NULL CHECK (length(created_at) = 24 AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    capture_context TEXT NOT NULL CHECK (json_valid(capture_context) AND json_type(capture_context) = 'object'),
    provenances     TEXT NOT NULL CHECK (json_valid(provenances) AND json_type(provenances) = 'array' AND json_array_length(provenances) >= 1)
);
CREATE TABLE assets (
    id          TEXT PRIMARY KEY CHECK (length(id) = 36),
    record_id   TEXT NOT NULL REFERENCES records(id) ON DELETE RESTRICT,
    asset_type  TEXT NOT NULL CHECK (asset_type IN ('original', 'preview', 'edited')),
    format      TEXT NOT NULL CHECK (format IN ('webp_lossless')),
    path        TEXT NOT NULL UNIQUE CHECK (substr(path, 1, 7) = 'assets/'
                     AND path LIKE 'assets/____-__/%'
                     AND substr(path, -5) = '.webp'
                     AND instr(path, '..') = 0
                     AND instr(path, char(92)) = 0
                     AND instr(path, ':') = 0
                     AND length(path) BETWEEN 32 AND 200),
    size_bytes  INTEGER NOT NULL CHECK (size_bytes > 0),
    sha256      TEXT NOT NULL CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    created_at  TEXT NOT NULL CHECK (length(created_at) = 24 AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    UNIQUE(record_id, asset_type)
);
