-- Metadata-only agent authority; no secret bytes or arbitrary credential targets.
CREATE TABLE agent_profiles (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=36),
    principal_id TEXT NOT NULL UNIQUE CHECK(length(principal_id)=36),
    revision INTEGER NOT NULL CHECK(revision>0),
    security_revision INTEGER NOT NULL CHECK(security_revision>0),
    state TEXT NOT NULL CHECK(state IN ('active','disabled','revoked')),
    created_at_utc TEXT NOT NULL,
    updated_at_utc TEXT NOT NULL
);
CREATE TABLE agent_profile_versions (
    profile_id TEXT NOT NULL REFERENCES agent_profiles(id),
    revision INTEGER NOT NULL CHECK(revision>0),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 80 AND instr(name,char(0))=0),
    policy TEXT NOT NULL CHECK(policy='metadata_only_v1'),
    state TEXT NOT NULL CHECK(state IN ('active','disabled','revoked')),
    security_revision INTEGER NOT NULL CHECK(security_revision>0),
    created_at_utc TEXT NOT NULL,
    PRIMARY KEY(profile_id,revision)
);
CREATE TABLE agent_connections (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=36),
    profile_id TEXT NOT NULL REFERENCES agent_profiles(id),
    revision INTEGER NOT NULL CHECK(revision>0),
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 80 AND instr(display_name,char(0))=0),
    state TEXT NOT NULL CHECK(state IN ('enrolling','active','revoked')),
    credential_generation INTEGER NOT NULL CHECK(credential_generation=1),
    expected_digest BLOB NOT NULL CHECK(length(expected_digest)=32),
    created_at_utc TEXT NOT NULL,
    updated_at_utc TEXT NOT NULL
);
CREATE TABLE agent_enrollments (
    operation_id TEXT PRIMARY KEY NOT NULL CHECK(length(operation_id)=36),
    owner_principal_id TEXT NOT NULL CHECK(length(owner_principal_id)=36),
    connection_id TEXT NOT NULL UNIQUE REFERENCES agent_connections(id),
    request_fingerprint TEXT NOT NULL CHECK(length(request_fingerprint)=64),
    state TEXT NOT NULL CHECK(state IN ('planned','activated')),
    created_at_utc TEXT NOT NULL
);
CREATE TABLE access_receipts (
    operation_id TEXT PRIMARY KEY NOT NULL,
    owner_principal_id TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL CHECK(length(request_fingerprint)=64),
    result_json TEXT NOT NULL CHECK(json_valid(result_json)),
    created_at_utc TEXT NOT NULL
);
CREATE TABLE access_cleanup_targets (
    operation_id TEXT NOT NULL,
    connection_id TEXT NOT NULL REFERENCES agent_connections(id),
    PRIMARY KEY(operation_id,connection_id)
);
CREATE TRIGGER access_cleanup_no_update BEFORE UPDATE ON access_cleanup_targets BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
CREATE TRIGGER access_cleanup_no_delete BEFORE DELETE ON access_cleanup_targets BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
CREATE TABLE access_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_id TEXT NOT NULL,
    owner_principal_id TEXT NOT NULL,
    profile_id TEXT NOT NULL REFERENCES agent_profiles(id),
    connection_id TEXT REFERENCES agent_connections(id),
    event_kind TEXT NOT NULL CHECK(event_kind IN ('create_profile','update_profile','plan_connection','activate_connection','revoke_connection')),
    security_revision INTEGER NOT NULL CHECK(security_revision>0),
    occurred_at_utc TEXT NOT NULL,
    UNIQUE(operation_id,event_kind)
);
CREATE INDEX agent_connections_profile ON agent_connections(profile_id,state);
CREATE TRIGGER access_profiles_cap BEFORE INSERT ON agent_profiles WHEN (SELECT count(*) FROM agent_profiles)>=64 BEGIN SELECT RAISE(ABORT,'ACCESS_LIMIT'); END;
CREATE TRIGGER access_connections_cap BEFORE INSERT ON agent_connections WHEN (SELECT count(*) FROM agent_connections)>=128 BEGIN SELECT RAISE(ABORT,'ACCESS_LIMIT'); END;
CREATE TRIGGER access_principal_immutable BEFORE UPDATE OF id,principal_id ON agent_profiles BEGIN SELECT RAISE(ABORT,'ACCESS_IDENTITY_IMMUTABLE'); END;
CREATE TRIGGER access_profile_terminal BEFORE UPDATE ON agent_profiles WHEN OLD.state='revoked' BEGIN SELECT RAISE(ABORT,'ACCESS_TERMINAL'); END;
CREATE TRIGGER access_connection_identity BEFORE UPDATE OF id,profile_id,expected_digest,credential_generation ON agent_connections BEGIN SELECT RAISE(ABORT,'ACCESS_IDENTITY_IMMUTABLE'); END;
CREATE TRIGGER access_connection_terminal BEFORE UPDATE ON agent_connections WHEN OLD.state='revoked' BEGIN SELECT RAISE(ABORT,'ACCESS_TERMINAL'); END;
CREATE TRIGGER access_profiles_no_delete BEFORE DELETE ON agent_profiles BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
CREATE TRIGGER access_connections_no_delete BEFORE DELETE ON agent_connections BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
CREATE TRIGGER access_versions_no_update BEFORE UPDATE ON agent_profile_versions BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
CREATE TRIGGER access_versions_no_delete BEFORE DELETE ON agent_profile_versions BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
CREATE TRIGGER access_enrollment_identity BEFORE UPDATE OF operation_id,owner_principal_id,connection_id,request_fingerprint ON agent_enrollments BEGIN SELECT RAISE(ABORT,'ACCESS_IDENTITY_IMMUTABLE'); END;
CREATE TRIGGER access_enrollment_no_delete BEFORE DELETE ON agent_enrollments BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
CREATE TRIGGER access_receipts_no_update BEFORE UPDATE ON access_receipts BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
CREATE TRIGGER access_receipts_no_delete BEFORE DELETE ON access_receipts BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
CREATE TRIGGER access_events_no_update BEFORE UPDATE ON access_events BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
CREATE TRIGGER access_events_no_delete BEFORE DELETE ON access_events BEGIN SELECT RAISE(ABORT,'ACCESS_HISTORY_IMMUTABLE'); END;
