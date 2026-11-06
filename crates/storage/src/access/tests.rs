use super::*;
use sm_application::Scope;
fn id() -> KnowledgeId {
    KnowledgeId(Uuid::new_v4())
}
fn owner() -> RequestContext {
    RequestContext::from_authenticated_peer(
        RequestId::new(Uuid::new_v4()),
        CorrelationId::new(Uuid::new_v4()),
        PrincipalId::new(Uuid::new_v4()),
        vec![Scope::ManageAccess],
    )
}
fn db() -> Connection {
    let mut db = Connection::open_in_memory().unwrap();
    crate::migration::initialize(&mut db, true, false).unwrap();
    db
}
fn create() -> AccessRequest {
    AccessRequest::CreateProfile {
        operation_id: id(),
        name: sm_protocol::knowledge::CategoryName("Agent".into()),
        policy: AgentPolicy::default(),
    }
}
fn profile_reply(r: Reply) -> AgentProfileView {
    match r {
        Reply::Public(AccessReply::Profile(p)) => *p,
        _ => panic!("profile"),
    }
}
fn issue(p: &AgentProfileView) -> AccessRequest {
    AccessRequest::IssueConnection {
        operation_id: id(),
        profile_id: p.profile_id,
        expected_profile_revision: p.revision,
        display_name: sm_protocol::knowledge::CategoryName("Connection".into()),
    }
}
#[test]
fn s07_access_receipts_history_caps_and_terminal_replay() {
    let mut db = db();
    let ctx = owner();
    let req = create();
    let p = profile_reply(handle(&mut db, ctx.clone(), Command::Manage(req.clone())).unwrap());
    let replay = profile_reply(handle(&mut db, ctx.clone(), Command::Manage(req.clone())).unwrap());
    assert_eq!(p.profile_id, replay.profile_id);
    let other = owner();
    assert!(
        matches!(handle(&mut db,other,Command::Manage(req.clone())),Err(e) if e.code==ErrorCode::IdempotencyConflict)
    );
    assert!(db.execute("DELETE FROM access_receipts", []).is_err());
    assert!(db
        .execute("UPDATE agent_profile_versions SET name='Other'", [])
        .is_err());
    assert!(db.execute("DELETE FROM access_events", []).is_err());
    assert!(db
        .execute(
            "UPDATE agent_profiles SET principal_id=?1",
            [Uuid::new_v4().to_string()]
        )
        .is_err());
    for _ in 1..64 {
        handle(&mut db, ctx.clone(), Command::Manage(create())).unwrap();
    }
    assert!(
        matches!(handle(&mut db,ctx.clone(),Command::Manage(create())),Err(e) if e.code==ErrorCode::InvalidRequest)
    );
    let revoke = AccessRequest::UpdateProfile {
        operation_id: id(),
        profile_id: p.profile_id,
        expected_revision: p.revision,
        name: p.name.clone(),
        policy: AgentPolicy::default(),
        state: AgentProfileState::Revoked,
    };
    let revoked =
        profile_reply(handle(&mut db, ctx.clone(), Command::Manage(revoke.clone())).unwrap());
    assert_eq!(revoked.security_revision.0, 2);
    handle(&mut db, ctx.clone(), Command::Manage(revoke)).unwrap();
    let restore = AccessRequest::UpdateProfile {
        operation_id: id(),
        profile_id: p.profile_id,
        expected_revision: revoked.revision,
        name: p.name,
        policy: AgentPolicy::default(),
        state: AgentProfileState::Active,
    };
    assert!(handle(&mut db, ctx, Command::Manage(restore)).is_err());
}
#[test]
fn s07_access_planned_digest_activation_collision_and_denied_secret() {
    let mut db = db();
    let ctx = owner();
    let p = profile_reply(handle(&mut db, ctx.clone(), Command::Manage(create())).unwrap());
    let req = issue(&p);
    let secret = Secret::generate_session().unwrap();
    let d = secret.session_digest().unwrap();
    let plan = match handle(&mut db, ctx.clone(), Command::Plan(req.clone(), d)).unwrap() {
        Reply::Plan(Some(p)) => p,
        _ => panic!("plan"),
    };
    assert_eq!(plan.connection.state, AgentConnectionState::Enrolling);
    assert_eq!(digest(&db, plan.connection.connection_id).unwrap(), d);
    assert_eq!(
        db.query_row("SELECT count(*) FROM access_receipts", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert!(handle(
        &mut db,
        ctx.clone(),
        Command::Activate(req.clone(), [0; 32])
    )
    .is_err());
    let replay = match handle(&mut db, ctx.clone(), Command::Plan(req.clone(), [3; 32])).unwrap() {
        Reply::Plan(Some(p)) => p,
        _ => panic!("plan"),
    };
    assert_eq!(
        replay.connection.connection_id,
        plan.connection.connection_id
    );
    assert_eq!(replay.expected_digest, d);
    let active = match handle(&mut db, ctx.clone(), Command::Activate(req.clone(), d)).unwrap() {
        Reply::Public(AccessReply::Connection(c)) => c,
        _ => panic!("active"),
    };
    let library = LibraryId::new(
        Uuid::parse_str(
            &db.query_row("SELECT library_id FROM library_meta", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        )
        .unwrap(),
    );
    let authenticate = |secret: Secret| Command::Authenticate {
        library,
        connection: active.connection_id,
        secret,
        request: RequestId::new(Uuid::new_v4()),
        correlation: CorrelationId::new(Uuid::new_v4()),
    };
    assert!(handle(
        &mut db,
        ctx.clone(),
        authenticate(Secret::generate_session().unwrap())
    )
    .is_err());
    let bound = match handle(
        &mut db,
        ctx.clone(),
        authenticate(Secret::from_bytes(secret.as_bytes().to_vec())),
    )
    .unwrap()
    {
        Reply::Context(c) => c,
        _ => panic!("ctx"),
    };
    assert!(!bound.allows(Scope::ManageHost));
    assert!(!bound.allows(Scope::ReadImages));
    for _ in 1..128 {
        let r = issue(&p);
        handle(&mut db, ctx.clone(), Command::Plan(r, d)).unwrap();
    }
    assert!(
        matches!(handle(&mut db,ctx.clone(),Command::Plan(issue(&p),d)),Err(e) if e.code==ErrorCode::InvalidRequest)
    );
    let revoke = AccessRequest::RevokeConnection {
        operation_id: id(),
        connection_id: active.connection_id,
        expected_revision: active.revision,
    };
    handle(&mut db, ctx.clone(), Command::Manage(revoke.clone())).unwrap();
    assert!(validate(&db, &bound).is_err());
    assert!(handle(&mut db, ctx.clone(), authenticate(secret)).is_err());
    handle(&mut db, ctx, Command::Manage(revoke)).unwrap();
}
#[test]
fn s07_access_transaction_failure_rolls_back_identity_event_receipt() {
    let mut db = db();
    db.execute_batch("CREATE TRIGGER access_fault BEFORE INSERT ON access_receipts BEGIN SELECT RAISE(ABORT,'synthetic'); END;").unwrap();
    assert!(handle(&mut db, owner(), Command::Manage(create())).is_err());
    for table in [
        "agent_profiles",
        "agent_profile_versions",
        "access_events",
        "access_receipts",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}
