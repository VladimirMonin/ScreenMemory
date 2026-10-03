-- Manual knowledge v1, additive only. Canonical/source/origin tables untouched.
CREATE TABLE category_catalog_meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), revision INTEGER NOT NULL CHECK(revision>0));
INSERT INTO category_catalog_meta VALUES(1,1);
CREATE TABLE knowledge_categories (
 id TEXT PRIMARY KEY CHECK(length(id)=36), revision INTEGER NOT NULL CHECK(revision>0),
 name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 80), description TEXT NOT NULL CHECK(length(description)<=400),
 normalization_version TEXT NOT NULL CHECK(normalization_version='win_nfc_unicase2.9_v1'),
 name_key TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('active','archived','deleted'))
);
CREATE UNIQUE INDEX knowledge_category_names ON knowledge_categories(name_key) WHERE state!='deleted';
CREATE TABLE knowledge_category_versions (
 category_id TEXT NOT NULL REFERENCES knowledge_categories(id), revision INTEGER NOT NULL CHECK(revision>0),
 value_json TEXT NOT NULL CHECK(json_valid(value_json)), PRIMARY KEY(category_id,revision)
);
CREATE TRIGGER knowledge_category_versions_no_update BEFORE UPDATE ON knowledge_category_versions BEGIN SELECT RAISE(ABORT,'immutable history'); END;
CREATE TRIGGER knowledge_category_versions_no_delete BEFORE DELETE ON knowledge_category_versions BEGIN SELECT RAISE(ABORT,'immutable history'); END;
CREATE TABLE knowledge_cards (
 record_id TEXT PRIMARY KEY REFERENCES records(id), record_revision INTEGER NOT NULL CHECK(record_revision>0),
 content_revision INTEGER NOT NULL CHECK(content_revision>0), overrides_json TEXT NOT NULL CHECK(json_valid(overrides_json)),
 effective_json TEXT NOT NULL CHECK(json_valid(effective_json)), source_asset_id TEXT NOT NULL REFERENCES assets(id),
 category_id TEXT REFERENCES knowledge_categories(id)
);
CREATE INDEX knowledge_card_category ON knowledge_cards(category_id);
CREATE TABLE knowledge_source_documents (
 record_id TEXT PRIMARY KEY REFERENCES knowledge_cards(record_id), content_revision INTEGER NOT NULL CHECK(content_revision>0),
 source_version TEXT NOT NULL CHECK(source_version='manual_effective_v1'), source_json TEXT NOT NULL CHECK(json_valid(source_json)),
 source_sha256 TEXT NOT NULL CHECK(length(source_sha256)=64), updated_at_utc TEXT NOT NULL
);
CREATE TABLE knowledge_projection_debts (
 record_id TEXT PRIMARY KEY REFERENCES knowledge_cards(record_id), target_content_revision INTEGER NOT NULL CHECK(target_content_revision>0),
 source_sha256 TEXT NOT NULL CHECK(length(source_sha256)=64), scheduling_intent TEXT NOT NULL CHECK(scheduling_intent='handler_unavailable')
);
CREATE TABLE knowledge_write_receipts (
 operation_id TEXT PRIMARY KEY CHECK(length(operation_id)=36), principal_id TEXT NOT NULL CHECK(length(principal_id)=36),
 request_fingerprint TEXT NOT NULL CHECK(length(request_fingerprint)=64), result_json TEXT NOT NULL CHECK(json_valid(result_json)), created_at_utc TEXT NOT NULL
);
CREATE TRIGGER knowledge_receipts_no_update BEFORE UPDATE ON knowledge_write_receipts BEGIN SELECT RAISE(ABORT,'immutable receipt'); END;
CREATE TRIGGER knowledge_receipts_no_delete BEFORE DELETE ON knowledge_write_receipts BEGIN SELECT RAISE(ABORT,'immutable receipt'); END;
