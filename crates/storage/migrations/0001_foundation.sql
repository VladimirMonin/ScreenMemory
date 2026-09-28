CREATE TABLE library_meta (
    singleton     INTEGER PRIMARY KEY CHECK (singleton = 1),
    library_id    TEXT NOT NULL UNIQUE,
    format_major  INTEGER NOT NULL CHECK (format_major > 0),
    created_at    TEXT NOT NULL
);
CREATE TABLE library_settings (
    key           TEXT PRIMARY KEY,
    value_json    TEXT NOT NULL,
    revision      INTEGER NOT NULL CHECK (revision > 0)
);
