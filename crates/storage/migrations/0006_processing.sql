-- S04 additive durable controls. No image rewrites, execution, model adoption or backlog seeds.
CREATE TABLE processing_policy (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL CHECK (revision > 0),
    authorization_epoch INTEGER NOT NULL CHECK (authorization_epoch > 0),
    value_json TEXT NOT NULL CHECK (json_valid(value_json) AND length(value_json) <= 4096)
);
INSERT INTO processing_policy VALUES (1,1,1,'{"mode":"off","background_loading":"forbid","query_loading":"ask","queue_paused":false,"retention":{"kind":"no_host_eviction"}}');
CREATE TABLE processing_profiles (
    id TEXT NOT NULL CHECK (length(id) = 36),
    revision INTEGER NOT NULL CHECK (revision > 0),
    fingerprint TEXT NOT NULL CHECK (length(fingerprint) = 64 AND fingerprint NOT GLOB '*[^0-9a-f]*'),
    value_json TEXT NOT NULL CHECK (json_valid(value_json) AND length(value_json) <= 8192),
    PRIMARY KEY(id,revision)
);
CREATE TABLE processing_execution_host (
    singleton INTEGER PRIMARY KEY CHECK (singleton=1),
    host_id TEXT NOT NULL CHECK (length(host_id)=36),
    runtime_enabled INTEGER NOT NULL CHECK (runtime_enabled IN (0,1))
);
CREATE TABLE processing_batches (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    principal_id TEXT NOT NULL CHECK (length(principal_id) = 36),
    revision INTEGER NOT NULL CHECK (revision > 0),
    control TEXT NOT NULL CHECK (control IN ('pause','resume','cancel')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE processing_jobs (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    batch_id TEXT NOT NULL REFERENCES processing_batches(id),
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 511),
    record_id TEXT NOT NULL REFERENCES records(id),
    asset_id TEXT NOT NULL REFERENCES assets(id),
    value_json TEXT NOT NULL CHECK (json_valid(value_json) AND length(value_json) <= 16384),
    planned_host TEXT CHECK (planned_host IS NULL OR length(planned_host)=36),
    UNIQUE(batch_id,ordinal),
    UNIQUE(batch_id,record_id)
);
CREATE TABLE processing_control_receipts (
    operation_id TEXT PRIMARY KEY CHECK (length(operation_id) = 36),
    principal_id TEXT NOT NULL CHECK (length(principal_id) = 36),
    fingerprint TEXT NOT NULL CHECK (length(fingerprint) = 64),
    reply_json TEXT NOT NULL CHECK (json_valid(reply_json) AND length(reply_json) <= 8388608)
);
CREATE TABLE processing_attempts (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    job_id TEXT NOT NULL REFERENCES processing_jobs(id),
    host_id TEXT NOT NULL CHECK (length(host_id) = 36),
    lease_token TEXT NOT NULL UNIQUE CHECK (length(lease_token) = 36),
    lease_expires_ms INTEGER NOT NULL CHECK (lease_expires_ms > 0),
    authorization_epoch INTEGER NOT NULL CHECK (authorization_epoch > 0),
    state TEXT NOT NULL CHECK (state IN ('claimed','dispatched','settled','uncertain','interrupted')),
    fence_json TEXT NOT NULL CHECK (json_valid(fence_json) AND length(fence_json) <= 32768),
    outcome_json TEXT CHECK (outcome_json IS NULL OR (json_valid(outcome_json) AND length(outcome_json) <= 32768)),
    UNIQUE(job_id,id)
);
CREATE UNIQUE INDEX processing_single_active_attempt ON processing_attempts(job_id) WHERE state IN ('claimed','dispatched','uncertain');
CREATE TABLE processing_derivative_holds (
    attempt_id TEXT PRIMARY KEY REFERENCES processing_attempts(id),
    operation_id TEXT NOT NULL UNIQUE REFERENCES export_operations(id),
    record_id TEXT NOT NULL REFERENCES records(id),
    asset_id TEXT NOT NULL REFERENCES assets(id),
    principal_id TEXT NOT NULL CHECK (length(principal_id) = 36)
);
CREATE TABLE processing_model_leases (
    id TEXT PRIMARY KEY CHECK (length(id)=36),
    attempt_id TEXT NOT NULL UNIQUE REFERENCES processing_attempts(id),
    generation TEXT NOT NULL CHECK (length(generation)=36),
    instance_reference TEXT NOT NULL CHECK (length(instance_reference) BETWEEN 1 AND 512),
    profile_fingerprint TEXT NOT NULL CHECK (length(profile_fingerprint)=64),
    state TEXT NOT NULL CHECK (state IN ('active','released','abandoned','uncertain'))
);
CREATE TABLE processing_runtime_calls (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    attempt_id TEXT NOT NULL REFERENCES processing_attempts(id),
    kind TEXT NOT NULL CHECK (kind IN ('load','inference')),
    generation TEXT NOT NULL CHECK (length(generation) = 36),
    endpoint TEXT NOT NULL CHECK (length(endpoint) BETWEEN 18 AND 27),
    plan_json TEXT NOT NULL CHECK (json_valid(plan_json) AND length(plan_json) <= 16384),
    state TEXT NOT NULL CHECK (state IN ('planned','settled','uncertain')),
    outcome_json TEXT CHECK (outcome_json IS NULL OR (json_valid(outcome_json) AND length(outcome_json) <= 32768))
);
