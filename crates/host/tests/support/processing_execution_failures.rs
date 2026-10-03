// Kept as a textual include so all native oracles share the accepted per-process SDK owner.
// This file contains public-path tests, not a second independently configured backend.
#[cfg(feature = "processing-test-seams")]
#[tokio::test(flavor = "current_thread")]
async fn s04_execution_load_intent_failure_ask_background_and_readback_refusal() {
    let _serial = SDK_OWNER.lock().await;
    let f = Fixture::new().await;
    // A genuine SQL constraint rollback prevents the SDK load channel. Explicit domain retry
    // creates attempt 2; there is no automatic replay of the failed first intent.
    let root = library();
    let store = Arc::new(MemoryStore::default());
    let mut s = Server::new(&f).await;
    let config = f.config(&s, true).with_load_intent_constraint_rollback();
    let (c, h) = f.start_config(&root, store, config).await;
    let r = f.save(&c).await;
    let p = save_profile(&c, profile(), None).await;
    policy(&c, manual()).await;
    let b = plan(&c, &r, &p, WorkKind::TechnicalOwnerSelfTest).await;
    let j = terminal(&c, &b).await;
    assert_eq!(j.reason, Some(BlockReason::RuntimeRefused));
    let e = execution(&c, j.id).await;
    assert_eq!(e.attempts.len(), 1);
    assert!(e.calls.is_empty());
    assert_eq!(
        e.attempts[0].outcome_code.as_deref(),
        Some("DURABLE_ADMISSION_REFUSED")
    );
    assert!(e.retained_derivatives.is_empty());
    assert_eq!(s.stats().await["loads"], 0);
    assert_eq!(s.stats().await["predictions"], 0);
    c.processing(ProcessingRequest::ControlJob {
        id: j.id,
        expected_revision: j.revision,
        control: JobControl::Retry,
    })
    .await
    .unwrap();
    let j = terminal(&c, &b).await;
    assert_eq!(j.state, JobState::Succeeded);
    assert_eq!(j.attempt_count, 2);
    assert_eq!(execution(&c, j.id).await.attempts.len(), 2);
    assert_eq!(s.stats().await["loads"], 1);
    assert_eq!(s.stats().await["predictions"], 1);
    stop(c, h).await;
    s.close().await;
    remove_own(root);
    // Ask is not an approval flag accepted from a wire request. Host-only approval is absent.
    let root = library();
    let mut s = Server::new(&f).await;
    let (c, h) = f
        .start_config(&root, Arc::new(MemoryStore::default()), f.config(&s, false))
        .await;
    let r = f.save(&c).await;
    let p = save_profile(&c, profile(), None).await;
    policy(
        &c,
        ProcessingPolicy {
            mode: ProcessingMode::Manual,
            ..ProcessingPolicy::default()
        },
    )
    .await;
    let b = plan(&c, &r, &p, WorkKind::TechnicalOwnerSelfTest).await;
    assert_eq!(
        terminal(&c, &b).await.reason,
        Some(BlockReason::RuntimeRefused)
    );
    assert_eq!(s.stats().await["loads"], 0);
    // Independently approved background loading can acquire the lane without query loading.
    policy(
        &c,
        ProcessingPolicy {
            mode: ProcessingMode::Automatic,
            background_loading: LoadPermission::Allow,
            query_loading: QueryLoadPermission::Forbid,
            ..ProcessingPolicy::default()
        },
    )
    .await;
    let b = plan(&c, &r, &p, WorkKind::TechnicalOwnerBackgroundSelfTest).await;
    assert_eq!(terminal(&c, &b).await.state, JobState::Succeeded);
    assert_eq!(s.stats().await["loads"], 1);
    stop(c, h).await;
    s.close().await;
    remove_own(root);
    // An observed mismatch is not converted into inference success or a hidden corrective load.
    let root = library();
    let mut s = Server::new(&f).await;
    s.mode("config_mismatch").await;
    let (c, h) = f.start(&root, &s, Arc::new(MemoryStore::default())).await;
    let r = f.save(&c).await;
    let p = save_profile(&c, profile(), None).await;
    policy(&c, manual()).await;
    let b = plan(&c, &r, &p, WorkKind::TechnicalOwnerSelfTest).await;
    let j = terminal(&c, &b).await;
    assert_ne!(j.state, JobState::Succeeded);
    assert!(j.probe_result.is_none());
    let e = execution(&c, j.id).await;
    assert_eq!(e.calls.len(), 1);
    assert_eq!(e.calls[0].kind, RuntimeCallKind::Load);
    assert_eq!(s.stats().await["loads"], 1);
    assert_eq!(s.stats().await["predictions"], 0);
    stop(c, h).await;
    s.close().await;
    remove_own(root);
    // Persisted load intent is observable while the actual SDK load is pending. Off cannot
    // treat channel cancellation/ACK/process death as settled backend GPU/model work.
    let root = library();
    let store = Arc::new(MemoryStore::default());
    let mut s = Server::new(&f).await;
    s.mode("observe_load").await;
    s.mode("load_hang").await;
    let (c, h) = f.start(&root, &s, store.clone()).await;
    let r = f.save(&c).await;
    let p = save_profile(&c, profile(), None).await;
    policy(&c, manual()).await;
    let b = plan(&c, &r, &p, WorkKind::TechnicalOwnerSelfTest).await;
    s.wait("load_received").await;
    let e = execution(&c, b.jobs[0].id).await;
    assert_eq!(e.calls.len(), 1);
    assert_eq!(e.calls[0].state, RuntimeCallState::Planned);
    assert_eq!(e.calls[0].native_ttl_requested_seconds, Some(900));
    assert_eq!(
        e.calls[0].native_ttl_verification.as_deref(),
        Some("not_verified")
    );
    assert_eq!(e.retained_derivatives.len(), 1);
    policy(&c, ProcessingPolicy::default()).await;
    let j = terminal(&c, &b).await;
    assert_eq!(j.reason, Some(BlockReason::BackendUncertain));
    stop(c, h).await;
    let (c, h) = f.start(&root, &s, store).await;
    let e = execution(&c, j.id).await;
    assert!(e.reconciliation_required);
    assert_eq!(e.calls[0].state, RuntimeCallState::Uncertain);
    assert_eq!(e.retained_derivatives.len(), 1);
    assert_eq!(s.stats().await["loads"], 1);
    assert_eq!(s.stats().await["predictions"], 0);
    stop(c, h).await;
    s.close().await;
    remove_own(root);
    remove_own(f.package_root);
}
