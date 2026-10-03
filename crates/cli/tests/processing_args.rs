use sm_cli::{parse_args, Command};
#[test]
fn native_processing_vocabulary_is_closed_and_owner_authority_cannot_be_arguments() {
    let base = ["--config", "C:\\synthetic\\client.json", "processing"];
    for action in ["status", "jobs", "inventory", "policy", "reconcile"] {
        let mut args = base.map(str::to_owned).to_vec();
        args.push(action.into());
        let parsed = parse_args(&args).unwrap();
        assert!(parsed.json);
        assert!(matches!(parsed.command, Command::Processing(_)));
        for flag in [
            "--package-path",
            "--token",
            "--endpoint",
            "--load",
            "--approval",
        ] {
            let mut bad = args.clone();
            bad.extend([flag.into(), "injected".into()]);
            assert!(parse_args(&bad).is_err());
        }
    }
    for action in ["claim", "unload", "adopt", "download", "force-settle"] {
        let mut args = base.map(str::to_owned).to_vec();
        args.push(action.into());
        assert!(parse_args(&args).is_err());
    }
}
#[test]
fn self_test_requires_explicit_stable_operation_policy_record_and_profile_identifiers() {
    let id = "00000000-0000-4000-8000-000000000001";
    let args = [
        "--config",
        "C:\\synthetic\\client.json",
        "processing",
        "self-test",
        "--record-id",
        id,
        "--profile-id",
        id,
        "--policy-revision",
        "1",
        "--operation-id",
        id,
    ]
    .map(str::to_owned)
    .to_vec();
    assert!(parse_args(&args).is_ok());
    let mut missing = args.clone();
    missing.truncate(missing.len() - 2);
    assert!(parse_args(&missing).is_err());
    let mut malformed = args;
    *malformed.last_mut().unwrap() = "not-uuid".into();
    assert!(parse_args(&malformed).is_err());
}
