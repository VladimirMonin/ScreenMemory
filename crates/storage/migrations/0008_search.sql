-- Contentful seven-field FTS5. One synchronous owner hook, no projection triggers.
CREATE TABLE search_projection_meta (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1), projection_epoch INTEGER NOT NULL CHECK(projection_epoch>0),
 lexical_fingerprint TEXT NOT NULL CHECK(lexical_fingerprint='seven_fields_6_3_1_2_2_2_1_win_nfc_unicase2.9_v1')
);
INSERT INTO search_projection_meta VALUES(1,1,'seven_fields_6_3_1_2_2_2_1_win_nfc_unicase2.9_v1');
CREATE TABLE search_documents (
 record_pk INTEGER PRIMARY KEY, record_id TEXT NOT NULL UNIQUE REFERENCES records(id),
 source_asset_id TEXT NOT NULL REFERENCES assets(id), content_revision INTEGER NOT NULL CHECK(content_revision>0),
 source_sha256 TEXT NOT NULL CHECK(length(source_sha256)=64), required_scope_mask INTEGER NOT NULL CHECK(required_scope_mask IN(1,3,5,7)),
 title TEXT NOT NULL, description TEXT NOT NULL, category_id TEXT REFERENCES knowledge_categories(id), category_text TEXT NOT NULL,
 application_text TEXT NOT NULL, window_title TEXT NOT NULL, filename TEXT NOT NULL, ocr_text TEXT NOT NULL,
 source_kind TEXT NOT NULL CHECK(source_kind IN('capture','imported_image')), ingested_at_utc TEXT NOT NULL,
 exact_terms_truncated INTEGER NOT NULL CHECK(exact_terms_truncated IN(0,1))
);
CREATE VIRTUAL TABLE image_fts USING fts5(title,description,category_text,application_text,window_title,filename,ocr_text,tokenize='unicode61');
CREATE TABLE search_exact_terms (
 record_pk INTEGER NOT NULL REFERENCES search_documents(record_pk), term_key TEXT NOT NULL CHECK(length(term_key) BETWEEN 1 AND 128),
 PRIMARY KEY(record_pk,term_key)
);
CREATE INDEX search_exact_lookup ON search_exact_terms(term_key,record_pk);
CREATE INDEX search_category ON search_documents(category_id,record_pk);
CREATE INDEX search_ingested ON search_documents(ingested_at_utc,record_pk);
